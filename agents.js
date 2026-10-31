// Agents the main session can hand work to by @-mention: saved profiles (a name, Claude, Codex
// or a custom model provider, a model, instructions, what it may do). The main agent runs one with
// `crew ask <agent> "<task>"`, one after another or several at once (--bg, then `crew wait`).
// Each run is the agent's CLI in its non-interactive mode in the caller's folder (a provider's
// agent: the CLI that speaks its protocol, in the provider's own home, providers.js); its events
// become a live card in the window, and its final answer goes back to the caller.

const { app, ipcMain } = require('electron');
const fs = require('fs');
const path = require('path');
const os = require('os');
const crypto = require('crypto');
const { spawn, spawnSync } = require('child_process');

const MAX_DEPTH = 2; // an agent's agent may delegate once more, no further
const WAIT_MS = 9 * 60e3; // how long `crew ask` / `crew wait` block before saying "still running"

let deps = null; // { getWin, cleanEnv, sessionEnv, shellCommand, getConfig, onRunDone, writeCodexRules, providerLaunch, applyEnv, hasProvider }
const file = (name) => path.join(app.getPath('userData'), name);

// ---------- the agents ----------

// { id, name, cli: 'claude' | 'codex' | 'provider:<id>', model, effort, instructions, access: 'read' | 'edit' | 'full' }
let agents = null;
function allAgents() {
  if (!agents) { try { agents = JSON.parse(fs.readFileSync(file('agents.json'), 'utf8')); } catch { agents = []; } }
  return agents;
}
const saveAgents = () => fs.writeFileSync(file('agents.json'), JSON.stringify(agents, null, 2));

const cleanName = (s) => String(s || '').trim().replace(/^@+/, '').replace(/\s+/g, '');
function findAgent(ref) {
  const n = cleanName(ref).toLowerCase();
  return allAgents().find((a) => a.name.toLowerCase() === n || a.id === ref) || null;
}

// ---------- runs ----------

const runs = new Map(); // run id -> run
let runSeq = 0;

const publicRun = (r) => ({
  id: r.id, agent: r.agent.name, cli: r.agent.cli, task: r.task, cwd: r.cwd, caller: r.caller,
  status: r.status, activity: r.activity, startedAt: r.startedAt, endedAt: r.endedAt,
  result: r.status === 'running' ? '' : r.result, error: r.error, sessionId: r.sessionId,
});

function notify(r) {
  const w = deps.getWin();
  if (w && !w.isDestroyed()) w.webContents.send('crew:run', publicRun(r));
}

const short = (s, n = 90) => { const t = String(s || '').replace(/\s+/g, ' ').trim(); return t.length > n ? `${t.slice(0, n - 1)}…` : t; };

// What the agent is doing now, from one of its tool calls.
function describeTool(name, input = {}) {
  const f = input.file_path || input.path || input.notebook_path;
  if (/^(Read|View)$/.test(name) && f) return `Reading ${path.basename(f)}`;
  if (/^(Edit|MultiEdit|Write|NotebookEdit)$/.test(name) && f) return `Editing ${path.basename(f)}`;
  if (name === 'Bash' && input.command) return `Running ${short(input.command, 60)}`;
  if (/^(Grep|Glob)$/.test(name)) return `Searching ${short(input.pattern || '', 40)}`;
  if (/^Web/.test(name)) return `Looking up ${short(input.query || input.url || '', 50)}`;
  if (name === 'Task' || name === 'Agent') return 'Delegating a subtask';
  return `Using ${name}`;
}

function finish(r, status, { result, error } = {}) {
  if (r.status !== 'running') return;
  r.status = status;
  r.endedAt = Date.now();
  if (result != null) r.result = result;
  if (error) r.error = error;
  r.activity = status === 'done' ? 'Done' : status === 'stopped' ? 'Stopped' : short(error || 'Failed', 120);
  for (const w of r.waiters) w();
  r.waiters.clear();
  notify(r);
  deps.onRunDone?.(r);
}

// The access setting, as each CLI's own permission flags.
function claudeArgs(a, promptFile) {
  const mode = { read: 'dontAsk', edit: 'auto', full: 'bypassPermissions' }[a.access] || 'dontAsk';
  const args = ['-p', '--output-format', 'stream-json', '--verbose', '--permission-mode', mode];
  if (a.access === 'read' || !a.access) args.push('--disallowedTools', 'Edit', 'Write', 'NotebookEdit');
  if (a.model) args.push('--model', a.model);
  if (a.effort) args.push('--effort', a.effort);
  if (promptFile) args.push('--append-system-prompt-file', promptFile);
  return args;
}

function codexArgs(a, cwd) {
  const args = ['exec', '--json', '--skip-git-repo-check', '-C', cwd];
  if (a.access === 'full') args.push('--dangerously-bypass-approvals-and-sandbox');
  else args.push('--sandbox', a.access === 'edit' ? 'workspace-write' : 'read-only');
  if (a.model) args.push('-m', a.model);
  if (a.effort) args.push('-c', `model_reasoning_effort=${a.effort}`);
  args.push('-'); // the prompt comes on stdin
  return args;
}

function startRun(agent, task, { cwd, caller, depth }) {
  const id = `r${++runSeq}`;
  const r = {
    id, agent: { ...agent }, task, cwd, caller, depth, status: 'running', activity: 'Starting…',
    startedAt: Date.now(), endedAt: 0, result: '', error: '', sessionId: null, waiters: new Set(), child: null, lastText: '',
  };
  runs.set(id, r);
  const cfg = deps.getConfig();
  let env = { ...deps.sessionEnv(deps.cleanEnv(), '', agent.cli), CREW_DEPTH: String(depth + 1), CREW_RUN_ID: id };
  // A custom provider's agent runs on the CLI that speaks its protocol (Codex or Claude Code), in the
  // provider's own home and with its model and effort (providers.js).
  let engine = agent.cli;
  let pre = [];
  let opts = agent;
  if (agent.cli.startsWith('provider:')) {
    const run = deps.providerLaunch(agent.cli.slice(9), { model: agent.model, effort: agent.effort });
    if (!run) { finish(r, 'failed', { error: 'Its model provider no longer exists in Crew.' }); return r; }
    engine = run.cli;
    pre = run.args;
    env = deps.applyEnv(env, run.env);
    opts = { ...agent, model: '', effort: '' }; // they come with the provider's launch
  }
  r.engine = engine;
  let argv;
  let input;
  if (engine === 'codex') {
    deps.writeCodexRules({ always: true }); // lets crew-ssh and crew out of Codex's sandbox
    argv = [cfg.codexCommand || 'codex', ...pre, ...codexArgs(opts, cwd)];
    input = agent.instructions?.trim() ? `${agent.instructions.trim()}\n\n---\n\nYour task:\n${task}` : task;
  } else {
    let promptFile = null;
    if (agent.instructions?.trim()) {
      promptFile = path.join(os.tmpdir(), `crew-agent-${id}-${process.pid}.md`);
      fs.writeFileSync(promptFile, `You are ${agent.name}, an agent another agent handed a task to through Crew. Do the task, then end with your answer: it's returned to them as is.\n\n${agent.instructions.trim()}\n`);
      r.promptFile = promptFile;
    }
    argv = [(cfg.claudeCommand || 'claude').split(/\s+/)[0], ...pre, ...claudeArgs(opts, promptFile)];
    input = task;
  }
  const [cmd, args] = deps.shellCommand(argv);
  let child;
  try {
    child = spawn(cmd, args, { cwd, env, windowsHide: true, windowsVerbatimArguments: process.platform === 'win32', stdio: ['pipe', 'pipe', 'pipe'] });
  } catch (e) {
    finish(r, 'failed', { error: e.message });
    return r;
  }
  r.child = child;
  child.stdin.on('error', () => { /* exited early */ });
  child.stdin.end(input);
  let buf = '';
  let stderr = '';
  child.stdout.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i).trim();
      buf = buf.slice(i + 1);
      if (line) onEvent(r, line);
    }
  });
  child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
  child.on('error', (e) => finish(r, 'failed', { error: e.message }));
  child.on('close', (code) => {
    if (buf.trim()) onEvent(r, buf.trim());
    if (r.promptFile) fs.rm(r.promptFile, { force: true }, () => {});
    if (r.status !== 'running') return;
    if (code === 0 && (r.result || r.lastText)) finish(r, 'done', { result: r.result || r.lastText });
    else finish(r, 'failed', { error: (stderr.trim().split('\n').filter(Boolean).pop()) || `${agent.cli} exited with code ${code}` });
  });
  notify(r);
  return r;
}

// One line of the CLI's JSON event stream.
function onEvent(r, line) {
  let e;
  try { e = JSON.parse(line); } catch { return; }
  let activity = null;
  if (r.engine === 'claude') {
    if (e.type === 'system' && e.session_id) r.sessionId = e.session_id;
    if (e.type === 'assistant') {
      for (const c of e.message?.content || []) {
        if (c.type === 'tool_use') activity = describeTool(c.name, c.input);
        else if (c.type === 'text' && c.text.trim()) { r.lastText = c.text; activity = short(c.text); }
      }
    }
    if (e.type === 'result') {
      if (e.session_id) r.sessionId = e.session_id;
      if (e.is_error || e.subtype !== 'success') finish(r, 'failed', { error: short(e.result || e.subtype || 'The agent stopped with an error', 300), result: e.result || '' });
      else finish(r, 'done', { result: e.result || r.lastText });
      return;
    }
  } else {
    if (e.type === 'thread.started' && e.thread_id) r.sessionId = e.thread_id;
    const item = e.item;
    if (item && (e.type === 'item.started' || e.type === 'item.completed')) {
      if (item.type === 'command_execution') activity = `Running ${short(item.command, 60)}`;
      else if (item.type === 'file_change') activity = `Editing ${(item.changes || []).map((c) => path.basename(c.path || '')).filter(Boolean).slice(0, 3).join(', ') || 'files'}`;
      else if (item.type === 'web_search') activity = `Looking up ${short(item.query || '', 50)}`;
      else if (item.type === 'agent_message' && e.type === 'item.completed' && item.text) { r.lastText = item.text; activity = short(item.text); }
      else if (item.type === 'reasoning') activity = 'Thinking…';
    }
    if (e.type === 'turn.failed' || e.type === 'error') {
      finish(r, 'failed', { error: short(e.error?.message || e.message || 'Codex stopped with an error', 300) });
      return;
    }
    if (e.type === 'turn.completed') { finish(r, 'done', { result: r.lastText }); return; }
  }
  if (activity && activity !== r.activity) { r.activity = activity; notify(r); }
}

function stopRun(r) {
  if (r.status !== 'running') return;
  const pid = r.child?.pid;
  // The CLI runs under cmd.exe on Windows: stop the whole tree, not just the shell.
  if (pid && process.platform === 'win32') spawnSync('taskkill', ['/pid', String(pid), '/T', '/F'], { windowsHide: true });
  else { try { r.child?.kill(); } catch { /* gone */ } }
  finish(r, 'stopped', { error: 'Stopped by the user' });
}

const waitFor = (r, ms) => (r.status !== 'running' ? Promise.resolve(true) : new Promise((resolve) => {
  const done = () => { clearTimeout(t); resolve(true); };
  const t = setTimeout(() => { r.waiters.delete(done); resolve(false); }, ms);
  r.waiters.add(done);
}));

const elapsed = (r) => { const s = Math.round(((r.endedAt || Date.now()) - r.startedAt) / 1000); return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`; };

function report(r) {
  const head = `=== ${r.agent.name} (${r.id}) · ${r.status}${r.status === 'running' ? '' : ` · ${elapsed(r)}`} ===`;
  if (r.status === 'running') return `${head}\nStill running (${r.activity}). Check again with: crew wait ${r.id}`;
  if (r.status === 'done') return `${head}\n${r.result.trim()}`;
  return `${head}\n${r.error}${r.result ? `\n${r.result.trim()}` : ''}`;
}

// ---------- the crew command's requests (over crew-ssh's pipe, see ssh.js) ----------

const out = (send, text) => send({ type: 'out', d: Buffer.from(`${text}\n`).toString('base64') });

const ops = {
  agents(m, { send, end }) {
    send({ type: 'result', agents: allAgents().map((a) => ({ name: a.name, cli: a.cli, model: a.model || '', effort: a.effort || '', access: a.access || 'read', instructions: a.instructions || '' })) });
    end();
  },

  async ask(m, { send, end, fail }) {
    const a = findAgent(m.agent);
    if (!a) { fail(`No agent named "${cleanName(m.agent)}". Agents: ${allAgents().map((x) => x.name).join(', ') || 'none yet (add them under Agents in Crew)'}.`); return; }
    const depth = +m.depth || 0;
    if (depth >= MAX_DEPTH) { fail(`Delegation is nested too deep (${depth} levels). Do this task yourself.`); return; }
    if (!String(m.task || '').trim()) { fail('No task given.'); return; }
    if (!m.cwd || !fs.existsSync(m.cwd)) { fail('The working folder was not found.'); return; }
    const r = startRun(a, m.task, { cwd: m.cwd, caller: m.session || '', depth });
    send({ type: 'status', message: `started ${a.name} as ${r.id}` });
    if (m.bg) { out(send, r.id); send({ type: 'exit', code: 0 }); end(); return; }
    const finished = await waitFor(r, Math.min(+m.timeout || WAIT_MS, WAIT_MS));
    out(send, report(r));
    send({ type: 'exit', code: !finished ? 3 : r.status === 'done' ? 0 : 1 });
    end();
  },

  async wait(m, { send, end, fail }) {
    const list = (m.ids || []).map((id) => runs.get(id));
    if (!list.length || list.some((r) => !r)) { fail(`Unknown run id. Runs: ${[...runs.keys()].join(', ') || 'none'}.`); return; }
    const deadline = Date.now() + Math.min(+m.timeout || WAIT_MS, WAIT_MS);
    for (const r of list) await waitFor(r, Math.max(0, deadline - Date.now()));
    out(send, list.map(report).join('\n\n'));
    send({ type: 'exit', code: list.some((r) => r.status === 'running') ? 3 : list.every((r) => r.status === 'done') ? 0 : 1 });
    end();
  },

  result(m, ctx) { return ops.wait({ ...m, timeout: 1 }, ctx); },

  stop(m, { send, end, fail }) {
    const r = runs.get(m.id);
    if (!r) { fail(`Unknown run id ${m.id}.`); return; }
    stopRun(r);
    out(send, report(r));
    send({ type: 'exit', code: 0 });
    end();
  },

  runs(m, { send, end }) {
    const mine = [...runs.values()].filter((r) => !m.session || r.caller === m.session);
    out(send, mine.length ? mine.map((r) => `${r.id}\t${r.agent.name}\t${r.status}\t${elapsed(r)}\t${short(r.task, 60)}`).join('\n') : 'No agent runs yet.');
    send({ type: 'exit', code: 0 });
    end();
  },
};

// ---------- window side ----------

// Codex's models for this account, from its app-server (model/list): ids for -m, and their
// names. Kept for ten minutes. Claude has no such list; the window offers its model aliases.
let codexModelCache = { at: 0, models: null };
function codexModels() {
  if (codexModelCache.models && Date.now() - codexModelCache.at < 10 * 60e3) return Promise.resolve(codexModelCache.models);
  return new Promise((resolve) => {
    const [cmd, args] = deps.shellCommand([deps.getConfig().codexCommand || 'codex', 'app-server']);
    let child;
    try {
      child = spawn(cmd, args, { env: deps.cleanEnv(), windowsHide: true, windowsVerbatimArguments: process.platform === 'win32', stdio: ['pipe', 'pipe', 'ignore'] });
    } catch { resolve(null); return; }
    let buf = '';
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      if (child.pid && process.platform === 'win32') spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true });
      else { try { child.kill(); } catch { /* gone */ } }
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
        if (m.id === 1) { send({ method: 'initialized' }); send({ id: 2, method: 'model/list', params: {} }); }
        else if (m.id === 2) {
          const list = (m.result?.data || []).filter((x) => !x.hidden).map((x) => ({
            value: x.model || x.id, label: x.displayName || x.id, description: x.description || '', isDefault: !!x.isDefault,
            efforts: (x.supportedReasoningEfforts || []).map((e) => e.reasoningEffort).filter(Boolean), defaultEffort: x.defaultReasoningEffort || '',
          }));
          if (list.length) codexModelCache = { at: Date.now(), models: list };
          finish(list.length ? list : null);
        }
      }
    });
    send({ id: 1, method: 'initialize', params: { clientInfo: { name: 'crew', title: 'Crew', version: app.getVersion() }, capabilities: { experimentalApi: true } } });
  });
}

ipcMain.handle('agents:list', () => allAgents());
ipcMain.handle('agents:codex-models', () => codexModels());
ipcMain.handle('agents:save', (_e, input) => {
  const list = allAgents();
  const name = cleanName(input.name);
  if (!name) return { error: 'Give the agent a name (no spaces), e.g. CodeReviewerAgent.' };
  if (list.some((a) => a.id !== input.id && a.name.toLowerCase() === name.toLowerCase())) return { error: `There's already an agent named ${name}.` };
  const a = list.find((x) => x.id === input.id) || { id: crypto.randomUUID() };
  const cli = input.cli === 'codex' ? 'codex' : /^provider:/.test(input.cli || '') && deps.hasProvider(input.cli.slice(9)) ? input.cli : 'claude';
  Object.assign(a, { name, cli, model: String(input.model || '').trim(), effort: /^[a-z]{2,10}$/.test(input.effort || '') ? input.effort : '', instructions: String(input.instructions || '').trim(), access: ['read', 'edit', 'full'].includes(input.access) ? input.access : 'read' });
  if (!list.includes(a)) list.push(a);
  saveAgents();
  return a;
});
ipcMain.handle('agents:delete', (_e, id) => { agents = allAgents().filter((a) => a.id !== id); saveAgents(); });
ipcMain.handle('agents:runs', () => [...runs.values()].map(publicRun));
ipcMain.handle('agents:stop', (_e, id) => { const r = runs.get(id); if (r) stopRun(r); });

function init(d) { deps = d; }
function shutdown() { for (const r of runs.values()) stopRun(r); }

module.exports = { init, shutdown, ops };
