const { app, BrowserWindow, ipcMain, dialog, shell, nativeTheme, Tray, Menu } = require('electron');
const path = require('path');
const fs = require('fs');
const os = require('os');
const crypto = require('crypto');
const { spawn, spawnSync } = require('child_process');
const pty = require('node-pty');

// Earlier versions were called "Claude Terminal" and then "Tandem", and Electron names the data
// folder (settings, browser tabs, website sign-ins) after the app. Electron creates the new folder
// before this code runs, so "has config.json" is what marks a folder as actually used. If the
// current folder hasn't been used yet, keep using the newest earlier one in place.
(() => {
  const appData = app.getPath('appData');
  const used = (dir) => fs.existsSync(path.join(dir, 'config.json'));
  if (used(app.getPath('userData'))) return;
  const legacy = ['Tandem', 'Claude Terminal'].map((n) => path.join(appData, n)).find(used);
  if (legacy) app.setPath('userData', legacy);
})();

// One copy at a time: launching Crew again brings the running window back instead of starting a
// second instance (which would also wait several seconds on the locked browser storage).
if (!app.requestSingleInstanceLock()) {
  app.quit();
  return;
}
app.on('second-instance', () => showWindow());

// Saved SSH hosts for the agents (after userData is settled above: it keeps its files there).
const ssh = require('./ssh');

const CLAUDE_DIR = path.join(os.homedir(), '.claude', 'projects');
const CODEX_HOME = process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
const CODEX_SESSIONS = path.join(CODEX_HOME, 'sessions');
const CODEX_INDEX = path.join(CODEX_HOME, 'session_index.jsonl'); // thread names, last entry wins
// Codex Desktop puts project-less chats in dated scratch folders here; they're grouped as one project.
const CODEX_CHATS = path.join(os.homedir(), 'Documents', 'Codex');
const CONFIG_PATH = path.join(app.getPath('userData'), 'config.json');

let win = null;
let tray = null;
let quitting = false; // true once the user really quits, so closing the window doesn't just hide it
const ptys = new Map(); // termId -> { proc, sessionId, cwd, agent }

// ---------- config ----------

const defaultConfig = {
  claudeCommand: 'claude',
  extraArgs: '',
  codexCommand: 'codex',
  codexArgs: '',
  defaultAgent: 'claude', // agent picked last time a session was started
  theme: 'light',
  fontSize: 14,
  extraProjects: [], // folders added manually that may not have sessions yet
  hiddenProjects: [],
  collapsed: {},
  projectOrder: [], // project paths in sidebar order, set by dragging
  closeToTray: true, // closing the window keeps Crew (and running sessions) in the tray
  showUsage: true, // plan usage bar at the bottom; adds a status line to Claude sessions Crew starts
  trayHintShown: false,
};

function loadConfig() {
  try {
    return { ...defaultConfig, ...JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8')) };
  } catch {
    return { ...defaultConfig };
  }
}

function saveConfig(cfg) {
  fs.mkdirSync(path.dirname(CONFIG_PATH), { recursive: true });
  fs.writeFileSync(CONFIG_PATH, JSON.stringify(cfg, null, 2));
}

let config = loadConfig();

// ---------- session scanning ----------

// Session files can be tens of MB, so only read the head (cwd, first prompt)
// and the tail (latest title). Results are cached by mtime + size.
const metaCache = new Map();

function readSlice(fd, start, length) {
  const buf = Buffer.alloc(length);
  const n = fs.readSync(fd, buf, 0, length, start);
  return buf.subarray(0, n).toString('utf8');
}

function parseLines(text, dropFirst, dropLast) {
  const lines = text.split('\n');
  if (dropFirst) lines.shift();
  if (dropLast) lines.pop();
  const out = [];
  for (const l of lines) {
    if (!l.trim()) continue;
    try { out.push(JSON.parse(l)); } catch { /* partial line */ }
  }
  return out;
}

function textOfContent(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    const t = content.find((c) => c && c.type === 'text');
    return t ? t.text : '';
  }
  return '';
}

function cleanPrompt(s) {
  return s
    .replace(/<[^>]+>[\s\S]*?<\/[^>]+>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function matchJsonString(text, re) {
  const m = text.match(re);
  const raw = m && (m[1] ?? m[2]);
  if (raw == null) return null;
  try { return JSON.parse(`"${raw}"`); } catch { return null; }
}

function readSessionMeta(file) {
  const st = fs.statSync(file);
  const key = file;
  const cached = metaCache.get(key);
  if (cached && cached.mtimeMs === st.mtimeMs && cached.size === st.size) return cached.meta;

  const fd = fs.openSync(file, 'r');
  let cwd = null;
  let firstPrompt = null;
  let title = null;
  let customTitle = null;
  try {
    const HEAD = 128 * 1024;
    const headText = readSlice(fd, 0, Math.min(HEAD, st.size));
    for (const o of parseLines(headText, false, st.size > HEAD)) {
      if (!cwd && o.cwd) cwd = o.cwd;
      if (!firstPrompt && o.type === 'user' && o.message && !o.isMeta && !o.isSidechain) {
        const t = cleanPrompt(textOfContent(o.message.content));
        if (t && !t.startsWith('Caveat:')) firstPrompt = t;
      }
      if (o.type === 'custom-title' && o.customTitle) customTitle = o.customTitle;
      if (o.type === 'ai-title' && o.aiTitle) title = o.aiTitle;
      if (o.type === 'summary' && o.summary && !title) title = o.summary;
    }
    // A line with an inline image can be larger than HEAD and gets dropped as partial;
    // pull the fields straight out of the raw text instead.
    if (!cwd) cwd = matchJsonString(headText, /"cwd":"((?:[^"\\]|\\.)*)"/);
    if (!firstPrompt) {
      const t = matchJsonString(headText, /"role":"user","content":(?:"((?:[^"\\]|\\.)*)"|\[\{"type":"text","text":"((?:[^"\\]|\\.)*)")/);
      if (t) firstPrompt = cleanPrompt(t.replace(/^\[Image #\d+\]\s*/, '')) || null;
    }
    if (st.size > HEAD) {
      const TAIL = 512 * 1024;
      const start = Math.max(HEAD, st.size - TAIL);
      const tailText = readSlice(fd, start, st.size - start);
      for (const o of parseLines(tailText, true, false)) {
        if (o.type === 'custom-title' && o.customTitle) customTitle = o.customTitle;
        if (o.type === 'ai-title' && o.aiTitle) title = o.aiTitle;
        if (!cwd && o.cwd) cwd = o.cwd;
      }
    }
  } finally {
    fs.closeSync(fd);
  }

  const meta = {
    id: path.basename(file, '.jsonl'),
    agent: 'claude',
    cwd,
    title: customTitle || title || (firstPrompt ? firstPrompt.slice(0, 80) : null),
    mtime: st.mtimeMs,
    size: st.size,
  };
  metaCache.set(key, { mtimeMs: st.mtimeMs, size: st.size, meta });
  return meta;
}

// Codex: rollout files under sessions/YYYY/MM/DD. The first line is session_meta (id, cwd, source);
// names live in session_index.jsonl. Returns null for sessions that shouldn't be listed.
function readCodexMeta(file) {
  const st = fs.statSync(file);
  const cached = metaCache.get(file);
  if (cached && cached.mtimeMs === st.mtimeMs && cached.size === st.size) return cached.meta;

  let meta = null;
  if (st.size > 0) {
    const HEAD = 256 * 1024;
    const fd = fs.openSync(file, 'r');
    let headText;
    try { headText = readSlice(fd, 0, Math.min(HEAD, st.size)); } finally { fs.closeSync(fd); }
    let info = null;
    let firstPrompt = null;
    for (const o of parseLines(headText, false, st.size > HEAD)) {
      if (o.type === 'session_meta' && o.payload) info = o.payload;
      if (firstPrompt) continue;
      const pl = o.payload || {};
      if (o.type === 'event_msg' && pl.type === 'user_message' && pl.message) firstPrompt = pl.message;
      if (o.type === 'response_item' && pl.type === 'message' && pl.role === 'user' && Array.isArray(pl.content)) {
        const t = pl.content.find((c) => c.type === 'input_text' && c.text && !c.text.trimStart().startsWith('<'));
        if (t) firstPrompt = t.text;
      }
    }
    if (!info) {
      // session_meta carries the full system prompt and can be longer than HEAD.
      const src = headText.match(/"source":(\{|"([^"]*)")/);
      info = {
        id: matchJsonString(headText, /"id":"((?:[^"\\]|\\.)*)"/),
        cwd: matchJsonString(headText, /"cwd":"((?:[^"\\]|\\.)*)"/),
        timestamp: matchJsonString(headText, /"timestamp":"((?:[^"\\]|\\.)*)"/),
        source: src ? (src[2] ?? {}) : undefined,
      };
    }
    // Sub-agent threads (e.g. Guardian reviews) and headless `codex exec` runs aren't conversations to resume.
    if (info.id && info.cwd && typeof info.source === 'string' && info.source !== 'exec') {
      meta = {
        id: info.id,
        agent: 'codex',
        cwd: info.cwd,
        created: Date.parse(info.timestamp) || st.birthtimeMs,
        firstPrompt: firstPrompt ? cleanPrompt(firstPrompt).slice(0, 80) : null,
        mtime: st.mtimeMs,
        size: st.size,
      };
    }
  }
  metaCache.set(file, { mtimeMs: st.mtimeMs, size: st.size, meta });
  return meta;
}

let codexNamesCache = { mtimeMs: 0, size: -1, names: {} };
function readCodexNames() {
  try {
    const st = fs.statSync(CODEX_INDEX);
    if (st.mtimeMs === codexNamesCache.mtimeMs && st.size === codexNamesCache.size) return codexNamesCache.names;
    const names = {};
    for (const o of parseLines(fs.readFileSync(CODEX_INDEX, 'utf8'), false, false)) {
      if (o.id && o.thread_name) names[o.id] = o.thread_name;
    }
    codexNamesCache = { mtimeMs: st.mtimeMs, size: st.size, names };
    return names;
  } catch {
    return {};
  }
}

function listFiles(dir, ext) {
  let out = [];
  let entries = [];
  try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return out; }
  for (const e of entries) {
    const f = path.join(dir, e.name);
    if (e.isDirectory()) out = out.concat(listFiles(f, ext));
    else if (e.name.endsWith(ext)) out.push(f);
  }
  return out;
}

function isInside(child, parent) {
  const c = normPath(child);
  const p = normPath(parent);
  return c === p || c.startsWith(p + path.sep);
}

// Fallback when no session records a cwd: "C--Users-foo-bar" -> "C:\Users\foo\bar" (lossy).
function decodeDirName(name) {
  const m = name.match(/^([A-Za-z])--(.*)$/);
  if (m) return `${m[1].toUpperCase()}:\\${m[2].replace(/-/g, '\\')}`;
  return name.replace(/-/g, '/');
}

function normPath(p) {
  return path.resolve(p).replace(/[\\/]+$/, '').toLowerCase();
}

function scanProjects() {
  const projects = new Map(); // normPath -> project

  let dirs = [];
  try { dirs = fs.readdirSync(CLAUDE_DIR, { withFileTypes: true }).filter((d) => d.isDirectory()); } catch { /* none yet */ }

  for (const d of dirs) {
    const dir = path.join(CLAUDE_DIR, d.name);
    let files = [];
    try { files = fs.readdirSync(dir).filter((f) => f.endsWith('.jsonl')); } catch { continue; }
    const sessions = [];
    for (const f of files) {
      try {
        const meta = readSessionMeta(path.join(dir, f));
        if (meta.size === 0) continue;
        sessions.push(meta);
      } catch { /* unreadable */ }
    }
    const cwd = sessions.find((s) => s.cwd)?.cwd || decodeDirName(d.name);
    const key = normPath(cwd);
    const proj = projects.get(key) || { path: cwd, name: path.basename(cwd) || cwd, sessions: [] };
    proj.sessions.push(...sessions.filter((s) => s.title)); // skip sessions with no prompt yet
    projects.set(key, proj);
  }

  const codexNames = readCodexNames();
  for (const f of listFiles(CODEX_SESSIONS, '.jsonl')) {
    let meta;
    try { meta = readCodexMeta(f); } catch { continue; }
    if (!meta) continue;
    const title = codexNames[meta.id] || meta.firstPrompt;
    if (!title) continue; // nothing typed yet
    const chat = isInside(meta.cwd, CODEX_CHATS);
    const projPath = chat ? CODEX_CHATS : meta.cwd;
    const key = normPath(projPath);
    const proj = projects.get(key) || { path: projPath, name: chat ? 'Codex chats' : path.basename(projPath) || projPath, sessions: [] };
    proj.sessions.push({ ...meta, title });
    projects.set(key, proj);
  }

  for (const p of config.extraProjects) {
    const key = normPath(p);
    if (!projects.has(key)) projects.set(key, { path: p, name: path.basename(p) || p, sessions: [] });
  }

  // The usage check's folder too, in case Claude ever saves a transcript for it.
  const hidden = new Set([...config.hiddenProjects, path.join(app.getPath('userData'), 'usage-probe')].map(normPath));
  const list = [...projects.values()].filter((p) => !hidden.has(normPath(p.path)));
  for (const p of list) {
    p.sessions.sort((a, b) => b.mtime - a.mtime);
    p.mtime = p.sessions[0]?.mtime || 0;
    p.exists = fs.existsSync(p.path);
  }
  // Projects keep the order the user set by dragging. Ones not placed yet (the first run, or a new
  // folder) are added at the end, newest first, and from then on stay where they are.
  list.sort((a, b) => b.mtime - a.mtime || a.name.localeCompare(b.name));
  const order = config.projectOrder.map(normPath);
  const missing = list.filter((p) => !order.includes(normPath(p.path)));
  if (missing.length) {
    config.projectOrder = [...config.projectOrder, ...missing.map((p) => p.path)];
    saveConfig(config);
    order.push(...missing.map((p) => normPath(p.path)));
  }
  list.sort((a, b) => order.indexOf(normPath(a.path)) - order.indexOf(normPath(b.path)));
  return list;
}

// Notify the renderer when session files change (debounced).
let watchTimer = null;
function notifyChanged() {
  clearTimeout(watchTimer);
  watchTimer = setTimeout(() => win && !win.isDestroyed() && win.webContents.send('projects:changed'), 1500);
}

function startWatcher() {
  const watch = (dir, opts, filter) => {
    try {
      fs.watch(dir, opts, (_ev, file) => { if (!file || filter(String(file))) notifyChanged(); });
    } catch (e) {
      console.error('watch failed', dir, e.message);
    }
  };
  fs.mkdirSync(CLAUDE_DIR, { recursive: true });
  watch(CLAUDE_DIR, { recursive: true }, (f) => f.endsWith('.jsonl'));
  if (fs.existsSync(CODEX_SESSIONS)) watch(CODEX_SESSIONS, { recursive: true }, (f) => f.endsWith('.jsonl'));
  if (fs.existsSync(CODEX_HOME)) watch(CODEX_HOME, {}, (f) => f === 'session_index.jsonl');
}

// ---------- pty ----------

function splitArgs(s) {
  const out = [];
  const re = /"([^"]*)"|(\S+)/g;
  let m;
  while ((m = re.exec(s || ''))) out.push(m[1] ?? m[2]);
  return out;
}

// Windows only updates an app's PATH when it starts, so a tool installed while Crew is open (e.g.
// from the install button) wouldn't be found. Re-read the machine and user PATH from the registry.
function registryPath(key) {
  const r = spawnSync('reg', ['query', key, '/v', 'Path'], { encoding: 'utf8', windowsHide: true, timeout: 3000 });
  const m = (r.stdout || '').match(/^\s*Path\s+REG_(?:EXPAND_)?SZ\s+(.*)$/mi);
  return m ? m[1].trim().replace(/%([^%]+)%/g, (all, v) => process.env[v] ?? all) : '';
}

function freshPath() {
  const current = process.env.Path || process.env.PATH || '';
  if (process.platform !== 'win32') return current;
  const parts = [
    registryPath('HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment'),
    registryPath('HKCU\\Environment'),
    current,
  ].join(';').split(';').filter(Boolean);
  return [...new Map(parts.map((p) => [p.replace(/[\\/]+$/, '').toLowerCase(), p])).values()].join(';');
}

function cleanEnv() {
  const env = { ...process.env, TERM: 'xterm-256color', COLORTERM: 'truecolor', FORCE_COLOR: '3' };
  // Don't let a parent Claude Code / Electron environment leak into the child.
  for (const k of Object.keys(env)) {
    if (k === 'CLAUDECODE' || k.startsWith('CLAUDE_CODE_') || k.startsWith('ELECTRON_')) delete env[k];
  }
  const pathKey = Object.keys(env).find((k) => k.toUpperCase() === 'PATH') || 'PATH';
  env[pathKey] = freshPath();
  return env;
}

// ---------- agent install check ----------

// Crew runs the agents' own CLIs and never bundles them. When one is missing, the pane offers the
// official install command instead of failing with "'claude' is not recognized".
const INSTALL = {
  claude: {
    name: 'Claude Code',
    page: 'https://docs.anthropic.com/en/docs/claude-code/setup',
    argv: process.platform === 'win32'
      ? ['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', 'irm https://claude.ai/install.ps1 | iex']
      : ['bash', '-lc', 'curl -fsSL https://claude.ai/install.sh | bash'],
    display: process.platform === 'win32' ? 'irm https://claude.ai/install.ps1 | iex' : 'curl -fsSL https://claude.ai/install.sh | bash',
  },
  codex: {
    name: 'Codex',
    page: 'https://github.com/openai/codex',
    needsNpm: true,
    argv: ['npm', 'install', '-g', '@openai/codex'],
    display: 'npm install -g @openai/codex',
  },
};

function commandExists(cmd, env) {
  if (/[\\/]/.test(cmd)) return fs.existsSync(cmd) || (process.platform === 'win32' && ['.exe', '.cmd', '.bat'].some((x) => fs.existsSync(cmd + x)));
  const r = process.platform === 'win32'
    ? spawnSync('where', [cmd], { env, windowsHide: true, timeout: 5000 })
    : spawnSync(process.env.SHELL || '/bin/bash', ['-lc', `command -v '${cmd.replace(/'/g, "'\\''")}'`], { env, timeout: 5000 });
  return r.status === 0;
}

ipcMain.handle('agent:check', (_e, agent) => {
  const command = splitArgs(agent === 'codex' ? config.codexCommand || 'codex' : config.claudeCommand || 'claude')[0];
  const env = cleanEnv();
  const info = INSTALL[agent] || INSTALL.claude;
  return {
    installed: commandExists(command, env),
    command,
    name: info.name,
    page: info.page,
    installCommand: info.display,
    npmMissing: !!info.needsNpm && !commandExists('npm', env),
  };
});

// Claude takes the session id up front; Codex picks its own, so a new Codex session
// is matched to its rollout file later by cwd and start time (see the renderer's refresh()).
function agentArgv(agent, id, resume) {
  if (agent === 'codex') {
    const extra = splitArgs(config.codexArgs);
    return [config.codexCommand || 'codex', ...(resume ? ['resume', ...extra, id] : extra)];
  }
  const usage = config.showUsage !== false && usageHookReady() ? ['--settings', usagePaths().settings] : [];
  return [config.claudeCommand || 'claude', ...usage, ...splitArgs(config.extraArgs), ...(resume ? ['--resume', id] : ['--session-id', id])];
}

// claude/codex are usually .cmd shims on Windows; go through cmd.exe so PATHEXT resolution works.
// The line after /c is wrapped in one more pair of quotes, which /s strips, so arguments quoted
// for spaces (e.g. a path under "Claude Terminal") reach the agent intact. That only works if
// the arguments go to cmd.exe verbatim: as a string for node-pty, and with
// windowsVerbatimArguments for child_process (both would otherwise re-escape the quotes).
function shellCommand(argv) {
  if (process.platform === 'win32') {
    return [process.env.COMSPEC || 'cmd.exe', ['/d', '/s', '/c', `"${argv.map((a) => (/\s/.test(a) ? `"${a}"` : a)).join(' ')}"`]];
  }
  return [process.env.SHELL || '/bin/bash', ['-lc', argv.map((a) => `'${a.replace(/'/g, "'\\''")}'`).join(' ')]];
}

function startPty(file, fileArgs, { cwd, cols, rows, meta, env = cleanEnv() }) {
  const termId = crypto.randomUUID();
  const workDir = cwd && fs.existsSync(cwd) ? cwd : os.homedir();
  const verbatim = process.platform === 'win32' && file === (process.env.COMSPEC || 'cmd.exe');
  const proc = pty.spawn(file, verbatim ? fileArgs.join(' ') : fileArgs, {
    name: 'xterm-256color',
    cols: cols || 120,
    rows: rows || 30,
    cwd: workDir,
    env,
    useConpty: true,
  });

  ptys.set(termId, { proc, cwd, ...meta });
  proc.onData((data) => win && !win.isDestroyed() && win.webContents.send('pty:data', termId, data));
  proc.onExit(({ exitCode }) => {
    ptys.delete(termId);
    win && !win.isDestroyed() && win.webContents.send('pty:exit', termId, exitCode);
  });
  return termId;
}

ipcMain.handle('pty:spawn', (_e, { agent = 'claude', cwd, sessionId, resume, cols, rows }) => {
  const id = sessionId || crypto.randomUUID();
  const [file, fileArgs] = shellCommand(agentArgv(agent, id, resume));
  // Agent sessions get crew-ssh, for the hosts saved in Crew.
  const env = ssh.sessionEnv(cleanEnv(), id, agent);
  const termId = startPty(file, fileArgs, { cwd, cols, rows, env, meta: { sessionId: id, agent } });
  return { termId, sessionId: id, startedAt: Date.now() };
});

// Runs the agent's official installer in the pane, so its output (and any prompts) are visible.
// The user's terminal on a saved host: an SSH shell that's a terminal like the others. The
// window picks the id first, so nothing the host prints before the reply is lost.
ipcMain.handle('ssh:shell', async (_e, { hostId, termId, cols, rows }) => {
  const send = (ch, ...a) => win && !win.isDestroyed() && win.webContents.send(ch, termId, ...a);
  try {
    const proc = await ssh.openShell(hostId, cols || 120, rows || 30, (d) => send('pty:data', d), (code) => {
      ptys.delete(termId);
      send('pty:exit', code);
    });
    ptys.set(termId, { proc, sessionId: `ssh:${hostId}`, agent: 'ssh' });
    return { termId };
  } catch (e) {
    return { error: e.message || String(e) };
  }
});

ipcMain.handle('pty:install', (_e, { agent, cols, rows }) => {
  const { argv } = INSTALL[agent] || INSTALL.claude;
  const [file, fileArgs] = argv[0] === 'npm' ? shellCommand(argv) : [argv[0], argv.slice(1)];
  return startPty(file, fileArgs, { cols, rows, meta: { agent, installing: true } });
});

ipcMain.on('pty:write', (_e, termId, data) => ptys.get(termId)?.proc.write(data));
ipcMain.on('pty:resize', (_e, termId, cols, rows) => {
  try { ptys.get(termId)?.proc.resize(Math.max(cols, 2), Math.max(rows, 2)); } catch { /* exited */ }
});
// A new Codex terminal starts under a temporary id; the renderer reports the real one once known.
ipcMain.on('pty:rekey', (_e, termId, sessionId) => {
  const p = ptys.get(termId);
  if (p) p.sessionId = sessionId;
});
ipcMain.on('pty:kill', (_e, termId) => {
  const p = ptys.get(termId);
  if (p) { try { p.proc.kill(); } catch { /* already gone */ } ptys.delete(termId); }
});

// ---------- projects / config ----------

ipcMain.handle('projects:list', () => scanProjects());

ipcMain.handle('projects:add', async () => {
  const r = await dialog.showOpenDialog(win, { properties: ['openDirectory'], title: 'Add project folder' });
  if (r.canceled || !r.filePaths[0]) return null;
  const p = r.filePaths[0];
  config.hiddenProjects = config.hiddenProjects.filter((h) => normPath(h) !== normPath(p));
  if (!config.extraProjects.some((x) => normPath(x) === normPath(p))) config.extraProjects.push(p);
  saveConfig(config);
  return p;
});

ipcMain.handle('projects:hide', (_e, p) => {
  config.extraProjects = config.extraProjects.filter((x) => normPath(x) !== normPath(p));
  if (!config.hiddenProjects.some((x) => normPath(x) === normPath(p))) config.hiddenProjects.push(p);
  saveConfig(config);
});

ipcMain.handle('projects:reveal', (_e, p) => shell.openPath(p));

// File links clicked in the terminal / image viewer.
ipcMain.handle('file:exists', (_e, p) => fs.existsSync(p));
ipcMain.handle('file:open', (_e, p) => shell.openPath(p));
ipcMain.handle('file:show', (_e, p) => shell.showItemInFolder(p));

// Images pasted into the composer are saved here so they can be handed to the agent as files.
const ATTACH_DIR = path.join(app.getPath('userData'), 'attachments');

ipcMain.handle('attachments:save', (_e, bytes, mime) => {
  fs.mkdirSync(ATTACH_DIR, { recursive: true });
  const ext = { 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp', 'image/bmp': 'bmp' }[mime] || 'png';
  const stamp = new Date().toISOString().replace(/[-:]/g, '').replace('T', '-').slice(0, 15);
  const file = path.join(ATTACH_DIR, `pasted-${stamp}-${crypto.randomBytes(3).toString('hex')}.${ext}`);
  fs.writeFileSync(file, Buffer.from(bytes));
  return file;
});

function cleanAttachments() {
  const cutoff = Date.now() - 7 * 24 * 3600 * 1000;
  let files = [];
  try { files = fs.readdirSync(ATTACH_DIR); } catch { return; }
  for (const f of files) {
    const p = path.join(ATTACH_DIR, f);
    try { if (fs.statSync(p).mtimeMs < cutoff) fs.unlinkSync(p); } catch { /* in use */ }
  }
}

ipcMain.handle('dialog:files', async () => {
  const r = await dialog.showOpenDialog(win, { properties: ['openFile', 'multiSelections'], title: 'Attach files' });
  return r.canceled ? [] : r.filePaths;
});

// ---------- session settings (model / effort / mode) for the composer ----------

function readTail(file, bytes) {
  const st = fs.statSync(file);
  const fd = fs.openSync(file, 'r');
  try { return readSlice(fd, Math.max(0, st.size - bytes), Math.min(bytes, st.size)); } finally { fs.closeSync(fd); }
}

function lastMatch(text, re) {
  let m, last = null;
  while ((m = re.exec(text))) last = m[1];
  return last;
}

function readJson(file) {
  try { return JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return {}; }
}

function claudeDefaults() {
  const s = readJson(path.join(os.homedir(), '.claude', 'settings.json'));
  const model = s.model || null;
  const perModel = s.modelSettings || {};
  // modelSettings is keyed by full model id; match an alias like "opus" to its entry.
  const key = Object.keys(perModel).find((k) => model && (k === model || k.includes(`-${model}-`)));
  return { model, effort: (key && perModel[key].effortLevel) || s.effortLevel || null, mode: s.permissions?.defaultMode || null };
}

function codexDefaults() {
  let toml = '';
  try { toml = fs.readFileSync(path.join(CODEX_HOME, 'config.toml'), 'utf8'); } catch { /* none */ }
  // Only top-level keys: stop at the first [table].
  const top = toml.split(/^\s*\[/m)[0];
  const get = (k) => top.match(new RegExp(`^\\s*${k}\\s*=\\s*"([^"]*)"`, 'm'))?.[1] || null;
  return { model: get('model'), effort: get('model_reasoning_effort'), mode: get('approval_policy') };
}

function findCodexFile(id) {
  if (!/^[0-9a-f-]{36}$/i.test(id)) return null;
  return listFiles(CODEX_SESSIONS, '.jsonl').find((f) => path.basename(f).includes(id)) || null;
}

ipcMain.handle('sessions:state', (_e, id, agent) => {
  const codex = agent === 'codex';
  const state = codex ? codexDefaults() : claudeDefaults();
  const file = codex ? findCodexFile(id) : findSessionFile(id);
  if (!file) return state;
  try {
    const tail = readTail(file, 1024 * 1024);
    if (codex) {
      state.model = lastMatch(tail, /"model":"([^"]+)"/g) || state.model;
      state.effort = lastMatch(tail, /"reasoning_effort":"([^"]+)"/g) || state.effort;
      state.mode = lastMatch(tail, /"approval_policy":"([^"]+)"/g) || state.mode;
    } else {
      state.model = lastMatch(tail, /"model":"(claude-[^"]+)"/g) || state.model;
      state.effort = lastMatch(tail, /"effort":"([a-z]+)"/g) || state.effort;
      state.mode = lastMatch(tail, /"permissionMode":"([A-Za-z]+)"/g) || state.mode;
    }
  } catch { /* unreadable: keep defaults */ }
  return state;
});

// ---------- session rename / delete ----------

function findSessionFile(id) {
  if (!/^[0-9a-f-]{36}$/i.test(id)) return null;
  let dirs = [];
  try { dirs = fs.readdirSync(CLAUDE_DIR, { withFileTypes: true }).filter((d) => d.isDirectory()); } catch { return null; }
  for (const d of dirs) {
    const f = path.join(CLAUDE_DIR, d.name, `${id}.jsonl`);
    if (fs.existsSync(f)) return f;
  }
  return null;
}

// Claude: the same record Claude Code's /rename writes. Codex: the same entry Codex appends to
// its name index. Either way the agent's own resume picker shows the new name too.
ipcMain.handle('sessions:rename', (_e, id, title, agent) => {
  const t = String(title || '').trim();
  if (!t || !/^[0-9a-f-]{36}$/i.test(id)) return false;
  if (agent === 'codex') {
    fs.appendFileSync(CODEX_INDEX, `${JSON.stringify({ id, thread_name: t, updated_at: new Date().toISOString() })}\n`);
    return true;
  }
  const file = findSessionFile(id);
  if (!file) return false;
  fs.appendFileSync(file, `${JSON.stringify({ type: 'custom-title', customTitle: t, sessionId: id })}\n`);
  return true;
});

function runCommand(argv) {
  return new Promise((resolve) => {
    const [file, args] = shellCommand(argv);
    let output = '';
    const child = spawn(file, args, { env: cleanEnv(), windowsHide: true, windowsVerbatimArguments: process.platform === 'win32', stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout.on('data', (d) => { output += d; });
    child.stderr.on('data', (d) => { output += d; });
    child.on('error', (e) => resolve({ code: -1, output: e.message }));
    child.on('close', (code) => resolve({ code, output }));
  });
}

function stopSession(id) {
  // Stop a running agent first so it doesn't recreate the file.
  let stopped = false;
  for (const [termId, p] of ptys) {
    if (p.sessionId === id) { try { p.proc.kill(); } catch { /* gone */ } ptys.delete(termId); stopped = true; }
  }
  return stopped;
}

ipcMain.handle('sessions:delete', async (_e, id, title, agent) => {
  const codex = agent === 'codex';
  if (!/^[0-9a-f-]{36}$/i.test(id)) return false;
  const file = codex ? null : findSessionFile(id);
  if (!codex && !file) return false;
  const r = await dialog.showMessageBox(win, {
    type: 'warning',
    buttons: ['Delete', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
    title: 'Delete session',
    message: `Delete "${title || id}"?`,
    detail: codex
      ? 'The Codex session is permanently deleted with `codex delete`. This cannot be undone. If the session is running, it will be stopped.'
      : 'The session file is moved to the Recycle Bin. If the session is running, it will be stopped.',
  });
  if (r.response !== 0) return false;
  const wasRunning = stopSession(id);
  if (codex) {
    // A Codex that was just stopped can hold the session for a moment while it exits.
    let res;
    for (let attempt = 0; attempt < 4; attempt++) {
      if (wasRunning || attempt > 0) await new Promise((r) => setTimeout(r, 1000));
      res = await runCommand([config.codexCommand || 'codex', 'delete', '--force', id]);
      if (res.code === 0) break;
    }
    if (res.code !== 0) {
      dialog.showMessageBox(win, { type: 'error', title: 'Delete failed', message: 'Codex could not delete the session.', detail: res.output.trim().slice(-1500) });
      return false;
    }
    return true;
  }
  await shell.trashItem(file);
  const sidecar = file.replace(/\.jsonl$/, ''); // subagent / tool-result folder for the session
  if (fs.existsSync(sidecar)) await shell.trashItem(sidecar).catch(() => {});
  metaCache.delete(file);
  return true;
});

// ---------- plan usage ----------

// Claude Code gives its status line the plan's rate limits (5-hour and weekly windows). The
// sessions Crew starts get a status line (statusline.js, via --settings) that saves them to
// usage-claude.json and then runs the user's own status line. Codex logs its limits itself.
function usagePaths() {
  const dir = app.getPath('userData');
  return {
    file: path.join(dir, 'usage-claude.json'),
    script: path.join(dir, 'crew-statusline.js'),
    cmd: path.join(dir, 'crew-statusline.cmd'),
    settings: path.join(dir, 'crew-claude-settings.json'),
  };
}

let usageHook = null; // true once the hook files are written
function usageHookReady() {
  if (usageHook !== null) return usageHook;
  try {
    const p = usagePaths();
    // Copied out of the app archive: Claude Code runs it with Crew's own executable in Node mode.
    fs.writeFileSync(p.script, fs.readFileSync(path.join(__dirname, 'statusline.js')));
    const fwd = (s) => s.replace(/\\/g, '/');
    let command;
    if (process.platform === 'win32') {
      fs.writeFileSync(p.cmd, `@echo off\r\nset ELECTRON_RUN_AS_NODE=1\r\n"${process.execPath}" "${p.script}" "${p.file}"\r\n`);
      command = `"${fwd(p.cmd)}"`;
    } else {
      command = `ELECTRON_RUN_AS_NODE=1 "${process.execPath}" "${p.script}" "${p.file}"`;
    }
    // Keep the user's padding so their status line sits where it always has.
    const own = readJson(path.join(os.homedir(), '.claude', 'settings.json'))?.statusLine || {};
    const statusLine = { type: 'command', command, ...(own.padding != null ? { padding: own.padding } : {}) };
    fs.writeFileSync(p.settings, JSON.stringify({ statusLine }, null, 2));
    usageHook = true;
  } catch {
    usageHook = false;
  }
  return usageHook;
}

// Windows are { used: percent, resetsAt: epoch ms, minutes: window length }.
function claudeUsage() {
  const j = readJson(usagePaths().file);
  if (!j?.rate_limits) return null;
  const win = (w, minutes) => (w && w.used_percentage != null ? { used: w.used_percentage, resetsAt: w.resets_at * 1000, minutes } : null);
  const windows = [win(j.rate_limits.five_hour, 300), win(j.rate_limits.seven_day, 10080)].filter(Boolean);
  return windows.length ? { at: j.at, windows } : null;
}

// The newest `rate_limits` Codex wrote into a session log (with each token count event).
function codexUsage() {
  const files = [];
  const listDir = (d) => { try { return fs.readdirSync(d).sort().reverse(); } catch { return []; } };
  for (const y of listDir(CODEX_SESSIONS)) {
    for (const m of listDir(path.join(CODEX_SESSIONS, y))) {
      for (const d of listDir(path.join(CODEX_SESSIONS, y, m))) {
        const dir = path.join(CODEX_SESSIONS, y, m, d);
        for (const f of listDir(dir)) if (f.endsWith('.jsonl')) files.push(path.join(dir, f));
        if (files.length >= 40) break;
      }
      if (files.length >= 40) break;
    }
    if (files.length >= 40) break;
  }
  const byMtime = files.map((f) => { try { return [f, fs.statSync(f).mtimeMs]; } catch { return [f, 0]; } }).sort((a, b) => b[1] - a[1]);
  for (const [f] of byMtime.slice(0, 15)) {
    let text;
    try {
      const fd = fs.openSync(f, 'r');
      const size = fs.fstatSync(fd).size;
      const len = Math.min(size, 1024 * 1024);
      const buf = Buffer.alloc(len);
      fs.readSync(fd, buf, 0, len, size - len);
      fs.closeSync(fd);
      text = buf.toString('utf8');
    } catch { continue; }
    const lines = text.split('\n');
    for (let i = lines.length - 1; i >= 0; i--) {
      if (!lines[i].includes('"rate_limits":{')) continue;
      let j;
      try { j = JSON.parse(lines[i]); } catch { continue; }
      const rl = j.payload?.rate_limits || j.payload?.info?.rate_limits;
      const win = (w) => (w && w.used_percent != null ? { used: w.used_percent, resetsAt: w.resets_at ? w.resets_at * 1000 : null, minutes: w.window_minutes } : null);
      const windows = [win(rl?.primary), win(rl?.secondary)].filter(Boolean);
      if (windows.length) return { at: Date.parse(j.timestamp) || 0, plan: rl.plan_type || null, windows };
    }
  }
  return null;
}

// Codex's current limits, asked of Codex itself: its app-server answers account/rateLimits/read
// by fetching them from OpenAI with Codex's own sign-in.
function codexLiveUsage() {
  return new Promise((resolve) => {
    const [file, args] = shellCommand([config.codexCommand || 'codex', 'app-server']);
    let child;
    try {
      child = spawn(file, args, { env: cleanEnv(), windowsHide: true, windowsVerbatimArguments: process.platform === 'win32', stdio: ['pipe', 'pipe', 'ignore'] });
    } catch { resolve(null); return; }
    let buf = '';
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      try { child.kill(); } catch { /* gone */ }
      resolve(v);
    };
    const timer = setTimeout(() => finish(null), 20000);
    const send = (m) => { try { child.stdin.write(`${JSON.stringify(m)}\n`); } catch { /* exited */ } };
    child.on('error', () => finish(null));
    child.on('close', () => finish(null));
    child.stdout.on('data', (d) => {
      buf += d;
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        let m;
        try { m = JSON.parse(line); } catch { continue; }
        if (m.id === 1) {
          send({ method: 'initialized' });
          send({ id: 2, method: 'account/rateLimits/read' });
        } else if (m.id === 2) {
          const rl = m.result?.rateLimits;
          const win = (w) => (w && w.usedPercent != null ? { used: w.usedPercent, resetsAt: w.resetsAt ? w.resetsAt * 1000 : null, minutes: w.windowDurationMins } : null);
          const windows = [win(rl?.primary), win(rl?.secondary)].filter(Boolean);
          finish(windows.length ? { at: Date.now(), plan: rl.planType || null, windows } : null);
        }
      }
    });
    send({ id: 1, method: 'initialize', params: { clientInfo: { name: 'crew', title: 'Crew', version: app.getVersion() }, capabilities: { experimentalApi: true } } });
  });
}

// Claude Code has no command that prints plan usage, so this opens it in a hidden terminal, runs
// /usage and reads the "Current session" and "Current week" bars off the screen. A headless
// xterm renders the output (redraws update only changed characters). The marker variable keeps
// Claude from saving a transcript, so the check never shows up as a session.
const MONTHS = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'];

// "7:50pm", "Nov 2, 7pm" or "Nov 2 at 7:30am", in local time.
function parseResetTime(text, now = Date.now()) {
  const m = /(?:([A-Za-z]{3})[a-z]*\s+(\d{1,2}),?\s+(?:at\s+)?)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)/i.exec(text || '');
  if (!m) return null;
  const d = new Date(now);
  d.setSeconds(0, 0);
  d.setHours((+m[3] % 12) + (m[5].toLowerCase() === 'pm' ? 12 : 0), +(m[4] || 0));
  if (m[1]) {
    const month = MONTHS.indexOf(m[1].toLowerCase());
    if (month < 0) return null;
    d.setMonth(month, +m[2]);
    if (d.getTime() < now - 86400e3) d.setFullYear(d.getFullYear() + 1);
  } else if (d.getTime() <= now) d.setDate(d.getDate() + 1);
  return d.getTime();
}

function parseClaudeUsageScreen(text) {
  const bar = (label) => {
    const m = new RegExp(`${label}[^\\n]*\\n?[^%]*?(\\d+(?:\\.\\d+)?)%\\s*used[\\s\\S]*?Resets\\s+([^\\n(]+)`, 'i').exec(text);
    return m ? { used: +m[1], resetsAt: parseResetTime(m[2]) } : null;
  };
  const session = bar('Current session');
  const week = bar('Current week \\(all models\\)') || bar('Current week');
  const windows = [session && { ...session, minutes: 300 }, week && { ...week, minutes: 10080 }].filter(Boolean);
  if (!windows.length) return null;
  const plan = /Claude (Pro|Max|Team|Enterprise)\b/.exec(text)?.[1] || null;
  return { at: Date.now(), plan, windows };
}

function claudeProbeUsage() {
  return new Promise((resolve) => {
    const { Terminal } = require('@xterm/headless');
    const cols = 120;
    const rows = 50;
    const term = new Terminal({ cols, rows, allowProposedApi: true });
    const cwd = path.join(app.getPath('userData'), 'usage-probe');
    try { fs.mkdirSync(cwd, { recursive: true }); } catch { /* the spawn below fails instead */ }
    const [file, args] = shellCommand([splitArgs(config.claudeCommand || 'claude')[0] || 'claude']);
    let proc;
    try {
      proc = pty.spawn(file, process.platform === 'win32' ? args.join(' ') : args, {
        name: 'xterm-256color', cols, rows, cwd, useConpty: true,
        env: { ...cleanEnv(), CLAUDE_CODE_CHILD_SESSION: '1' },
      });
    } catch { resolve(null); return; }
    const screen = () => { // what's on screen now, not the scrollback
      const b = term.buffer.active;
      const out = [];
      for (let i = b.baseY; i < b.baseY + rows; i++) out.push(b.getLine(i)?.translateToString(true) || '');
      return out.join('\n');
    };
    const everything = () => { // with the scrollback, where the header (and the plan name) is
      const b = term.buffer.active;
      const out = [];
      for (let i = 0; i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) || '');
      return out.join('\n');
    };
    let lastKey = 0;
    let step = 'start';
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      clearInterval(tick);
      clearTimeout(timer);
      try { proc.write('\x1b'); } catch { /* exited */ }
      setTimeout(() => { try { proc.kill(); } catch { /* gone */ } term.dispose(); }, 300);
      resolve(v);
    };
    const timer = setTimeout(() => finish(null), 35000);
    proc.onData((d) => term.write(d));
    proc.onExit(() => finish(null));
    let readyAt = 0;
    const tick = setInterval(() => {
      const text = screen();
      const trustPrompt = /trust this folder|Quick safety check/i.test(text);
      if (trustPrompt) {
        // Only ever Crew's own empty folder. The prompt starts on "No, exit": move to "Yes" first.
        if (Date.now() - lastKey < 600) return;
        lastKey = Date.now();
        const selected = /❯\s*([^\n]*)/.exec(text)?.[1] || '';
        if (/^\s*(\d\.\s*)?Yes/i.test(selected)) { proc.write('\r'); step = 'start'; readyAt = 0; } else proc.write('\x1b[B');
      } else if (step === 'start' && /Claude Code v\d/.test(text) && /❯/.test(text)) {
        readyAt = readyAt || Date.now();
        if (Date.now() - readyAt > 1500) { proc.write('/usage'); step = 'typed'; readyAt = Date.now(); }
      } else if (step === 'typed' && Date.now() - readyAt > 700) {
        proc.write('\r');
        step = 'sent';
      } else if (step === 'sent') {
        // Both bars, or just one once the screen has had time to settle (plans without a weekly bar).
        const u = parseClaudeUsageScreen(text.slice(Math.max(0, text.lastIndexOf('Current session'))));
        if (u && (u.windows.length >= 2 || Date.now() - readyAt > 8000)) finish({ ...u, plan: u.plan || /Claude (Pro|Max|Team|Enterprise)\b/.exec(everything())?.[1] || null });
      }
    }, 400);
  });
}

// Live numbers are cached and refreshed in the background; the status line hook and Codex's
// logs fill in when a live check fails, and whichever reading is newest wins.
const usageCache = { claude: null, codex: null };
const usageRefreshing = {};
const usageChecked = {};
const USAGE_EVERY = { claude: 5 * 60e3, codex: 2 * 60e3 };
const newest = (...xs) => xs.filter(Boolean).sort((a, b) => b.at - a.at)[0] || null;

async function refreshUsage(agent, force) {
  if (usageRefreshing[agent]) return usageRefreshing[agent];
  if (!force && Date.now() - (usageChecked[agent] || 0) < USAGE_EVERY[agent]) return null;
  usageChecked[agent] = Date.now();
  usageRefreshing[agent] = (async () => {
    try {
      const live = agent === 'claude' ? await claudeProbeUsage() : await codexLiveUsage();
      if (live) usageCache[agent] = live;
    } catch { /* keep what we had */ }
    usageRefreshing[agent] = null;
    if (win && !win.isDestroyed()) win.webContents.send('usage:changed');
  })();
  return usageRefreshing[agent];
}

ipcMain.handle('usage:get', async (_e, force) => {
  if (config.showUsage === false) return {};
  const pending = [refreshUsage('codex', force), refreshUsage('claude', force)];
  if (force) await Promise.all(pending);
  return {
    claude: newest(usageCache.claude, claudeUsage()),
    codex: newest(usageCache.codex, codexUsage()),
  };
});

// ---------- git ----------

// The Git view runs the real git CLI in the repository. Output is plain text (no color, no
// pager, paths unquoted) and git never waits on a terminal prompt: credentials go through the
// configured credential helper (Git Credential Manager on Windows) or the command fails.
const GIT_FLAGS = ['-c', 'core.quotepath=false', '-c', 'color.ui=false', '-c', 'i18n.logOutputEncoding=UTF-8'];

function runGit(cwd, args, input) {
  return new Promise((resolve) => {
    // GIT_EDITOR=true accepts default messages (merges, reverts) instead of waiting on an editor.
    const env = { ...cleanEnv(), GIT_TERMINAL_PROMPT: '0', GIT_PAGER: '', GIT_EDITOR: 'true', LC_ALL: 'C.UTF-8' };
    delete env.FORCE_COLOR;
    let child;
    try {
      child = spawn('git', [...GIT_FLAGS, ...args], { cwd, env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
    } catch (e) {
      resolve({ code: -1, stdout: '', stderr: e.message });
      return;
    }
    const out = [];
    const err = [];
    child.stdout.on('data', (d) => out.push(d));
    child.stderr.on('data', (d) => err.push(d));
    child.on('error', (e) => resolve({ code: -1, stdout: '', stderr: e.code === 'ENOENT' ? 'Git is not installed or not on PATH.' : e.message }));
    child.on('close', (code) => resolve({ code, stdout: Buffer.concat(out).toString('utf8'), stderr: Buffer.concat(err).toString('utf8') }));
    child.stdin.on('error', () => { /* git exited before reading stdin */ });
    child.stdin.end(input ?? undefined);
  });
}

ipcMain.handle('git:root', async (_e, cwd) => {
  if (!cwd || !fs.existsSync(cwd)) return null;
  const r = await runGit(cwd, ['rev-parse', '--show-toplevel']);
  return r.code === 0 ? path.normalize(r.stdout.trim()) : null;
});

ipcMain.handle('git:run', (_e, cwd, args, input) => {
  if (!cwd || !fs.existsSync(cwd) || !Array.isArray(args)) return { code: -1, stdout: '', stderr: 'Repository folder not found.' };
  return runGit(cwd, args.map(String), input);
});

// Appends a pattern to the repository's .gitignore, starting it on a new line.
ipcMain.handle('git:ignore', (_e, root, pattern) => {
  try {
    const file = path.join(root, '.gitignore');
    const cur = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : '';
    if (cur.split(/\r?\n/).includes(pattern)) return true;
    const eol = cur.includes('\r\n') ? '\r\n' : '\n';
    fs.appendFileSync(file, `${cur && !cur.endsWith('\n') ? eol : ''}${pattern}${eol}`);
    return true;
  } catch {
    return false;
  }
});

ipcMain.handle('config:get', () => config);
ipcMain.handle('config:set', (_e, patch) => {
  config = { ...config, ...patch };
  saveConfig(config);
  if (patch.theme && win) win.setTitleBarOverlay?.(overlayFor(config.theme));
  return config;
});

// ---------- window ----------

function overlayFor(theme) {
  return theme === 'dark'
    ? { color: '#1f1f1f', symbolColor: '#d6d6d6', height: 36 }
    : { color: '#f9f8f6', symbolColor: '#3b3b3b', height: 36 };
}

function createWindow() {
  nativeTheme.themeSource = 'light';
  win = new BrowserWindow({
    width: 1400,
    height: 900,
    minWidth: 800,
    minHeight: 500,
    title: 'Crew',
    icon: path.join(__dirname, 'build', 'icon.png'),
    backgroundColor: config.theme === 'dark' ? '#1f1f1f' : '#f9f8f6',
    titleBarStyle: 'hidden',
    titleBarOverlay: overlayFor(config.theme),
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      webviewTag: true,
    },
  });
  // The side browser loads arbitrary sites: no preload, no Node, sandboxed.
  win.webContents.on('will-attach-webview', (_e, prefs, params) => {
    delete prefs.preload;
    prefs.nodeIntegration = false;
    prefs.contextIsolation = true;
    prefs.sandbox = true;
    params.partition = 'persist:browser';
  });
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  win.on('close', (e) => {
    if (quitting || !config.closeToTray) return;
    e.preventDefault();
    win.hide();
    if (!config.trayHintShown && tray) {
      tray.displayBalloon({ iconType: 'info', title: 'Crew is still running', content: 'Your sessions keep running. Click the tray icon to reopen, or right-click it to quit.' });
      config = { ...config, trayHintShown: true };
      saveConfig(config);
    }
  });
  win.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url);
    return { action: 'deny' };
  });
}

// Pop-ups and target=_blank links from the side browser open as new tabs in the panel.
app.on('web-contents-created', (_e, contents) => {
  if (contents.getType() !== 'webview') return;
  contents.setWindowOpenHandler(({ url }) => {
    if (/^https?:/i.test(url) && win && !win.isDestroyed()) win.webContents.send('browser:new-tab', url);
    return { action: 'deny' };
  });
  // Pages get keyboard input first, so pass the tab shortcuts up to the panel.
  contents.on('before-input-event', (event, input) => {
    if (input.type !== 'keyDown' || !input.control || input.alt) return;
    const key = input.key.length === 1 ? input.key.toLowerCase() : input.key;
    if (!['t', 'w', 'l', 'Tab'].includes(key)) return;
    event.preventDefault();
    if (win && !win.isDestroyed()) win.webContents.send('browser:shortcut', { key, shift: input.shift });
  });
});

function showWindow() {
  if (!win || win.isDestroyed()) return createWindow();
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

function createTray() {
  tray = new Tray(path.join(__dirname, 'build', 'icon.ico'));
  tray.setToolTip('Crew');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Open Crew', click: showWindow },
    { type: 'separator' },
    { label: 'Quit Crew', click: () => { quitting = true; app.quit(); } },
  ]));
  tray.on('click', showWindow);
}

// node-pty's kill() on Windows closes the pseudo console synchronously, and that waits for output
// that only this (now blocked) main thread drains, so quitting with sessions open could hang with
// no window and no tray. On quit, end each session's process tree with taskkill instead.
function killAllPtys() {
  for (const { proc } of ptys.values()) {
    try {
      if (process.platform === 'win32') spawnSync('taskkill', ['/T', '/F', '/PID', String(proc.pid)], { windowsHide: true, timeout: 3000 });
      else proc.kill();
    } catch { /* already gone */ }
  }
  ptys.clear();
}

app.on('before-quit', () => {
  if (quitting === 'done') return;
  quitting = 'done';
  killAllPtys();
  ssh.shutdown();
  // Last resort: if anything still holds the process, leave anyway.
  setTimeout(() => app.exit(0), 3000).unref();
});

app.whenReady().then(() => {
  createWindow();
  ssh.init(() => win);
  createTray();
  startWatcher();
  cleanAttachments();
});

app.on('window-all-closed', () => {
  killAllPtys();
  app.quit();
});
