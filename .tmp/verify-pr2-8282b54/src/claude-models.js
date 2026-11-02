// Claude's models and each one's reasoning-effort levels, for the composer's chips: from the model
// list T3 Code publishes (github.com/pingdotgg/t3code, apps/server/src/provider/model-manifest.json,
// MIT License, Copyright (c) 2026 T3 Tools Inc.). A copy ships with Crew (claude-models.json); once
// a day Crew fetches the current one and keeps it in its data folder. Models the installed Claude
// Code is too old for are left out.

const { ipcMain, net } = require('electron');
const fs = require('fs');
const path = require('path');
const { execFile } = require('child_process');

const SOURCE = 'https://raw.githubusercontent.com/pingdotgg/t3code/main/apps/server/src/provider/model-manifest.json';
const DAY = 24 * 3600 * 1000;
// The levels /effort takes. The list also has Ultracode (an option set alongside xhigh) and
// Ultrathink (a word in the prompt): neither is something /effort switches.
const EFFORTS = new Set(['low', 'medium', 'high', 'xhigh', 'max']);

let deps = null; // { dataDir, claudeVersionArgv: () => [file, args, opts] }
let list = null; // { current, models, profiles, updatedAt }
let version = null; // the installed Claude Code's version, "2.1.304"

const cacheFile = () => path.join(deps.dataDir(), 'claude-models.json');
const readJson = (f) => { try { return JSON.parse(fs.readFileSync(f, 'utf8')); } catch { return null; } };
const valid = (o) => o && Array.isArray(o.models) && o.models.length && o.profiles && typeof o.profiles === 'object';

function load() {
  const bundled = readJson(path.join(__dirname, 'claude-models.json'));
  const cached = readJson(cacheFile());
  list = valid(cached) && (!bundled || String(cached.updatedAt) >= String(bundled.updatedAt)) ? cached : bundled;
}

// The day's list from T3 Code, kept only if it reads right.
async function refresh(force = false) {
  try {
    const st = fs.statSync(cacheFile());
    if (!force && Date.now() - st.mtimeMs < DAY) return;
  } catch { /* none yet */ }
  try {
    const res = await net.fetch(SOURCE, { cache: 'no-store' });
    if (!res.ok) return;
    const m = await res.json();
    const c = m?.providers?.claudeAgent;
    const next = { source: SOURCE, updatedAt: m.updatedAt, current: m.currentModels?.claudeAgent || [], models: c?.models, profiles: c?.profiles, defaultModel: c?.defaults?.chat };
    if (!valid(next)) return;
    fs.writeFileSync(cacheFile(), JSON.stringify(next));
    list = next;
  } catch { /* offline: the copy we have */ }
}

function readVersion() {
  return new Promise((resolve) => {
    const [file, args, opts] = deps.claudeVersionArgv();
    execFile(file, args, { timeout: 15000, windowsHide: true, ...opts }, (err, out) => {
      const m = String(out || '').match(/(\d+)\.(\d+)\.(\d+)/);
      version = m ? m[0] : null;
      resolve(version);
    });
  });
}

const cmp = (a, b) => {
  const x = a.split('.').map(Number);
  const y = b.split('.').map(Number);
  for (let i = 0; i < 3; i++) if ((x[i] || 0) !== (y[i] || 0)) return (x[i] || 0) - (y[i] || 0);
  return 0;
};
const fits = (m) => {
  const c = m.adapter?.claudeCode || {};
  if (!version) return true;
  return (!c.minVersion || cmp(version, c.minVersion) >= 0) && (!c.maxVersionExclusive || cmp(version, c.maxVersionExclusive) < 0);
};

// What the window shows: current models first (in the list's order), then older ones, each with
// the effort levels /effort can set and the model's default.
function catalog() {
  if (!list) return null;
  const order = new Map((list.current || []).map((s, i) => [s, i]));
  const models = list.models.filter((m) => m.slug && m.status !== 'hidden' && fits(m)).map((m) => {
    const effort = (list.profiles[m.profile]?.capabilities?.optionDescriptors || []).find((d) => d.id === 'effort');
    return {
      slug: m.slug,
      label: String(m.name || m.slug).replace(/^Claude\s+/, ''),
      aliases: m.aliases || [],
      current: m.status === 'current',
      efforts: (effort?.options || []).filter((o) => EFFORTS.has(o.id)).map((o) => ({ id: o.id, label: o.label, isDefault: !!o.isDefault })),
    };
  });
  models.sort((a, b) => (a.current === b.current ? (order.get(a.slug) ?? 99) - (order.get(b.slug) ?? 99) : a.current ? -1 : 1));
  // What "Default" stands for now (Claude Code's recommended model).
  const def = models.find((m) => m.slug === list.defaultModel) || models.find((m) => m.current) || null;
  return { models, defaultModel: def?.slug || null, version, updatedAt: list.updatedAt };
}

ipcMain.handle('claude:models', () => catalog());
ipcMain.handle('claude:models-refresh', async () => { await Promise.all([refresh(true), readVersion()]); return catalog(); });

function init(d) {
  deps = d;
  load();
  readVersion();
  refresh();
}

module.exports = { init, catalog };
