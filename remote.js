// Phone access: the Crew app on a phone (github CrewApp) connects here to see and drive the
// sessions open in Crew, over a WebSocket. Off until the user turns it on in Settings; a phone
// pairs by scanning a QR code that carries the address and a random token, and every request must
// carry that token. On the same network it's plain WebSocket inside the LAN; from elsewhere use a
// private network like Tailscale (its 100.x address is offered too), never an open port.
//
// What a phone gets: the open terminals (agents and host shells) with their recent output and live
// updates, typing into them, opening sessions in Crew, crew-ssh's approval prompts, and the
// @-agents' runs.

const { ipcMain } = require('electron');
const http = require('http');
const os = require('os');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const transcript = require('./transcript');

const BUFFER = 300 * 1024; // recent output kept per terminal, replayed when a phone opens it
const PROTOCOL = 2; // 2: sessions as chats (chat / msgs / screen)
const QUIET = 3000; // a terminal with no output for this long counts as idle (its agent finished or is waiting)
const UPLOAD_MAX = 20 * 1024 * 1024;

// The phone app's web version (a copy of CrewApp's www), served so any phone browser can use Crew
// without installing the app. It's only the page; everything else still needs the pairing token.
const PHONE_DIR = path.join(__dirname, 'phone');
const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.webmanifest': 'application/manifest+json' };

function servePhone(req, res) {
  const rel = decodeURIComponent(new URL(req.url, 'http://x').pathname).replace(/^\/+/, '') || 'index.html';
  const file = path.join(PHONE_DIR, rel);
  const type = TYPES[path.extname(file)];
  if (!type || !file.startsWith(PHONE_DIR + path.sep) || !fs.existsSync(file)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': type, 'cache-control': 'no-cache' });
  fs.createReadStream(file).pipe(res);
}

let deps = null; // { getWin, ptys, scanProjects, getConfig, setConfig, sshAnswer, stopRun, attachDir }
let server = null;
let wss = null;
const clients = new Set(); // { ws, device, attached: Set<termId> }
const buffers = new Map(); // termId -> recent output
const prompts = new Map(); // id -> prompt, crew-ssh's open questions
const runs = new Map(); // id -> run
const activity = new Map(); // termId -> { busy, since, last, timer }

const cfg = () => deps.getConfig();
const port = () => cfg().remotePort || 7788;
function token() {
  if (!cfg().remoteToken) deps.setConfig({ remoteToken: crypto.randomBytes(18).toString('base64url') });
  return cfg().remoteToken;
}
const sameToken = (t) => {
  const a = Buffer.from(String(t || ''));
  const b = Buffer.from(token());
  return a.length === b.length && crypto.timingSafeEqual(a, b);
};

// The computer's addresses a phone can reach: the LAN ones, and Tailscale's (100.64.0.0/10).
function addresses() {
  const out = [];
  for (const [name, list] of Object.entries(os.networkInterfaces())) {
    for (const a of list || []) {
      if (a.family !== 'IPv4' || a.internal) continue;
      const tailscale = /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(a.address);
      if (!tailscale && /^169\.254\./.test(a.address)) continue;
      out.push({ address: a.address, kind: tailscale ? 'tailscale' : 'lan', name });
    }
  }
  return out.sort((a, b) => (a.kind === b.kind ? 0 : a.kind === 'lan' ? -1 : 1));
}

// ---------- what a phone sees ----------

function send(c, msg) { if (c.ws.readyState === 1) c.ws.send(JSON.stringify(msg)); }
function broadcast(msg, only) { for (const c of clients) if (c.ready && (!only || only(c))) send(c, msg); }

// The open terminals, with titles from the session lists.
function terminals() {
  const titles = new Map();
  try { for (const p of deps.scanProjects()) for (const s of p.sessions) titles.set(s.id, { title: s.title, project: p.name }); } catch { /* none */ }
  const list = [];
  for (const [termId, p] of deps.ptys) {
    if (p.installing) continue;
    const t = titles.get(p.sessionId);
    const host = p.agent === 'ssh' ? deps.hostName?.(p.sessionId.slice(4)) : null;
    list.push({
      termId, sessionId: p.sessionId, agent: p.agent, cwd: p.cwd || '', cols: p.cols || 120, rows: p.rows || 30, busy: !!activity.get(termId)?.busy,
      title: host ? `${host} · terminal` : t?.title || 'New session', project: host || t?.project || '',
    });
  }
  return list;
}

const normPath = (x) => String(x || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
const briefSession = (s) => ({ id: s.id, title: s.title, agent: s.agent, provider: s.provider || null, mtime: s.mtime });
const list = (fn) => { try { return fn?.() || []; } catch { return []; } };

// As Crew's sidebar has them: the projects; the hosts, each with the agent sessions of its
// workspace (which isn't listed as a project); and the @-agents.
function sidebar() {
  let scanned = [];
  try { scanned = deps.scanProjects(); } catch { /* none */ }
  const hosts = list(deps.hosts);
  const byPath = new Map(scanned.map((p) => [normPath(p.path), p]));
  const workspaces = new Set(hosts.map((h) => normPath(h.workspace)));
  return {
    projects: scanned.filter((p) => !workspaces.has(normPath(p.path))).slice(0, 60)
      .map((p) => ({ path: p.path, name: p.name, sessions: p.sessions.slice(0, 40).map(briefSession) })),
    hosts: hosts.map((h) => ({ ...h, sessions: (byPath.get(normPath(h.workspace))?.sessions || []).slice(0, 20).map(briefSession) })),
    agents: list(deps.agents),
    providers: list(deps.providers),
  };
}

const sendState = (c) => send(c, { t: 'state', terminals: terminals(), ...sidebar(), prompts: [...prompts.values()], runs: [...runs.values()] });
let stateTimer = null;
function stateChanged() { clearTimeout(stateTimer); stateTimer = setTimeout(() => { for (const c of clients) if (c.ready) sendState(c); }, 400); }

// ---------- hooks from the rest of Crew ----------

function onData(termId, data) {
  let b = (buffers.get(termId) || '') + data;
  if (b.length > BUFFER) b = b.slice(b.length - BUFFER);
  buffers.set(termId, b);
  if (clients.size) {
    pendingOut.set(termId, (pendingOut.get(termId) || '') + data);
    if (!outTimer) outTimer = setTimeout(flushOut, OUT_EVERY);
  }
  screenOf(termId)?.write(data, () => screenChanged(termId));
  noteActivity(termId);
}

// An agent redraws in many small pieces (its spinner alone, several times a second): phones get
// each terminal's output in one message per frame instead, which keeps them smooth.
const OUT_EVERY = 16;
const pendingOut = new Map(); // termId -> output not sent yet
let outTimer = null;
function flushOut() {
  clearTimeout(outTimer);
  outTimer = null;
  for (const [termId, data] of pendingOut) broadcast({ t: 'out', termId, data }, (c) => c.attached.has(termId));
  pendingOut.clear();
}

// Working or idle, from the output: an agent at work keeps redrawing its spinner and output, and
// goes quiet when it's done or waiting for you. A phone shows it and says when one finished.
function noteActivity(termId) {
  const now = Date.now();
  let a = activity.get(termId);
  if (!a) { a = { busy: false, since: 0, last: 0, timer: null }; activity.set(termId, a); }
  a.last = now;
  if (!a.busy) { a.busy = true; a.since = now; broadcast({ t: 'activity', termId, busy: true }); }
  if (!a.timer) a.timer = setTimeout(function check() {
    const quiet = Date.now() - a.last;
    if (quiet < QUIET) { a.timer = setTimeout(check, QUIET - quiet); return; }
    a.timer = null;
    a.busy = false;
    broadcast({ t: 'activity', termId, busy: false, ran: a.last - a.since });
  }, QUIET);
}

// A file from the phone (a photo, a document), saved with Crew's pasted images so its path can go
// into a message for the agent.
function saveUpload(name, base64) {
  const bytes = Buffer.from(String(base64 || ''), 'base64');
  if (!bytes.length) throw new Error('The file is empty.');
  if (bytes.length > UPLOAD_MAX) throw new Error('Files up to 20 MB can be sent.');
  const safe = path.basename(String(name || 'file')).replace(/[^\w.\-一-鿿]+/g, '_').slice(-80) || 'file';
  const stamp = new Date().toISOString().replace(/[-:]/g, '').replace('T', '-').slice(0, 15);
  fs.mkdirSync(deps.attachDir, { recursive: true });
  const file = path.join(deps.attachDir, `phone-${stamp}-${crypto.randomBytes(3).toString('hex')}-${safe}`);
  fs.writeFileSync(file, bytes);
  return file;
}
function onStart() { stateChanged(); }
function onExit(termId, code) {
  buffers.delete(termId);
  clearTimeout(activity.get(termId)?.timer);
  activity.delete(termId);
  screens.get(termId)?.dispose();
  screens.delete(termId);
  broadcast({ t: 'exit', termId, code }, (c) => c.attached.has(termId));
  stateChanged();
}
function onPrompt(p) { prompts.set(p.id, p); broadcast({ t: 'prompt', prompt: p }); }
function onPromptDone(id) { if (prompts.delete(id)) broadcast({ t: 'prompt-done', id }); }
function onRun(r) {
  runs.set(r.id, r);
  if (runs.size > 30) runs.delete(runs.keys().next().value);
  broadcast({ t: 'run', run: r });
}

// ---------- a session as a chat (the Android app) ----------

// The app shows a session as a conversation, read from the agent's transcript file
// (transcript.js), instead of its terminal. What only the terminal shows comes from a copy of
// each agent terminal's screen kept here: the working line ("✻ Thinking… (12s)") and a question
// the agent is asking with numbered choices (a permission prompt, a picker).
const screens = new Map(); // termId -> headless terminal
function screenOf(termId) {
  const p = deps.ptys.get(termId);
  if (!p) return null;
  const cols = p.cols || 120;
  const rows = p.rows || 30;
  let s = screens.get(termId);
  if (!s) {
    const { Terminal: Headless } = require('@xterm/headless'); // loaded with the first terminal
    s = new Headless({ cols, rows, scrollback: 300, allowProposedApi: true });
    screens.set(termId, s);
  }
  if (s.cols !== cols || s.rows !== rows) s.resize(cols, rows);
  return s;
}
// The screen and the history just above it (the last lines are the screen itself).
function screenLines(termId, history = 200) {
  const s = screens.get(termId);
  if (!s) return [];
  const b = s.buffer.active;
  const out = [];
  for (let i = Math.max(0, b.length - s.rows - history); i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) || '');
  return out;
}

// What the agent is writing or running right now, as its terminal shows it: from its last bullet
// (Claude's ⏺, Codex's •) down to the working line. The transcript has it only once it's done.
function liveBlock(lines) {
  let end = -1;
  for (let i = lines.length - 1; i >= 0; i--) if (/esc to (interrupt|cancel)/i.test(lines[i])) { end = i; break; }
  if (end < 0) return '';
  let start = -1;
  for (let i = end - 1; i >= Math.max(0, end - 250); i--) if (/^\s*[⏺●•]\s/.test(lines[i])) { start = i; break; }
  if (start < 0) return '';
  const block = lines.slice(start, end);
  while (block.length && !block[block.length - 1].trim()) block.pop();
  const indent = block[0].search(/[⏺●•]/) + 2; // text under the bullet is indented to its text
  return block.map((l, i) => (i === 0 ? l.slice(indent) : l.slice(Math.min(indent, l.search(/\S|$/))))).join('\n').replace(/\s+$/, '').slice(0, 6000);
}

// "✻ Churning… (12s · ↓ 1.2k tokens · esc to interrupt)" -> "Churning… (12s · ↓ 1.2k tokens)"
function workingLine(lines) {
  const l = [...lines].reverse().find((x) => /esc to (interrupt|cancel)/i.test(x));
  if (!l) return '';
  return l.replace(/\s*[·•]?\s*esc to (interrupt|cancel)/i, '').replace(/\(\s*\)/, '').replace(/^[\s✻✶✳✢·•*+⏺●◦]+/, '').trim();
}

// A choice the agent is waiting on: numbered rows near the bottom, one of them highlighted
// (❯ or ›), under a question. Numbered lists in replies have no highlight, so they don't count.
function askOf(lines) {
  const rows = [];
  let marked = false;
  let firstAt = -1;
  for (let i = Math.max(0, lines.length - 24); i < lines.length; i++) {
    const m = lines[i].match(/^[\s│|]*([❯›>])?\s*(\d+)[.)]\s+(.+?)[\s│|]*$/);
    if (!m) continue;
    if (!rows.length) firstAt = i;
    if (m[1]) marked = true;
    rows.push({ n: Number(m[2]), label: m[3].replace(/\s{2,}.*$/, '').trim() });
  }
  if (!marked || rows.length < 2 || rows[0].n !== 1) return null;
  let question = '';
  for (let i = firstAt - 1; i >= 0 && i >= firstAt - 8; i--) {
    const t = lines[i].replace(/[│╭╮╰╯─┌┐└┘|]/g, '').trim();
    if (t) { question = t; break; }
  }
  return { question, options: rows.slice(0, 9) };
}

// What a phone following the terminal sees, sent when it changes.
function screenState(termId) {
  const all = screenLines(termId);
  const lines = all.slice(-(screens.get(termId)?.rows || 30));
  const busy = !!activity.get(termId)?.busy;
  return { working: busy ? workingLine(lines) : '', live: busy ? liveBlock(all) : '', ask: busy ? null : askOf(lines) };
}
function pushScreen(c) {
  if (!c.chatTerm) return;
  const s = JSON.stringify(screenState(c.chatTerm));
  if (s === c.lastScreen) return;
  c.lastScreen = s;
  send(c, { t: 'screen', termId: c.chatTerm, ...JSON.parse(s) });
}

// The chats a phone follows: the transcript, read at once, then what's added to it.
function followChat(c, sessionId, termId) {
  unfollowChat(c);
  c.chat = { sessionId, follower: null, triedAt: 0 };
  c.chatTerm = termId || null;
  c.lastScreen = '';
  findTranscript(c);
  const items = c.chat.follower ? safe(() => c.chat.follower.read(), []) : [];
  send(c, { t: 'msgs', sessionId, items: items.slice(-400), reset: true });
  pushScreen(c);
  ensureChatTimer();
}
function unfollowChat(c) { c.chat = null; c.chatTerm = null; }
const safe = (fn, fallback) => { try { return fn(); } catch { return fallback; } };
// A new session's file appears once its first message is written: look again every few seconds.
function findTranscript(c) {
  const ch = c.chat;
  if (!ch || ch.follower || Date.now() - ch.triedAt < 2500) return;
  ch.triedAt = Date.now();
  const where = safe(() => deps.sessionFile?.(ch.sessionId), null);
  if (where?.file) ch.follower = new transcript.Follower(where.file, where.agent);
}

let chatTimer = null;
function ensureChatTimer() {
  if (chatTimer) return;
  chatTimer = setInterval(() => {
    let any = false;
    for (const c of clients) {
      if (!c.chat) continue;
      any = true;
      if (!c.chat.follower) {
        findTranscript(c);
        if (c.chat.follower) { send(c, { t: 'msgs', sessionId: c.chat.sessionId, items: safe(() => c.chat.follower.read(), []).slice(-400), reset: true }); }
      } else {
        const items = safe(() => c.chat.follower.poll(), []);
        if (items.length) send(c, { t: 'msgs', sessionId: c.chat.sessionId, items });
      }
      pushScreen(c);
    }
    if (!any) { clearInterval(chatTimer); chatTimer = null; }
  }, 400);
}

// ---------- a host's terminal on the phone (the Android app) ----------

// The app shows a host's terminal as its screen: lines of text runs with their colors, sent as
// the screen changes, with the terminal sized to the phone while it watches (as the web version
// does it). A run is [text, color or null, bold].
const BASE16 = ['#000000', '#cd3131', '#0dbc79', '#e5e510', '#2472c8', '#bc3fbc', '#11a8cd', '#e5e5e5',
  '#666666', '#f14c4c', '#23d18b', '#f5f543', '#3b8eea', '#d670d6', '#29b8db', '#ffffff'];
function palette(i) {
  if (i < 16) return BASE16[i];
  if (i < 232) {
    const n = i - 16;
    const v = (k) => (k ? 55 + k * 40 : 0).toString(16).padStart(2, '0');
    return `#${v(Math.floor(n / 36))}${v(Math.floor(n / 6) % 6)}${v(n % 6)}`;
  }
  const g = (8 + (i - 232) * 10).toString(16).padStart(2, '0');
  return `#${g}${g}${g}`;
}
function cellColor(cell) {
  if (cell.isFgDefault()) return null;
  if (cell.isFgPalette()) return palette(cell.getFgColor());
  return `#${cell.getFgColor().toString(16).padStart(6, '0')}`;
}
function screenRuns(termId, history = 150) {
  const s = screens.get(termId);
  if (!s) return { lines: [], cursor: null };
  const b = s.buffer.active;
  const cell = b.getNullCell();
  const lines = [];
  const first = Math.max(0, b.length - s.rows - history);
  for (let y = first; y < b.length; y++) {
    const line = b.getLine(y);
    const runs = [];
    if (line) {
      let text = '';
      let color = null;
      let bold = false;
      for (let x = 0; x < s.cols; x++) {
        line.getCell(x, cell);
        if (cell.getWidth() === 0) continue;
        const c = cellColor(cell);
        const bd = !!cell.isBold();
        if (text && (c !== color || bd !== bold)) { runs.push([text, color, bold]); text = ''; }
        color = c;
        bold = bd;
        text += cell.getChars() || ' ';
      }
      if (text) runs.push([text, color, bold]);
      // Trailing blanks carry nothing.
      while (runs.length && !runs[runs.length - 1][0].trim()) runs.pop();
      if (runs.length) runs[runs.length - 1][0] = runs[runs.length - 1][0].replace(/\s+$/, '');
    }
    lines.push(runs);
  }
  // The cursor: its column, and its line within \`lines\`.
  return { lines, cursor: [b.cursorX, b.baseY + b.cursorY - first] };
}

const watchDirty = new Set(); // terminals whose screen changed since phones watching them last got it
let watchTimer = null;
function pushWatched() {
  watchTimer = null;
  for (const c of clients) if (c.watch && watchDirty.has(c.watch)) send(c, { t: 'screen-text', termId: c.watch, ...screenRuns(c.watch) });
  watchDirty.clear();
}
function screenChanged(termId) {
  if (![...clients].some((c) => c.watch === termId)) return;
  watchDirty.add(termId);
  if (!watchTimer) watchTimer = setTimeout(pushWatched, 120);
}
function watchTerm(c, termId, cols, rows) {
  unwatchTerm(c);
  if (!deps.ptys.get(termId)) { send(c, { t: 'exit', termId, code: null }); return; }
  c.watch = termId;
  if (cols && rows) fitToPhone(c, termId, cols, rows);
  screenOf(termId);
  // After a resize the shell redraws: send the screen at once, and again once it has.
  send(c, { t: 'screen-text', termId, ...screenRuns(termId) });
  setTimeout(() => { if (c.watch === termId) send(c, { t: 'screen-text', termId, ...screenRuns(termId) }); }, 400);
}
function unwatchTerm(c) {
  if (!c.watch) return;
  release(c, c.watch);
  c.watch = null;
}

// ---------- a terminal at the phone's size ----------

// While a phone shows a terminal, the terminal takes the phone's size, so the agent lays its
// screen out for the phone (no sideways scrolling); when the phone leaves, it goes back to the
// window's size. The latest phone to open it wins.
function fitToPhone(c, termId, cols, rows) {
  const p = deps.ptys.get(termId);
  if (!p) return false;
  cols = Math.max(20, Math.min(300, Math.round(+cols || 0)));
  rows = Math.max(8, Math.min(200, Math.round(+rows || 0)));
  if (p.deskCols == null) { p.deskCols = p.cols || 120; p.deskRows = p.rows || 30; }
  for (const other of clients) if (other !== c) other.sized.delete(termId);
  c.sized.add(termId);
  p.phoneSize = { cols, rows };
  if (p.cols === cols && p.rows === rows) return false;
  p.cols = cols;
  p.rows = rows;
  try { p.proc.resize(cols, rows); } catch { /* exited */ }
  return true;
}
function release(c, termId) {
  if (!c.sized.delete(termId)) return;
  const p = deps.ptys.get(termId);
  if (!p?.phoneSize) return;
  p.phoneSize = null;
  p.cols = p.deskCols || p.cols;
  p.rows = p.deskRows || p.rows;
  try { p.proc.resize(p.cols, p.rows); } catch { /* exited */ }
}

// ---------- the composer's chips ----------

// A session's model, effort and mode chips and their menus come from Crew's window, which builds
// them for its own composer; a pick made on the phone runs there too.
let askSeq = 0;
const asks = new Map();
function askWindow(req, timeout = 20000) {
  return new Promise((resolve) => {
    const w = deps.getWin();
    if (!w || w.isDestroyed()) { resolve(null); return; }
    const id = ++askSeq;
    asks.set(id, resolve);
    w.webContents.send('remote:ask', { id, ...req });
    setTimeout(() => { if (asks.delete(id)) resolve(null); }, timeout);
  });
}
ipcMain.on('remote:answer', (_e, id, data) => { const r = asks.get(id); asks.delete(id); r?.(data); });

async function sendChips(c, sessionId) {
  send(c, { t: 'chips', sessionId, chips: await askWindow({ what: 'chips', sessionId }, 5000) });
}

// ---------- requests from a phone ----------

function handle(c, m) {
  if (!c.ready) {
    if (m.t !== 'hello' || !sameToken(m.token)) { send(c, { t: 'denied', reason: 'This phone isn\'t paired with Crew, or the pairing code changed. Scan the QR code in Crew\'s Settings again.' }); c.ws.close(); return; }
    c.ready = true;
    c.device = String(m.device || 'Phone').slice(0, 60);
    send(c, { t: 'ready', protocol: PROTOCOL, computer: os.hostname() });
    sendState(c);
    notifyWindow();
    return;
  }
  switch (m.t) {
    case 'state': sendState(c); break;
    case 'attach': {
      const p = deps.ptys.get(m.termId);
      if (!p) { send(c, { t: 'exit', termId: m.termId, code: null }); break; }
      c.attached.add(m.termId);
      // Shown at the phone's size: the agent redraws for it, then the phone gets the screen.
      const resized = m.cols && m.rows && fitToPhone(c, m.termId, m.cols, m.rows);
      setTimeout(() => {
        if (!c.attached.has(m.termId)) return;
        flushOut(); // what's pending is in the snapshot: don't send it again after
        send(c, { t: 'snapshot', termId: m.termId, data: buffers.get(m.termId) || '', cols: p.cols || 120, rows: p.rows || 30 });
      }, resized ? 450 : 0);
      break;
    }
    case 'resize': if (c.attached.has(m.termId)) fitToPhone(c, m.termId, m.cols, m.rows); break;
    case 'detach': c.attached.delete(m.termId); release(c, m.termId); break;
    case 'input': deps.ptys.get(m.termId)?.proc.write(String(m.data || '')); break;
    // Opening a session goes through Crew's window, which shows it as usual.
    case 'open': {
      const w = deps.getWin();
      if (w && !w.isDestroyed()) w.webContents.send('remote:open', { sessionId: m.sessionId || null, projectPath: m.projectPath, agent: m.agent || 'claude', provider: m.provider || null, resume: !!m.resume });
      break;
    }
    // A host: its terminal beside an agent session, as clicking it in Crew's sidebar does.
    case 'open-host': {
      const w = deps.getWin();
      if (w && !w.isDestroyed()) w.webContents.send('remote:open', { hostId: m.hostId, sessionId: m.sessionId || null, terminalOnly: !!m.terminalOnly });
      break;
    }
    case 'chips': sendChips(c, m.sessionId); break;
    case 'chat': followChat(c, String(m.sessionId || ''), m.termId || null); break;
    case 'chat-term': if (c.chat) { c.chatTerm = m.termId || null; c.lastScreen = ''; pushScreen(c); } break;
    case 'unchat': unfollowChat(c); break;
    case 'watch': watchTerm(c, String(m.termId || ''), m.cols, m.rows); break;
    case 'unwatch': unwatchTerm(c); break;
    case 'chip-menu':
      askWindow({ what: 'menu', sessionId: m.sessionId, kind: m.kind }).then((menu) => send(c, { t: 'chip-menu', reqId: m.reqId, menu }));
      break;
    case 'chip-pick':
      askWindow({ what: 'pick', menuId: m.menuId, index: m.index }).then(() => {
        // The pick takes a moment in the agent (a picker, a command): show the new chips after.
        for (const ms of [800, 3000]) setTimeout(() => sendChips(c, m.sessionId), ms);
      });
      break;
    case 'answer': deps.sshAnswer(m.id, m.answer ?? null); onPromptDone(m.id); break;
    case 'stop-run': deps.stopRun(m.id); break;
    case 'upload':
      try { send(c, { t: 'uploaded', reqId: m.reqId, path: saveUpload(m.name, m.data) }); } catch (e) { send(c, { t: 'uploaded', reqId: m.reqId, error: e.message }); }
      break;
    default: break;
  }
}

// ---------- the server ----------

function start() {
  if (server) return;
  server = http.createServer((req, res) => {
    try { servePhone(req, res); } catch { res.writeHead(400); res.end(); }
  });
  const { WebSocketServer } = require('ws'); // phone access is off for most people
  wss = new WebSocketServer({ server, maxPayload: 30 * 1024 * 1024 });
  wss.on('connection', (ws) => {
    const c = { ws, device: '', ready: false, attached: new Set(), sized: new Set() };
    clients.add(c);
    const hello = setTimeout(() => { if (!c.ready) ws.close(); }, 10000);
    ws.on('message', (raw) => { let m; try { m = JSON.parse(String(raw)); } catch { return; } handle(c, m); });
    ws.on('close', () => { clearTimeout(hello); clients.delete(c); for (const id of [...c.sized]) release(c, id); notifyWindow(); });
    ws.on('error', () => { /* closes */ });
  });
  server.on('error', (e) => { status.error = e.code === 'EADDRINUSE' ? `Port ${port()} is in use. Choose another one.` : e.message; stop(); notifyWindow(); });
  status.error = '';
  server.listen(port(), '0.0.0.0', () => notifyWindow());
}

function stop() {
  for (const c of clients) { try { c.ws.close(); } catch { /* gone */ } }
  clients.clear();
  try { wss?.close(); } catch { /* closed */ }
  try { server?.close(); } catch { /* closed */ }
  wss = null;
  server = null;
}

const status = { error: '' };
async function info() {
  const enabled = !!cfg().remoteEnabled;
  const addrs = addresses();
  const pair = { v: 1, name: os.hostname(), port: port(), token: token(), hosts: addrs.map((a) => a.address) };
  return {
    enabled, running: !!server?.listening, port: port(), addresses: addrs, error: status.error,
    devices: [...clients].filter((c) => c.ready).map((c) => c.device),
    qr: enabled ? await require('qrcode').toDataURL(`http://${addrs[0]?.address || '127.0.0.1'}:${port()}/?d=${Buffer.from(JSON.stringify(pair)).toString('base64url')}`, { margin: 1, width: 220 }) : null,
    code: enabled ? token() : '',
  };
}
async function notifyWindow() {
  const w = deps.getWin();
  if (w && !w.isDestroyed()) w.webContents.send('remote:changed', await info());
}

ipcMain.handle('remote:info', () => info());
ipcMain.handle('remote:set', async (_e, { enabled, port: p, newToken }) => {
  const patch = {};
  if (typeof enabled === 'boolean') patch.remoteEnabled = enabled;
  if (p && +p >= 1024 && +p <= 65535) patch.remotePort = +p;
  if (newToken) patch.remoteToken = crypto.randomBytes(18).toString('base64url');
  deps.setConfig(patch);
  stop(); // a new token or port drops the paired phones
  if (cfg().remoteEnabled) start();
  return info();
});

function init(d) {
  deps = d;
  if (cfg().remoteEnabled) start();
}

module.exports = { init, stop, onData, onStart, onExit, onPrompt, onPromptDone, onRun };
