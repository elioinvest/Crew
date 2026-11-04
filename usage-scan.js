// Reads token usage out of Claude Code and Codex session files, in a process of its own so a first
// scan of gigabytes doesn't touch Crew (usage-stats.js starts it). Each file is scanned once, and
// later only what was added; the result is one row per hour, provider, model, speed and session.
// How to read the files correctly comes from T3 Code (MIT, Copyright (c) T3 Tools Inc.):
//   Claude: one line per content block, each repeating the whole message's usage, so a message
//     counts once (by message and request id, across files, as resumed sessions copy them).
//   Codex: token_count events carry no model (the last turn_context has it), can be re-emitted, and
//     forks copy their parent's history, so the cumulative counters decide what's a real request.

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const CACHE_VERSION = 1;
const CHUNK = 1024 * 1024;
const KEEP_DAYS = 100;
const SPEEDS = ['standard', 'fast', 'ultrafast'];
const count = (v) => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? Math.trunc(v) : 0);
const minuteOf = (v) => { const t = typeof v === 'string' ? Date.parse(v) : NaN; return Number.isNaN(t) ? null : Math.floor(t / 60000); };
const shortHash = (s) => crypto.createHash('md5').update(s).digest('base64').slice(0, 12);

function listFiles(dir) {
  let out = [];
  let entries = [];
  try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return out; }
  for (const e of entries) {
    const f = path.join(dir, e.name);
    if (e.isDirectory()) out = out.concat(listFiles(f));
    else if (e.name.endsWith('.jsonl')) out.push(f);
  }
  return out;
}

// ---------- one line ----------

const SYNTHETIC = '<synthetic>';

// { t, model, session, input, cached, write, output, reasoning, speed, key }
function claudeRecords(line) {
  if (!line.includes('"usage"') || !line.includes('"assistant"')) return [];
  let o;
  try { o = JSON.parse(line.toString('utf8')); } catch { return []; }
  if (o.type !== 'assistant') return [];
  const m = o.message;
  const u = m?.usage;
  if (!u || typeof u !== 'object') return [];
  const t = minuteOf(o.timestamp);
  const model = typeof m.model === 'string' ? m.model : '';
  if (t === null || !model || model === SYNTHETIC) return [];
  const key = typeof m.id === 'string' && typeof o.requestId === 'string' ? `${m.id}:${o.requestId}` : typeof m.id === 'string' ? m.id : null;
  const speed = u.speed === 'fast' ? 'fast' : 'standard';
  const session = typeof o.sessionId === 'string' ? o.sessionId : '';
  const out = [{
    t, model, session, speed,
    input: count(u.input_tokens), cached: count(u.cache_read_input_tokens), write: count(u.cache_creation_input_tokens), output: count(u.output_tokens), reasoning: 0,
    key,
  }];
  // Advisor calls run on another model at its own rates and appear only here.
  (Array.isArray(u.iterations) ? u.iterations : []).forEach((a, i) => {
    if (a?.type !== 'advisor_message' || typeof a.model !== 'string' || !a.model || a.model === SYNTHETIC) return;
    out.push({
      t, model: a.model, session, speed,
      input: count(a.input_tokens), cached: count(a.cache_read_input_tokens), write: count(a.cache_creation_input_tokens), output: count(a.output_tokens), reasoning: 0,
      key: key && `${key}:advisor:${i}`,
    });
  });
  return out;
}

const CODEX_FIELDS = ['input_tokens', 'cached_input_tokens', 'cache_write_input_tokens', 'output_tokens', 'reasoning_output_tokens', 'total_tokens'];

function codexUsage(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw) || typeof raw.input_tokens !== 'number' || typeof raw.output_tokens !== 'number') return null;
  const u = {
    input_tokens: raw.input_tokens,
    cached_input_tokens: raw.cached_input_tokens ?? 0,
    cache_write_input_tokens: raw.cache_write_input_tokens ?? 0,
    output_tokens: raw.output_tokens,
    reasoning_output_tokens: raw.reasoning_output_tokens ?? 0,
    total_tokens: raw.total_tokens ?? raw.input_tokens + raw.output_tokens,
  };
  if (!CODEX_FIELDS.every((k) => Number.isInteger(u[k]) && u[k] >= 0)) return null;
  if (u.cached_input_tokens + u.cache_write_input_tokens > u.input_tokens) return null;
  if (u.reasoning_output_tokens > u.output_tokens) return null;
  if (u.total_tokens < u.input_tokens + u.output_tokens) return null;
  return u;
}

const initialCodexState = () => ({ model: '', speed: 'standard', session: '', lastSignature: null, cumulative: null, sequenceStart: null, sawMeta: false, forkCopies: false, forkAnchor: 0 });
const sameUsage = (a, b) => CODEX_FIELDS.every((k) => a[k] === b[k]);
const FORK_COPY_GAP = 1000; // ms: forks restamp their parent's history in a burst

function rebase(state, cumulative) {
  state.cumulative = cumulative;
  if (cumulative.input_tokens + cumulative.output_tokens === 0) state.sequenceStart = null;
}

function codexRecords(line, state) {
  if (!line.includes('"token_count"') && !line.includes('"turn_context"') && !line.includes('"thread_settings_applied"') && !line.includes('"session_meta"')) return [];
  let o;
  try { o = JSON.parse(line.toString('utf8')); } catch { return []; }
  const p = o.payload;
  if (!p || typeof p !== 'object') return [];
  if (o.type === 'session_meta') {
    if (state.sawMeta) return []; // copied ancestors' metas would misattribute what follows
    state.sawMeta = true;
    const id = p.id ?? p.session_id;
    if (typeof id === 'string') state.session = id;
    const forked = typeof p.forked_from_id === 'string' || typeof p.source?.subagent?.thread_spawn?.parent_thread_id === 'string';
    const at = Date.parse(o.timestamp);
    if (forked && !Number.isNaN(at)) { state.forkCopies = true; state.forkAnchor = at; }
    return [];
  }
  if (o.type === 'turn_context') {
    if (typeof p.model === 'string') state.model = p.model;
    return [];
  }
  if (p.type === 'thread_settings_applied') {
    const tier = p.thread_settings?.service_tier;
    state.speed = tier === 'priority' || tier === 'fast' ? 'fast' : tier === 'ultrafast' ? 'ultrafast' : 'standard';
    return [];
  }
  if (p.type !== 'token_count' || !p.info || typeof p.info !== 'object') return [];
  const at = Date.parse(o.timestamp);
  if (Number.isNaN(at) || !state.model) return [];
  const last = codexUsage(p.info.last_token_usage);
  const hasCumulative = p.info.total_token_usage !== undefined && p.info.total_token_usage !== null;
  const cumulative = hasCumulative ? codexUsage(p.info.total_token_usage) : null;

  if (state.forkCopies) {
    if (at - state.forkAnchor < FORK_COPY_GAP) {
      state.forkAnchor = at;
      // Already billed in the parent, but it sets the baseline here.
      const sig = last && JSON.stringify(last);
      if (cumulative) rebase(state, cumulative);
      else if (!hasCumulative && last && sig !== state.lastSignature) state.cumulative = advance(state.cumulative, last);
      state.lastSignature = sig;
      return [];
    }
    state.forkCopies = false;
    state.lastSignature = null;
  }

  const previous = state.cumulative;
  if (cumulative) rebase(state, cumulative);
  if (!last || (hasCumulative && !cumulative)) return [];
  // Counters with nothing in them report the context's size, not a request.
  if (last.input_tokens + last.output_tokens === 0) { state.lastSignature = null; return []; }
  if (cumulative && previous && sameUsage(cumulative, previous)) return []; // re-emitted
  if (last.total_tokens !== last.input_tokens + last.output_tokens) return [];

  const sig = JSON.stringify(last);
  if (cumulative) {
    // A session resumed in a new process restarts its counters at this request.
    const restarted = sameUsage(cumulative, last);
    const start = JSON.stringify([at, last]);
    if (restarted && start === state.sequenceStart) { state.cumulative = previous; return []; }
    const reconciled = previous === null
      ? codexUsage(Object.fromEntries(CODEX_FIELDS.map((k) => [k, cumulative[k] - last[k]]))) !== null
      : CODEX_FIELDS.every((k) => cumulative[k] - previous[k] === last[k]);
    if (!restarted && !reconciled) return [];
    if (restarted) state.sequenceStart = start;
  } else {
    if (sig === state.lastSignature) return [];
    const advanced = advance(previous, last);
    if (!advanced) return [];
    state.cumulative = advanced;
  }
  state.lastSignature = sig;
  const input = last.input_tokens - last.cached_input_tokens - last.cache_write_input_tokens; // Codex's input includes the cached part
  if (input + last.cached_input_tokens + last.cache_write_input_tokens + last.output_tokens === 0) return [];
  return [{
    t: Math.floor(at / 60000), model: state.model, session: state.session, speed: state.speed,
    input, cached: last.cached_input_tokens, write: last.cache_write_input_tokens, output: last.output_tokens, reasoning: last.reasoning_output_tokens,
    key: null,
  }];
}

function advance(previous, last) {
  return codexUsage(Object.fromEntries(CODEX_FIELDS.map((k) => [k, (previous?.[k] ?? 0) + last[k]])));
}

// ---------- one file ----------

// Cached records are arrays: [minute, model index, session index, input, cached, write, output, reasoning, speed index, key hash].
async function scanFile(file, kind, prior) {
  const st = fs.statSync(file);
  if (prior && prior.size === st.size && prior.mtimeMs === st.mtimeMs) return prior;
  const fresh = !prior || st.size < prior.offset || prior.kind !== kind;
  const out = fresh
    ? { kind, size: 0, mtimeMs: 0, offset: 0, state: kind === 'codex' ? initialCodexState() : null, models: [], sessions: [], records: [] }
    : { ...prior };
  const intern = (list, v) => { let i = list.indexOf(v); if (i < 0) { i = list.length; list.push(v); } return i; };
  let lineStart = out.offset;
  let parts = [];
  if (st.size > out.offset) {
    for await (const chunk of fs.createReadStream(file, { start: out.offset, end: st.size - 1, highWaterMark: CHUNK })) {
      let from = 0;
      for (let nl = chunk.indexOf(10, from); nl >= 0; nl = chunk.indexOf(10, from)) {
        parts.push(chunk.subarray(from, nl));
        const line = parts.length === 1 ? parts[0] : Buffer.concat(parts);
        parts = [];
        const found = kind === 'codex' ? codexRecords(line, out.state) : claudeRecords(line);
        for (const r of found) {
          out.records.push([r.t, intern(out.models, r.model), intern(out.sessions, r.session), r.input, r.cached, r.write, r.output, r.reasoning, SPEEDS.indexOf(r.speed), r.key ? shortHash(r.key) : 0]);
        }
        lineStart += line.length + 1;
        from = nl + 1;
      }
      if (from < chunk.length) parts.push(chunk.subarray(from));
      out.offset = lineStart; // a line still being written is read again next time
    }
  }
  out.size = st.size;
  out.mtimeMs = st.mtimeMs;
  return out;
}

// ---------- prices ----------

// Keeps only the rates of the models seen, per token. From LiteLLM's price list; a model it doesn't
// have stays unpriced rather than understated. Cache rates missing from an entry use the input rate.
function loadPriceTable(file) {
  let doc;
  try { doc = JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return new Map(); }
  const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? v : null);
  const rates = (e, suffix, base) => {
    const input = num(e[`input_cost_per_token${suffix}`]);
    const output = num(e[`output_cost_per_token${suffix}`]);
    if (input === null || output === null) return null;
    const cache = (name, field) => num(e[`${name}${suffix}`]) ?? (base && base.input > 0 ? (base[field] / base.input) * input : input);
    return { input, output, cacheRead: cache('cache_read_input_token_cost', 'cacheRead'), cacheWrite: cache('cache_creation_input_token_cost', 'cacheWrite') };
  };
  const scaled = (r, x) => ({ input: r.input * x, output: r.output * x, cacheRead: r.cacheRead * x, cacheWrite: r.cacheWrite * x });
  const table = new Map();
  for (const [name, e] of Object.entries(doc)) {
    if (!e || typeof e !== 'object') continue;
    const standard = rates(e, '');
    if (!standard) continue;
    const key = name.trim().toLowerCase();
    if (!key) continue;
    const x = num(e.provider_specific_entry?.fast);
    table.set(key, { ...standard, fast: x > 0 ? scaled(standard, x) : rates(e, '_priority', standard), ultrafast: rates(e, '_ultrafast', standard) });
  }
  // "anthropic/claude-x" also answers to "claude-x", unless resellers disagree on its price.
  const aliases = new Map();
  const same = (a, b) => a.input === b.input && a.output === b.output && a.cacheRead === b.cacheRead && a.cacheWrite === b.cacheWrite;
  for (const [key, r] of table) {
    const bare = key.slice(key.lastIndexOf('/') + 1);
    if (!bare || bare === key || table.has(bare)) continue;
    if (!aliases.has(bare)) aliases.set(bare, r);
    else if (aliases.get(bare) && !same(aliases.get(bare), r)) aliases.set(bare, null);
  }
  for (const [k, r] of aliases) if (r) table.set(k, r);
  return table;
}

// Bare family names could be any generation, so they stay unpriced. "[1m]" variants price as the base.
const UNPRICEABLE = new Set(['synthetic', '<synthetic>', 'opus', 'sonnet', 'haiku', 'fable']);
function rateOf(table, model) {
  const key = model.trim().toLowerCase().replace(/\[.*$/, '');
  const bare = key.slice(key.lastIndexOf('/') + 1);
  if (!bare || UNPRICEABLE.has(bare)) return null;
  return table.get(key) || null;
}

// ---------- everything ----------

// roots: [{ kind: 'claude' | 'codex', dir, provider: label for grouping }]
async function scanAll({ roots, cachePath, pricesPath }) {
  let cache = {};
  try {
    const c = JSON.parse(fs.readFileSync(cachePath, 'utf8'));
    if (c.version === CACHE_VERSION) cache = c.files;
  } catch { /* first scan */ }
  const files = {};
  const next = {};
  let scanned = 0;
  for (const root of roots) {
    for (const f of listFiles(root.dir)) {
      try {
        const prior = cache[f];
        const r = await scanFile(f, root.kind, prior);
        if (r !== prior) scanned++;
        next[f] = r;
        files[f] = { ...root, r };
      } catch { /* unreadable now; the next scan tries again */ }
    }
  }
  // A transcript since deleted still counts: what was spent stays spent.
  for (const [f, r] of Object.entries(cache)) if (!next[f]) { next[f] = r; files[f] = { kind: r.kind, provider: r.provider, r }; }
  for (const [f, v] of Object.entries(files)) next[f].provider = v.provider;
  if (scanned) {
    try { fs.writeFileSync(cachePath, JSON.stringify({ version: CACHE_VERSION, files: next })); } catch { /* scan again next time */ }
  }

  const since = (Date.now() - KEEP_DAYS * 86400e3) / 60000;
  const seen = new Set();
  const rows = new Map();
  const sessions = [];
  const sessionIndex = new Map();
  const models = new Set();
  let duplicates = 0;
  for (const { provider, r } of Object.values(files)) {
    for (const rec of r.records) {
      const [t, m, s, input, cached, write, output, reasoning, sp, hash] = rec;
      // A message copied to another file counts once; a rollout that moved leaves a copy too.
      const key = hash || `${r.sessions[s]}:${t}:${input}:${cached}:${output}`;
      if (seen.has(key)) { duplicates++; continue; }
      seen.add(key);
      if (t < since) continue;
      const model = r.models[m];
      const session = r.sessions[s];
      let si = sessionIndex.get(session);
      if (si === undefined) { si = sessions.length; sessions.push(session); sessionIndex.set(session, si); }
      const hour = Math.floor(t / 60) * 3600000;
      const k = `${hour}|${provider}|${model}|${sp}|${si}`;
      const row = rows.get(k);
      if (row) { row[5] += input; row[6] += cached; row[7] += write; row[8] += output; row[9] += reasoning; } else rows.set(k, [hour, provider, model, sp, si, input, cached, write, output, reasoning]);
      models.add(model);
    }
  }
  const table = loadPriceTable(pricesPath);
  const rates = {};
  for (const m of models) { const r = rateOf(table, m); if (r) rates[m] = r; }
  return { rows: [...rows.values()], sessions: sessions.length, rates, scanned, files: Object.keys(files).length, duplicates, at: Date.now() };
}

if (process.parentPort) {
  process.parentPort.once('message', async (e) => {
    try { process.parentPort.postMessage({ ok: await scanAll(e.data) }); } catch (err) { process.parentPort.postMessage({ error: err.message }); }
    process.exit(0);
  });
}

module.exports = { scanAll, claudeRecords, codexRecords, initialCodexState, rateOf, loadPriceTable };
