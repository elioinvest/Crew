// Custom model providers: gateways, self-hosted services and other APIs compatible with OpenAI or
// Anthropic. Each one is an agent of its own beside Claude and Codex, with its own environment: a
// home folder Crew keeps for it, where the official CLI that speaks its protocol keeps the
// provider's settings, sessions and history apart from the user's own Claude and Codex.
//   Anthropic Messages -> Claude Code with CLAUDE_CONFIG_DIR=<home> (ANTHROPIC_BASE_URL /
//                         ANTHROPIC_AUTH_TOKEN / ANTHROPIC_MODEL for the provider)
//   OpenAI Responses   -> Codex with CODEX_HOME=<home>, whose config.toml makes the provider its
//                         default model provider
// Those are the protocols the two CLIs support; Codex dropped Chat Completions.
//
// providers.json holds the providers (the API key encrypted with Windows data protection), and
// provider-sessions.json the model and effort picked in each provider session.

const { app, ipcMain, safeStorage, dialog } = require('electron');
const fs = require('fs');
const os = require('os');
const path = require('path');

const file = (name) => path.join(app.getPath('userData'), name);
const readJson = (name, fallback) => { try { return JSON.parse(fs.readFileSync(file(name), 'utf8')); } catch { return fallback; } };
const writeJson = (name, v) => fs.writeFileSync(file(name), JSON.stringify(v, null, 2));

const seal = (s) => (s && safeStorage.isEncryptionAvailable() ? safeStorage.encryptString(s).toString('base64') : null);
const unseal = (b) => { try { return b ? safeStorage.decryptString(Buffer.from(b, 'base64')) : ''; } catch { return ''; } };

const PROTOCOLS = { anthropic: 'claude', responses: 'codex' }; // protocol -> the CLI that speaks it
const ID_RE = /^[a-z][a-z0-9-]{0,39}$/;
const MAX_ICON = 512 * 1024;

let providers = readJson('providers.json', []);
// session id -> { provider, model, effort }: the provider a session runs on and what was picked in it.
let bindings = Object.fromEntries(Object.entries(readJson('provider-sessions.json', {}))
  .map(([k, v]) => [k, typeof v === 'string' ? { provider: v } : v]));
const saveProviders = () => writeJson('providers.json', providers);
const saveBindings = () => writeJson('provider-sessions.json', bindings);

const find = (id) => providers.find((p) => p.id === id) || null;
const modelOf = (p, model) => model || p.defaultModel || p.models?.[0]?.id || '';

// What the window sees: never the key.
const publicProvider = (p) => ({
  id: p.id, name: p.name, baseUrl: p.baseUrl, protocol: p.protocol, cli: PROTOCOLS[p.protocol],
  models: p.models || [], defaultModel: modelOf(p), icon: p.icon || null, hasKey: !!p.key,
});

// The root of the API, for listing models: the URL as given, with /v1 added for Anthropic-style
// roots (Claude Code's ANTHROPIC_BASE_URL has no /v1).
const trimUrl = (u) => String(u || '').trim().replace(/\/+$/, '');
const anthropicBase = (u) => trimUrl(u).replace(/\/v1$/, '');
const apiRoot = (p) => (/\/v\d+$/.test(trimUrl(p.baseUrl)) ? trimUrl(p.baseUrl) : `${trimUrl(p.baseUrl)}/v1`);

const EFFORTS = ['low', 'medium', 'high', 'xhigh'];

// ---------- each provider's own environment ----------

const homesRoot = () => file('provider-homes');
const homeOf = (id) => path.join(homesRoot(), id);
// The homes to list sessions from: { id, cli, home }.
const homes = () => providers.map((p) => ({ id: p.id, cli: PROTOCOLS[p.protocol], home: homeOf(p.id) }));

// Crew's own tools follow the agent into the provider's home: the crew-ssh and ego-browser skills
// (linked, so they stay the user's installed ones) and, for Codex, the rules that let crew-ssh and
// crew run outside its sandbox.
function linkSkills(home, cliHome) {
  for (const name of ['crew-ssh', 'ego-browser']) {
    const src = path.join(cliHome, 'skills', name);
    const dst = path.join(home, 'skills', name);
    if (!fs.existsSync(src) || fs.existsSync(dst)) continue;
    try { fs.mkdirSync(path.dirname(dst), { recursive: true }); fs.symlinkSync(fs.realpathSync(src), dst, 'junction'); } catch { /* the agent works without it */ }
  }
}

const toml = (s) => JSON.stringify(String(s)); // a TOML basic string

// The [windows] table of the user's own Codex config.toml (the sandbox setting), or null.
function windowsTable(userCodex) {
  try {
    const text = fs.readFileSync(path.join(userCodex, 'config.toml'), 'utf8');
    const m = /^\[windows\][^\n]*\n([\s\S]*?)(?=^\s*\[|(?![\s\S]))/m.exec(text);
    return m ? `[windows]\n${m[1].trim()}\n` : null;
  } catch { return null; }
}

// Codex writes into the home's config.toml too: the folders the user trusted ([projects."…"]) and
// settings of its own ([tui], [notice], …). Crew rewrites only what it manages (the provider, the
// model, the update check, [model_providers.crew], [windows]) and keeps the rest, top-level keys
// included.
const CREW_KEYS = new Set(['model_provider', 'model', 'check_for_update_on_startup', 'model_catalog_json']);
const CREW_TABLES = new Set(['model_providers.crew', 'windows']);
function codexOwnSettings(configFile) {
  let text;
  try { text = fs.readFileSync(configFile, 'utf8'); } catch { return null; }
  const kept = [];
  let table = null; // null: top level
  let keep = true;
  for (const line of text.split(/\r?\n/)) {
    const header = /^\s*\[\[?\s*([^\]]+?)\s*\]\]?\s*(#.*)?$/.exec(line);
    if (header) { table = header[1]; keep = !CREW_TABLES.has(table); }
    else if (table === null) {
      const key = /^\s*([A-Za-z0-9_-]+)\s*=/.exec(line)?.[1];
      if (!key || CREW_KEYS.has(key) || /^\s*# Written by Crew/.test(line)) continue;
    }
    if (keep && (table !== null || line.trim())) kept.push(line);
  }
  // Top-level keys must come before any table: put them first, then the kept tables.
  const firstTable = kept.findIndex((l) => /^\s*\[/.test(l));
  const top = firstTable < 0 ? kept : kept.slice(0, firstTable);
  const tables = firstTable < 0 ? [] : kept.slice(firstTable);
  return { top: top.join('\n').trim(), tables: tables.join('\n').trim() };
}

// Codex's /model only accepts models in its catalog, so the provider's models go into one
// (model_catalog_json): Codex's fallback metadata for models it doesn't know, with each model's
// name and the effort levels, and Codex's own base instructions (codex-prompt.md, from
// openai/codex rust-v0.165.0, Apache-2.0), which a catalog entry has to carry. Returns its path.
const CATALOG_EFFORTS = [
  ['low', 'Fast responses with lighter reasoning'], ['medium', 'Balances speed and reasoning depth'],
  ['high', 'Greater reasoning depth for complex problems'], ['xhigh', 'Extra high reasoning depth'],
].map(([effort, description]) => ({ effort, description }));
function writeCatalog(p, home) {
  if (!p.models?.length) return null;
  let base;
  try { base = fs.readFileSync(path.join(__dirname, 'codex-prompt.md'), 'utf8'); } catch { return null; }
  const models = p.models.map((m, i) => ({
    slug: m.id, display_name: m.name || m.id, description: p.name, default_reasoning_level: 'medium',
    supported_reasoning_levels: CATALOG_EFFORTS, shell_type: 'unified_exec', visibility: 'list', supported_in_api: true,
    priority: i, availability_nux: null, upgrade: null, support_verbosity: false, default_verbosity: null,
    apply_patch_tool_type: null, truncation_policy: { mode: 'bytes', limit: 10000 }, experimental_supported_tools: [],
    base_instructions: base,
  }));
  const f = path.join(home, 'models.json');
  fs.writeFileSync(f, JSON.stringify({ models }));
  return f;
}

// Writes the provider's home: its settings for the CLI that runs it. Rewritten whenever the provider
// is saved; the sessions and history the CLI keeps there are left alone.
function writeHome(p) {
  const home = homeOf(p.id);
  fs.mkdirSync(home, { recursive: true });
  const m = modelOf(p);
  if (p.protocol === 'responses') {
    const userCodex = process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
    const configFile = path.join(home, 'config.toml');
    // What Codex itself saved here (folder trust, its own settings), so a restart doesn't ask again.
    const own = codexOwnSettings(configFile);
    const catalog = writeCatalog(p, home);
    fs.writeFileSync(configFile, [
      `# Written by Crew for the custom provider ${toml(p.name)}. Crew rewrites its own settings when the provider changes and keeps the rest.`,
      'model_provider = "crew"',
      m ? `model = ${toml(m)}` : null,
      'check_for_update_on_startup = false',
      catalog ? `model_catalog_json = ${toml(catalog)}` : null,
      own?.top || null,
      '',
      '[model_providers.crew]',
      `name = ${toml(p.name)}`,
      `base_url = ${toml(trimUrl(p.baseUrl))}`,
      'env_key = "CREW_PROVIDER_API_KEY"',
      'wire_api = "responses"',
      '',
      // The user's Windows sandbox choice, or Codex asks to set its sandbox up again in this home.
      windowsTable(userCodex),
      own?.tables || null,
      '',
    ].filter((l) => l != null).join('\n'));
    linkSkills(home, userCodex);
    try { // crew-ssh / crew out of the sandbox (ssh.js writes the same file into the user's Codex home)
      const rules = path.join(userCodex, 'rules', 'crew.rules');
      if (fs.existsSync(rules)) { fs.mkdirSync(path.join(home, 'rules'), { recursive: true }); fs.copyFileSync(rules, path.join(home, 'rules', 'crew.rules')); }
    } catch { /* written again at the next session */ }
  } else {
    // Claude Code keeps its global state in <home>/.claude.json: skip the first-run setup, with the
    // user's theme.
    const state = path.join(home, '.claude.json');
    if (!fs.existsSync(state)) {
      let theme;
      try { theme = JSON.parse(fs.readFileSync(path.join(os.homedir(), '.claude.json'), 'utf8')).theme; } catch { /* default */ }
      fs.writeFileSync(state, JSON.stringify({ hasCompletedOnboarding: true, ...(theme ? { theme } : {}) }, null, 2));
    }
    linkSkills(home, path.join(os.homedir(), '.claude'));
  }
  return home;
}

// How to start a session on the provider: the CLI, extra arguments and environment changes.
// `model` overrides the provider's default model, `effort` sets the reasoning effort.
function launch(providerId, { model, effort } = {}) {
  const p = find(providerId);
  if (!p) return null;
  const home = writeHome(p);
  const key = unseal(p.key);
  const m = modelOf(p, model);
  if (p.protocol === 'anthropic') {
    const env = { CLAUDE_CONFIG_DIR: home, ANTHROPIC_BASE_URL: anthropicBase(p.baseUrl), ANTHROPIC_AUTH_TOKEN: key, ANTHROPIC_API_KEY: null };
    // Claude Code also uses its Haiku/Sonnet/Opus aliases for background work and /model; point
    // them all at the provider's model, which is likely the only one it serves under that name.
    if (m) Object.assign(env, { ANTHROPIC_MODEL: m, ANTHROPIC_DEFAULT_OPUS_MODEL: m, ANTHROPIC_DEFAULT_SONNET_MODEL: m, ANTHROPIC_DEFAULT_HAIKU_MODEL: m });
    return { cli: 'claude', args: EFFORTS.includes(effort) ? ['--effort', effort] : [], env };
  }
  const args = [];
  if (m) args.push('-c', `model=${m}`);
  if (EFFORTS.includes(effort)) args.push('-c', `model_reasoning_effort=${effort}`);
  return { cli: 'codex', args, env: { CODEX_HOME: home, CREW_PROVIDER_API_KEY: key } };
}

// Applies launch() env changes: null removes a variable (case-insensitively, for Windows).
function applyEnv(env, changes) {
  const out = { ...env };
  for (const [k, v] of Object.entries(changes || {})) {
    for (const existing of Object.keys(out)) if (existing.toUpperCase() === k.toUpperCase()) delete out[existing];
    if (v != null) out[k] = v;
  }
  return out;
}

// What a session runs on: { provider, model, effort }, or null for the CLI's own account.
const sessionProvider = (sessionId) => (sessionId && find(bindings[sessionId]?.provider) ? bindings[sessionId] : null);
function bind(sessionId, providerId, { model, effort } = {}) {
  if (!sessionId) return;
  if (providerId && find(providerId)) bindings[sessionId] = { provider: providerId, model: model || undefined, effort: effort || undefined };
  else delete bindings[sessionId];
  saveBindings();
}
function rebind(from, to) {
  if (!bindings[from] || from === to) return;
  bindings[to] = bindings[from];
  delete bindings[from];
  saveBindings();
}

// The models the provider lists: GET <root>/models, as OpenAI and Anthropic both serve it.
async function fetchModels({ id, baseUrl, protocol, key }) {
  const saved = id ? find(id) : null;
  const p = { baseUrl: baseUrl || saved?.baseUrl, protocol: protocol || saved?.protocol };
  const token = key || unseal(saved?.key);
  if (!p.baseUrl) throw new Error('Enter the API URL first.');
  const headers = { accept: 'application/json' };
  if (token) Object.assign(headers, { authorization: `Bearer ${token}`, 'x-api-key': token });
  if (p.protocol === 'anthropic') headers['anthropic-version'] = '2023-06-01';
  const res = await fetch(`${apiRoot(p)}/models`, { headers, signal: AbortSignal.timeout(15000) });
  if (!res.ok) throw new Error(`The provider answered ${res.status} ${res.statusText}.`);
  const body = await res.json();
  const list = Array.isArray(body) ? body : body.data || body.models || [];
  return [...new Set(list.map((m) => (typeof m === 'string' ? m : m.id || m.name)).filter(Boolean))].sort();
}

ipcMain.handle('providers:list', () => providers.map(publicProvider));

ipcMain.handle('providers:save', (_e, input) => {
  const id = String(input.id || '').trim();
  if (!ID_RE.test(id)) throw new Error('The provider ID starts with a lowercase letter and uses only a-z, 0-9 and -.');
  if (!PROTOCOLS[input.protocol]) throw new Error('Choose an API protocol.');
  if (!/^https?:\/\/\S+$/i.test(String(input.baseUrl || '').trim())) throw new Error('Enter the API URL, starting with https://.');
  let p = find(input.originalId || id);
  if (input.originalId !== id && find(id)) throw new Error(`A provider with the ID "${id}" already exists.`);
  if (!p) { p = {}; providers.push(p); }
  if (p.id && p.id !== id) {
    for (const b of Object.values(bindings)) if (b.provider === p.id) b.provider = id;
    try { fs.renameSync(homeOf(p.id), homeOf(id)); } catch { /* no sessions yet */ } // its sessions move with it
  }
  if (p.protocol && p.protocol !== input.protocol) {
    // The other CLI keeps its files differently: start the home over, keeping the old one aside.
    try { fs.renameSync(homeOf(id), `${homeOf(id)}-${p.protocol}-${Date.now()}`); } catch { /* none */ }
  }
  const models = [];
  for (const m of input.models || []) {
    const mid = String(m.id || m).trim();
    if (mid && !models.some((x) => x.id === mid)) models.push({ id: mid, name: String(m.name || '').trim() || mid });
  }
  const icon = input.icon?.emoji ? { emoji: String(input.icon.emoji).slice(0, 8) }
    : input.icon?.image && String(input.icon.image).startsWith('data:image/') && input.icon.image.length < MAX_ICON * 1.4 ? { image: input.icon.image } : null;
  Object.assign(p, {
    id, name: String(input.name || '').trim() || id, baseUrl: trimUrl(input.baseUrl), protocol: input.protocol,
    models, defaultModel: models.some((m) => m.id === input.defaultModel) ? input.defaultModel : models[0]?.id || '', icon,
  });
  if (input.key) p.key = seal(String(input.key).trim());
  saveProviders();
  saveBindings();
  try { writeHome(p); } catch { /* written again at the next session */ }
  return publicProvider(p);
});

ipcMain.handle('providers:delete', (_e, id) => {
  providers = providers.filter((p) => p.id !== id);
  saveProviders();
});

ipcMain.handle('providers:fetch-models', async (_e, q) => {
  try { return { models: await fetchModels(q) }; } catch (e) { return { error: e.name === 'TimeoutError' ? 'The provider didn\'t answer within 15 seconds.' : e.message }; }
});

// Reads a picked image into a data URL, kept with the provider.
ipcMain.handle('providers:pick-icon', async () => {
  const r = await dialog.showOpenDialog({ properties: ['openFile'], filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'svg', 'ico', 'webp', 'gif'] }] });
  if (r.canceled || !r.filePaths[0]) return null;
  const f = r.filePaths[0];
  if (fs.statSync(f).size > MAX_ICON) return { error: 'Choose an image under 512 KB.' };
  const ext = path.extname(f).slice(1).toLowerCase();
  const type = { svg: 'image/svg+xml', jpg: 'image/jpeg', ico: 'image/x-icon' }[ext] || `image/${ext}`;
  return { image: `data:${type};base64,${fs.readFileSync(f).toString('base64')}` };
});

ipcMain.handle('providers:bind', (_e, sessionId, providerId, picks) => bind(sessionId, providerId, picks));
ipcMain.handle('providers:rebind', (_e, from, to) => rebind(from, to));
ipcMain.handle('providers:bindings', () => bindings);

module.exports = { launch, applyEnv, sessionProvider, bind, rebind, homes, writeHome: (id) => find(id) && writeHome(find(id)), homesRoot, has: (id) => !!find(id) };
