// What the prompt box offers after / and $ (renderer/renderer.js): an agent's skills and custom
// commands, read from where Claude Code and Codex keep them, as T3 Code does.
//   Claude Code: <config>/skills/<name>/SKILL.md, <project>/.claude/skills, the skills of enabled
//     plugins (named plugin:skill), and the custom commands in commands/ (subfolders give dir:name).
//   Codex: its app-server's skills/list for the project, falling back to <CODEX_HOME>/skills; and
//     the custom prompts in prompts/ (run as /prompts:name).

const { ipcMain, app } = require('electron');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');

let deps = {}; // { shellCommand, cleanEnv, getConfig, providerHome }

const readText = (f) => { try { return fs.readFileSync(f, 'utf8'); } catch { return null; } };
const readJson = (f) => { try { return JSON.parse(fs.readFileSync(f, 'utf8')); } catch { return null; } };
const dirs = (d) => { try { return fs.readdirSync(d, { withFileTypes: true }).filter((e) => e.isDirectory() || e.isSymbolicLink()).map((e) => e.name); } catch { return []; } };

// The YAML front matter's plain keys (name, description, …), enough for SKILL.md and commands.
function frontMatter(text) {
  const m = /^---\r?\n([\s\S]*?)\r?\n---/.exec(text || '');
  if (!m) return {};
  const out = {};
  const lines = m[1].split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const kv = /^([\w-]+):[ \t]*(.*)$/.exec(lines[i]);
    if (!kv) continue;
    let v = kv[2].trim();
    if (v === '' || v === '|' || v === '>' || v === '>-' || v === '|-') { // a block on the lines below
      const block = [];
      while (i + 1 < lines.length && /^\s+\S/.test(lines[i + 1])) block.push(lines[++i].trim());
      v = block.join(' ');
    }
    out[kv[1]] = v.replace(/^(['"])(.*)\1$/, '$2');
  }
  return out;
}

const isFalse = (v) => /^(false|no|off|n|0)$/i.test(String(v ?? '').trim());
const oneLine = (s) => String(s || '').replace(/\s+/g, ' ').trim().slice(0, 200);

function skillAt(dir, fallbackName, prefix, scope) {
  const file = path.join(dir, 'SKILL.md');
  const text = readText(file);
  if (text == null) return null;
  const fm = frontMatter(text);
  if (isFalse(fm['user-invocable'])) return null;
  const base = (prefix ? fallbackName : fm.name || fallbackName).replace(/[^\w:-]/g, '-');
  return { name: prefix ? `${prefix}:${base}` : base, description: oneLine(fm.description), scope, path: file };
}

// A commands/ folder: each .md is a command; subfolders namespace them (dir:name).
function commandsIn(root, prefix, scope, out, rel = '') {
  let entries;
  try { entries = fs.readdirSync(path.join(root, rel), { withFileTypes: true }); } catch { return; }
  for (const e of entries) {
    const r = rel ? `${rel}/${e.name}` : e.name;
    if (e.isDirectory()) { if (rel.split('/').length < 3) commandsIn(root, prefix, scope, out, r); continue; }
    if (!/\.md$/i.test(e.name)) continue;
    const text = readText(path.join(root, r)) || '';
    const fm = frontMatter(text);
    const name = r.replace(/\.md$/i, '').split('/').join(':');
    const firstLine = text.replace(/^---[\s\S]*?\n---\r?\n/, '').split(/\r?\n/).find((l) => l.trim()) || '';
    out.push({ name: prefix ? `${prefix}:${name}` : name, description: oneLine(fm.description || firstLine), hint: fm['argument-hint'] || '', scope });
  }
}

function claudeItems(cwd, configDir) {
  const home = configDir || process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  const skills = [];
  const commands = [];
  const seen = new Set();
  const addSkill = (s) => { if (s && !seen.has(s.name)) { seen.add(s.name); skills.push(s); } };
  // User skills come first: Claude Code prefers them over a project's skill of the same name.
  for (const d of dirs(path.join(home, 'skills'))) addSkill(skillAt(path.join(home, 'skills', d), d, '', 'personal'));
  if (cwd) for (const d of dirs(path.join(cwd, '.claude', 'skills'))) addSkill(skillAt(path.join(cwd, '.claude', 'skills', d), d, '', 'project'));
  commandsIn(path.join(home, 'commands'), '', 'personal', commands);
  if (cwd) commandsIn(path.join(cwd, '.claude', 'commands'), '', 'project', commands);

  // Plugins: the ones turned on in settings, installed for everyone or for this project.
  const enabled = {};
  for (const f of [path.join(home, 'settings.json'), cwd && path.join(cwd, '.claude', 'settings.json'), cwd && path.join(cwd, '.claude', 'settings.local.json')]) {
    const s = f && readJson(f);
    if (s?.enabledPlugins && typeof s.enabledPlugins === 'object') Object.assign(enabled, s.enabledPlugins);
  }
  const installed = readJson(path.join(home, 'plugins', 'installed_plugins.json'))?.plugins || {};
  const here = cwd ? path.resolve(cwd).toLowerCase() : null;
  for (const key of Object.keys(installed).sort()) {
    if (enabled[key] !== true) continue;
    const list = Array.isArray(installed[key]) ? installed[key] : [];
    const install = list.find((r) => r?.installPath && (r.scope === 'user' || r.scope === 'managed' || (here && r.projectPath && path.resolve(r.projectPath).toLowerCase() === here)) && fs.existsSync(r.installPath));
    if (!install) continue;
    const manifest = readJson(path.join(install.installPath, '.claude-plugin', 'plugin.json')) || {};
    const prefix = manifest.name || key.split('@')[0];
    const declared = typeof manifest.skills === 'string' ? [manifest.skills] : Array.isArray(manifest.skills) ? manifest.skills : [];
    for (const rel of ['skills', ...declared]) {
      const root = path.resolve(install.installPath, rel);
      if (path.relative(install.installPath, root).startsWith('..')) continue;
      for (const d of dirs(root)) addSkill(skillAt(path.join(root, d), d, prefix, 'plugin'));
    }
    commandsIn(path.join(install.installPath, 'commands'), prefix, 'plugin', commands);
  }
  return { skills: skills.sort((a, b) => a.name.localeCompare(b.name)), commands };
}

// Codex's skills for a folder, asked of Codex itself (its app-server), so system and repo skills
// are included. Null when the app-server doesn't answer.
function codexSkillsFromServer(cwd, codexHome) {
  return new Promise((resolve) => {
    const cfg = deps.getConfig?.() || {};
    const [file, args] = deps.shellCommand([cfg.codexCommand || 'codex', 'app-server']);
    let child;
    try {
      const env = { ...deps.cleanEnv(), ...(codexHome ? { CODEX_HOME: codexHome } : {}) };
      child = spawn(file, args, { env, windowsHide: true, windowsVerbatimArguments: process.platform === 'win32', stdio: ['pipe', 'pipe', 'ignore'] });
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
    const timer = setTimeout(() => finish(null), 15000);
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
          send({ id: 2, method: 'skills/list', params: { cwds: [cwd] } });
        } else if (m.id === 2) {
          const data = m.result?.data;
          if (!Array.isArray(data)) { finish(null); return; }
          const entry = data.find((x) => path.resolve(x.cwd || '').toLowerCase() === path.resolve(cwd).toLowerCase());
          const list = entry ? entry.skills : data.flatMap((x) => x.skills || []);
          finish((list || []).filter((s) => s && s.name && s.enabled !== false).map((s) => ({
            name: s.name,
            description: oneLine(s.shortDescription || s.interface?.shortDescription || s.description),
            scope: s.scope === 'user' ? 'personal' : s.scope === 'repo' ? 'project' : s.scope || '',
            path: s.path || '',
          })));
        }
      }
    });
    send({ id: 1, method: 'initialize', params: { clientInfo: { name: 'crew', title: 'Crew', version: app.getVersion() }, capabilities: { experimentalApi: true } } });
  });
}

const codexCache = new Map(); // cwd|home -> { at, skills }
async function codexItems(cwd, codexHome) {
  const home = codexHome || process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
  const key = `${cwd}|${home}`;
  let skills = codexCache.get(key);
  if (!skills || Date.now() - skills.at > 5 * 60e3) {
    let list = cwd ? await codexSkillsFromServer(cwd, codexHome) : null;
    if (!list) {
      list = [];
      for (const d of dirs(path.join(home, 'skills'))) { const s = skillAt(path.join(home, 'skills', d), d, '', 'personal'); if (s) list.push(s); }
      if (cwd) for (const d of dirs(path.join(cwd, '.codex', 'skills'))) { const s = skillAt(path.join(cwd, '.codex', 'skills', d), d, '', 'project'); if (s) list.push(s); }
    }
    skills = { at: Date.now(), skills: list.sort((a, b) => a.name.localeCompare(b.name)) };
    codexCache.set(key, skills);
  }
  const commands = [];
  commandsIn(path.join(home, 'prompts'), 'prompts', 'personal', commands);
  return { skills: skills.skills, commands };
}

function init(d) {
  deps = d;
  // A custom provider's sessions run in its own config folder (CLAUDE_CONFIG_DIR / CODEX_HOME).
  ipcMain.handle('composer:items', async (_e, agent, cwd, providerId) => {
    try {
      const home = providerId ? deps.providerHome(providerId) : null;
      return agent === 'codex' ? await codexItems(cwd, home) : claudeItems(cwd, home);
    } catch {
      return { skills: [], commands: [] };
    }
  });
}

module.exports = { init, frontMatter };
