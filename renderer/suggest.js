/* global api, $, el, state, inputEl, autosizeInput, activeEntry, agentsList, runnerInfo, providerOf, AGENTS */
// The prompt box's suggestions, as in T3 Code: @ for agents, files and folders, / for the agent's
// commands (at the start of a message), $ for its skills. Picking one puts its text in the box;
// it's sent to the agent's terminal like anything typed. Claude Code runs a skill only when
// /name opens the message, so a $skill in a Claude message is moved there on sending.

const suggestPop = $('#mention-pop');
let suggest = null; // { kind, start, caret, query, items, index }

// The agents' own commands. Custom commands and skills come from composer.js in the main process.
const BUILTIN_COMMANDS = {
  claude: [
    ['model', 'Switch the model'], ['effort', 'Set the reasoning effort'], ['compact', 'Summarize the conversation to free up context'],
    ['clear', 'Start over with an empty conversation'], ['context', 'Show what fills the context window'], ['resume', 'Resume an earlier conversation'],
    ['rewind', 'Go back to an earlier point in the conversation'], ['review', 'Review a pull request'], ['security-review', 'Review the pending changes for security problems'],
    ['init', 'Write a CLAUDE.md for this project'], ['memory', 'Edit memory files'], ['permissions', 'Allow or deny tools'],
    ['agents', 'Manage subagents'], ['mcp', 'Manage MCP servers'], ['plugin', 'Manage plugins'], ['hooks', 'Manage hooks'],
    ['add-dir', 'Add a folder Claude can work in'], ['export', 'Export the conversation'], ['usage', 'Show plan usage limits'],
    ['cost', 'Show this session\'s cost and duration'], ['status', 'Show version, model and account'], ['config', 'Open settings'],
    ['doctor', 'Check the installation'], ['help', 'Show help'],
  ],
  codex: [
    ['model', 'Switch the model and reasoning effort'], ['approvals', 'Choose what Codex may do without asking'], ['review', 'Review the current changes'],
    ['new', 'Start a new conversation'], ['resume', 'Resume an earlier conversation'], ['compact', 'Summarize the conversation to free up context'],
    ['diff', 'Show the Git diff'], ['mention', 'Mention a file'], ['init', 'Write an AGENTS.md for this project'],
    ['mcp', 'List MCP tools'], ['status', 'Show the session\'s settings and usage'],
  ],
};

const SCOPE_LABEL = { personal: 'Personal', project: 'Project', plugin: 'Plugin', system: 'System', admin: 'System', builtin: 'Built-in' };
const ICON = {
  file: '<svg viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
};
const HEADS = { path: 'Agents, files and folders', command: 'Commands', skill: 'Skills' };
const EMPTY = { path: 'No matching files or folders.', command: 'No matching command.', skill: 'No skills found. Try / to browse commands.' };

// ---------- what the agent offers ----------

const itemsCache = new Map(); // agent|project|provider -> { at, data | promise }
function agentItems(entry) {
  const agent = entry.agent === 'codex' ? 'codex' : 'claude';
  const prov = providerOf(state.active)?.id || '';
  const key = `${agent}|${entry.projectPath}|${prov}`;
  let c = itemsCache.get(key);
  if (!c || Date.now() - c.at > 60e3) {
    c = { at: Date.now(), data: c?.data || null };
    c.promise = api.composerItems(agent, entry.projectPath, prov || null).then((d) => { c.data = d; return d; }).catch(() => (c.data = { skills: [], commands: [] }));
    itemsCache.set(key, c);
  }
  return c;
}

// ---------- matching ----------

// Lower is better: the name exactly, from its start, at a word start, anywhere; then the description.
function score(name, description, q) {
  if (!q) return 10;
  const n = name.toLowerCase();
  if (n === q) return 0;
  if (n.startsWith(q)) return 1;
  if (new RegExp(`[-_:/.\\s]${q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`).test(n)) return 2;
  if (n.includes(q)) return 3;
  if ((description || '').toLowerCase().includes(q)) return 6;
  return null;
}

// Nothing typed yet keeps the list's own order; among equal matches, shorter paths come first.
function ranked(list, q, limit = 50) {
  if (!q) return list.slice(0, limit);
  return list.map((it) => ({ it, s: score(it.match ?? it.label, it.description, q) }))
    .filter((x) => x.s !== null)
    .sort((a, b) => a.s - b.s || (a.it.rel?.length || 0) - (b.it.rel?.length || 0) || a.it.label.localeCompare(b.it.label))
    .slice(0, limit).map((x) => x.it);
}

// The token being typed at the caret, and what it asks for.
function triggerAtCaret() {
  const v = inputEl.value;
  const caret = inputEl.selectionStart;
  if (caret !== inputEl.selectionEnd) return null;
  const before = v.slice(0, caret);
  const m = /(^|[\s(（，,])([@/$])([^\s@$]*)$/.exec(before);
  if (!m) return null;
  const start = caret - m[3].length - 1;
  const query = m[3];
  if (m[2] === '/') {
    // Commands work only at the start of a message; "/x/y" is a path, not a command.
    if (before.slice(0, start).trim() || /[/\\]/.test(query)) return null;
    return { kind: 'command', start, caret, query };
  }
  if (m[2] === '$') {
    if (/^\d/.test(query)) return null; // $5, a price
    return { kind: 'skill', start, caret, query };
  }
  return { kind: 'path', start, caret, query };
}

const skillRow = (s, codex, asCommand) => ({
  kind: 'skill', label: asCommand && !codex ? `/${s.name}` : `$${s.name}`, match: s.name,
  description: s.description, badge: SCOPE_LABEL[s.scope] || '', insert: asCommand && !codex ? `/${s.name} ` : `$${s.name} `,
});

let fileSeq = 0;
async function fileRows(entry, q) {
  const seq = ++fileSeq;
  const root = entry.projectPath;
  let rows = [];
  if (!q) {
    const r = await api.listFiles(root, '');
    rows = (r.items || []).filter((i) => !i.ignored).slice(0, 30).map((i) => ({ rel: i.rel, dir: i.dir }));
  } else {
    const r = await api.searchFiles(root, q, 'name');
    const files = (r.results || []).map((x) => x.rel);
    const folders = new Set();
    const needle = q.toLowerCase();
    for (const f of files) {
      const parts = f.split('/');
      for (let i = 1; i < parts.length; i++) if (parts[i - 1].toLowerCase().includes(needle)) folders.add(parts.slice(0, i).join('/'));
    }
    rows = [...[...folders].map((rel) => ({ rel, dir: true })), ...files.map((rel) => ({ rel, dir: false }))];
  }
  if (seq !== fileSeq) return null; // a newer search is on its way
  const items = rows.map(({ rel, dir }) => {
    const name = rel.split('/').pop();
    const ref = /\s/.test(rel) ? `"${rel}${dir ? '/' : ''}"` : `${rel}${dir ? '/' : ''}`;
    return { kind: dir ? 'folder' : 'file', label: name, match: name, description: rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '', insert: `@${ref} `, rel };
  });
  return q ? ranked(items, q.toLowerCase(), 40) : items;
}

// ---------- the list ----------

async function updateSuggest() {
  const entry = activeEntry();
  const t = entry && entry.agent !== 'ssh' && entry.agent !== 'file' ? triggerAtCaret() : null;
  if (!t) { hideSuggest(); return; }
  const q = t.query.toLowerCase();
  const codex = entry.agent === 'codex';
  let items = [];
  let loading = false;
  if (t.kind === 'path') {
    const agents = ranked(agentsList.map((a) => ({
      kind: 'agent', agent: a, label: `@${a.name}`, match: a.name, insert: `@${a.name} `,
      description: `${runnerInfo(a.cli).label} · ${{ read: 'reads', edit: 'edits', full: 'full access' }[a.access]}`,
    })), q, 8);
    show(t, agents, true);
    const files = await fileRows(entry, t.query);
    if (!files || !sameTrigger(t)) return;
    items = [...agents, ...files];
  } else {
    const c = agentItems(entry);
    if (!c.data) {
      loading = true;
      c.promise.then(() => { if (sameTrigger(t)) updateSuggest(); });
    }
    const data = c.data || { skills: [], commands: [] };
    if (t.kind === 'command') {
      const builtin = (BUILTIN_COMMANDS[codex ? 'codex' : 'claude']).map(([name, description]) => ({ kind: 'command', label: `/${name}`, match: name, description, badge: SCOPE_LABEL.builtin, insert: `/${name} `, builtin: true }));
      const custom = data.commands.map((cmd) => ({ kind: 'command', label: `/${cmd.name}`, match: cmd.name, description: cmd.description || cmd.hint, badge: SCOPE_LABEL[cmd.scope] || '', insert: `/${cmd.name} ` }));
      items = ranked([...builtin, ...custom, ...data.skills.map((s) => skillRow(s, codex, true))], q.replace(/^skill:/, ''));
    } else {
      items = ranked(data.skills.map((s) => skillRow(s, codex, false)), q);
    }
  }
  show(t, items, loading);
}

const sameTrigger = (t) => { const now = triggerAtCaret(); return now && now.kind === t.kind && now.start === t.start && now.query === t.query; };

function show(t, items, loading) {
  const index = suggest && suggest.kind === t.kind && suggest.start === t.start ? Math.min(suggest.index, Math.max(0, items.length - 1)) : 0;
  suggest = { ...t, items, index };
  const rows = items.map((it, i) => el('div', {
    class: `mention-item${i === index ? ' on' : ''}${it.builtin ? '' : ' own'}`, // own: not translated (i18n.js)
    onmousedown: (e) => { e.preventDefault(); pickSuggest(i); },
    onmousemove: () => { if (suggest.index !== i) { suggest.index = i; markOn(); } },
  },
  // Commands and skills have no icon: their / or $ says what they are.
  it.kind === 'agent' ? runnerInfo(it.agent.cli).icon() : ICON[it.kind] ? el('span', { class: 'sg-icon', html: ICON[it.kind] }) : null,
  el('b', {}, it.label),
  el('span', { class: 'muted' }, it.description || ''),
  it.badge ? el('span', { class: 'sg-badge' }, it.badge) : null));
  const body = rows.length ? rows : [el('div', { class: 'sg-empty' }, loading ? (t.kind === 'path' ? 'Searching files…' : 'Loading skills…') : EMPTY[t.kind])];
  suggestPop.replaceChildren(el('div', { class: 'sg-head' }, HEADS[t.kind]), el('div', { class: 'sg-list' }, ...body));
  suggestPop.classList.remove('hidden');
  suggestPop.querySelector('.mention-item.on')?.scrollIntoView({ block: 'nearest' });
}

function markOn() {
  suggestPop.querySelectorAll('.mention-item').forEach((r, i) => r.classList.toggle('on', i === suggest.index));
  suggestPop.querySelector('.mention-item.on')?.scrollIntoView({ block: 'nearest' });
}

function hideSuggest() {
  suggestPop.classList.add('hidden');
  suggest = null;
}

function pickSuggest(i) {
  const it = suggest?.items[i];
  if (!it) return;
  const v = inputEl.value;
  const before = v.slice(0, suggest.start) + it.insert;
  const after = v.slice(suggest.caret).replace(/^\S*/, ''); // the rest of the token being replaced
  inputEl.value = before + after.replace(/^ /, '');
  inputEl.selectionStart = inputEl.selectionEnd = before.length;
  hideSuggest();
  autosizeInput();
  inputEl.focus();
  inputEl.dispatchEvent(new Event('input')); // keep the draft and the send button up to date
}

let suggestTimer = null;
const scheduleSuggest = () => { clearTimeout(suggestTimer); suggestTimer = setTimeout(updateSuggest, 60); };
inputEl.addEventListener('input', scheduleSuggest);
inputEl.addEventListener('click', scheduleSuggest);
inputEl.addEventListener('blur', () => setTimeout(hideSuggest, 150));
// Capture phase on the box, so these keys reach the list before the prompt box sends or moves.
$('#composer').addEventListener('keydown', (e) => {
  if (!suggest || suggestPop.classList.contains('hidden') || e.target !== inputEl || e.isComposing) return;
  const n = suggest.items.length;
  if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && n) { suggest.index = (suggest.index + (e.key === 'ArrowDown' ? 1 : n - 1)) % n; markOn(); }
  else if ((e.key === 'Enter' || e.key === 'Tab') && n && !e.shiftKey) pickSuggest(suggest.index);
  else if (e.key === 'Escape') hideSuggest();
  else return;
  e.preventDefault();
  e.stopPropagation();
}, true);

// ---------- sending ----------

// Claude Code expands a skill only when /name opens the message, so the first $skill moves to
// the front (the rest of the message becomes its arguments) and any others become /name.
// Codex reads $skill where it stands.
function withSkills(entry, text) {
  if (entry.agent === 'codex' || !text.includes('$')) return text;
  const c = agentItems(entry);
  const names = new Set((c.data?.skills || []).map((s) => s.name));
  if (!names.size) return text;
  let first = null;
  const body = text.replace(/(^|\s)\$([\w:-]+)(?=\s|$)/g, (all, pre, name) => {
    if (!names.has(name)) return all;
    if (!first) { first = name; return pre; }
    return `${pre}/${name}`;
  });
  if (!first || /^\s*\//.test(text)) return text; // a /command already opens it
  return `/${first} ${body.replace(/[ \t]{2,}/g, ' ').trim()}`.trim();
}

// Loads the agent's list in the background when a session is shown, so the first / or $ is quick.
function warmSuggest() {
  const entry = activeEntry();
  if (entry && (entry.agent === 'claude' || entry.agent === 'codex')) agentItems(entry);
}
