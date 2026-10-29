/* global api, $, el, showMenu, state, AGENTS, CLAUDE_ALIASES, CLAUDE_EFFORTS, effortLabel, i18n, sessionTitle, toast, confirmBox, field, textInput, openSessionRow, inputEl, autosizeInput */
// Agents you can @-mention in a session to hand them work: the list in the sidebar and its
// editor, live cards for their runs, and @ completion in the prompt box. The main agent runs
// them with the crew command (crew.js); agents.js in the main process does the running.

const agentListEl = $('#agent-list');
let agentsList = [];
let agentsCollapsed = localStorage.getItem('agentsCollapsed') === '1';

const AGENT_ACCESS = {
  read: 'Only read',
  edit: 'Edit files and run commands in the project',
  full: 'Anything, without asking',
};

// Starting points for a new agent.
const AGENT_TEMPLATES = [
  { label: 'Code reviewer', name: 'CodeReviewerAgent', cli: 'codex', access: 'read', instructions: 'Review the code for bugs, security problems, missing error handling and unclear code. Don\'t change files. Report findings by severity, each with the file, line and a suggested fix. Say clearly if you found nothing serious.' },
  { label: 'Tester', name: 'TesterAgent', cli: 'claude', access: 'edit', instructions: 'Run the project\'s tests and linters. Add tests for anything important that has none. Report what passed, what failed and why.' },
  { label: 'Publisher', name: 'CodePublisherAgent', cli: 'claude', access: 'full', instructions: 'Publish the project: build it, commit and push the changes, tag a release or deploy as the project does it (read its README and scripts first). Stop and report instead of guessing when credentials or a step are missing.' },
  { label: 'Blank agent', name: '', cli: 'claude', access: 'read', instructions: '' },
];

async function loadAgents() {
  agentsList = await api.listAgents();
  renderAgents();
}

function renderAgents() {
  $('#agents-head').classList.toggle('collapsed', agentsCollapsed);
  agentListEl.classList.toggle('hidden', agentsCollapsed);
  if (agentsCollapsed) return;
  if (!agentsList.length) {
    agentListEl.replaceChildren(el('div', { class: 'host-empty' }, 'Add agents, then @-mention them in a session: "build this, then have @CodeReviewerAgent check it". ',
      el('a', { onclick: (e) => addAgentMenu(e) }, 'Add an agent'), '.'));
    return;
  }
  agentListEl.replaceChildren(...[...agentsList].sort((a, b) => a.name.localeCompare(b.name)).map((a) => el('div', {
    class: 'row-item agent-row',
    title: `@${a.name} · ${AGENTS[a.cli].label}${a.model ? ` (${a.model})` : ''} · ${AGENT_ACCESS[a.access]}${a.instructions ? `\n\n${a.instructions}` : ''}`,
    onclick: () => editAgent(a),
    oncontextmenu: (e) => {
      e.preventDefault();
      showMenu(e, [
        { label: 'Edit', action: () => editAgent(a) },
        { label: `Copy @${a.name}`, action: () => navigator.clipboard.writeText(`@${a.name}`) },
        { label: 'Delete', danger: true, action: () => deleteAgentAsk(a) },
      ]);
    },
  },
  el('span', { class: 'agent-icon', html: AGENTS[a.cli].badge }),
  el('span', { class: 'label' }, a.name),
  el('span', { class: 'meta' }, { read: 'reads', edit: 'edits', full: 'full access' }[a.access]))));
}

function addAgentMenu(e) {
  const r = e.currentTarget?.getBoundingClientRect?.();
  showMenu(r ? { clientX: r.left, clientY: r.bottom + 2 } : e, AGENT_TEMPLATES.map((t) => ({ label: t.label, action: () => editAgent(null, t) })));
}

$('#btn-add-agent').addEventListener('click', (e) => { e.stopPropagation(); addAgentMenu(e); });
$('#agents-toggle').addEventListener('click', () => {
  agentsCollapsed = !agentsCollapsed;
  localStorage.setItem('agentsCollapsed', agentsCollapsed ? '1' : '0');
  renderAgents();
});

async function deleteAgentAsk(a) {
  if (!(await confirmBox(`Delete ${a.name}?`, 'Sessions can no longer @-mention it. Its past runs are kept as sessions.', 'Delete'))) return;
  await api.deleteAgent(a.id);
  loadAgents();
}

const agentModal = $('#agent-modal');
const closeAgentModal = () => agentModal.classList.add('hidden');
agentModal.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeAgentModal(); });

function editAgent(a, template = null) {
  const isNew = !a;
  a = a || { ...template, id: undefined };
  const card = agentModal.firstElementChild;
  const name = textInput(a.name, { placeholder: 'e.g. CodeReviewerAgent (no spaces)' });
  const cli = el('select', {}, el('option', { value: 'claude' }, 'Claude Code'), el('option', { value: 'codex' }, 'Codex'));
  cli.value = a.cli || 'claude';
  const cliHint = el('span', { class: 'field-hint' });
  const model = el('select', {});
  const modelHint = el('span', { class: 'field-hint' });
  const installed = {}; // cli -> the agent:check result
  let pickedModel = a.model || '';
  model.addEventListener('change', () => { pickedModel = model.value; syncEfforts(); });

  // Reasoning effort: Claude's levels, or the ones the chosen Codex model supports.
  const effort = el('select', {});
  let pickedEffort = a.effort || '';
  let modelList = []; // the listed models, with their efforts (Codex)
  effort.addEventListener('change', () => { pickedEffort = effort.value; });
  function syncEfforts() {
    let levels = CLAUDE_EFFORTS;
    let defaultLabel = 'Default (your Claude Code setting)';
    if (cli.value === 'codex') {
      const m = modelList.find((x) => x.value === model.value) || modelList.find((x) => x.isDefault);
      levels = m?.efforts?.length ? m.efforts : ['low', 'medium', 'high', 'xhigh'];
      defaultLabel = m?.defaultEffort ? `Default (${effortLabel(m.defaultEffort)})` : 'Default (the model\'s)';
    }
    const opts = [el('option', { value: '' }, defaultLabel), ...levels.map((l) => el('option', { value: l }, effortLabel(l)))];
    if (pickedEffort && !levels.includes(pickedEffort)) pickedEffort = ''; // this model doesn't have it
    effort.replaceChildren(...opts);
    effort.value = pickedEffort;
  }

  // Which CLIs are installed: one that isn't says so, with how to install it.
  const syncCli = () => {
    for (const o of cli.options) {
      const info = installed[o.value];
      o.textContent = `${AGENTS[o.value].label === 'Claude' ? 'Claude Code' : 'Codex'}${info && !info.installed ? ' (not installed)' : ''}`;
    }
    const info = installed[cli.value];
    cliHint.textContent = !info ? 'Checking…' : info.installed ? 'Installed' : `Not installed. Install it with: ${info.installCommand}`;
    cliHint.classList.toggle('danger', !!info && !info.installed);
    saveBtn.disabled = !!info && !info.installed;
  };
  Promise.all(['claude', 'codex'].map(async (c) => { installed[c] = await api.checkAgent(c); })).then(syncCli);

  // The models to choose from: Claude's model aliases (always its latest of each), or what
  // Codex reports for this account. A saved model that's no longer listed stays selectable.
  let modelSeq = 0;
  const syncModels = async () => {
    const seq = ++modelSeq;
    const which = cli.value;
    const option = (value, label) => el('option', { value }, label);
    if (which === 'claude') {
      const list = Object.entries(CLAUDE_ALIASES).map(([value, label]) => ({ value, label }));
      fillModels(list, 'Default (your Claude Code model)');
      modelHint.textContent = 'Each one is the latest version of that model.';
      return;
    }
    model.replaceChildren(option(pickedModel, pickedModel ? `${pickedModel} (loading…)` : 'Loading models…'));
    model.disabled = true;
    modelHint.textContent = '';
    const list = await api.codexModels();
    if (seq !== modelSeq) return;
    model.disabled = false;
    if (!list) {
      fillModels([], 'Default (your Codex model)');
      modelHint.textContent = installed.codex && !installed.codex.installed ? '' : 'Couldn\'t load Codex\'s models; it uses its default.';
      return;
    }
    fillModels(list, `Default (${list.find((m) => m.isDefault)?.label || 'your Codex model'})`);
    modelHint.textContent = '';
  };
  function fillModels(list, defaultLabel) {
    modelList = list;
    const opts = [el('option', { value: '' }, defaultLabel), ...list.map((m) => el('option', { value: m.value, title: m.description || '' }, m.label))];
    if (pickedModel && !list.some((m) => m.value === pickedModel)) opts.push(el('option', { value: pickedModel }, `${pickedModel} (saved)`));
    model.replaceChildren(...opts);
    model.value = pickedModel;
    syncEfforts();
  }
  cli.addEventListener('change', () => { pickedModel = ''; syncCli(); syncModels(); });
  syncModels();
  const access = el('select', {}, ...Object.entries(AGENT_ACCESS).map(([v, l]) => el('option', { value: v }, l)));
  access.value = a.access || 'read';
  const warn = el('div', { class: 'field-hint agent-warn' });
  const syncWarn = () => {
    warn.textContent = access.value === 'full' ? 'It runs any command and changes anything without asking you, including outside the project. Give this only to agents you trust with that, like a publisher.'
      : access.value === 'edit' ? (cli.value === 'codex' ? 'Codex\'s sandbox keeps it to the project folder.' : 'Claude\'s auto mode lets it act, and blocks risky actions.')
        : 'It can look but not change anything.';
    warn.classList.toggle('danger', access.value === 'full');
  };
  access.addEventListener('change', syncWarn);
  cli.addEventListener('change', syncWarn);
  syncWarn();
  const instructions = el('textarea', { rows: '6', spellcheck: 'false', placeholder: 'What this agent does and how, e.g. "Review the code for bugs and security problems. Don\'t change files; report findings."' });
  // A template's instructions in the interface's language (an editable value: i18n.js leaves those).
  instructions.value = (isNew && a.instructions ? i18n.t(a.instructions) : a.instructions) || '';
  const error = el('div', { class: 'host-status error hidden' });
  const saveBtn = el('button', { class: 'primary', onclick: () => save() }, 'Save');
  const save = async () => {
    const r = await api.saveAgent({ id: a.id, name: name.value, cli: cli.value, model: model.value, effort: effort.value, access: access.value, instructions: instructions.value });
    if (r.error) { error.textContent = r.error; error.classList.remove('hidden'); return; }
    closeAgentModal();
    loadAgents();
  };
  card.replaceChildren(
    el('h2', {}, isNew ? 'Add agent' : `Edit ${a.name}`),
    el('div', { class: 'row' }, field('Name', name, 'What you @-mention'), el('label', {}, 'Runs on', cli, cliHint)),
    el('div', { class: 'row' }, el('label', {}, 'Model', model, modelHint), field('Reasoning effort', effort)),
    field('Can', access),
    warn,
    field('Instructions', instructions),
    el('p', { class: 'field-hint' }, 'Mention it in a session, e.g. "Add a pricing page, then have @', a.name || 'ThisAgent', ' review it." The main agent hands it the work in the same folder, one after another or several at once, and gets its answer back.'),
    error,
    el('div', { class: 'modal-actions' },
      isNew ? null : el('button', { class: 'danger-link', onclick: () => { closeAgentModal(); deleteAgentAsk(a); } }, 'Delete'),
      el('span', { class: 'spacer' }),
      el('button', { onclick: closeAgentModal }, 'Cancel'),
      saveBtn));
  agentModal.classList.remove('hidden');
  requestAnimationFrame(() => name.focus());
}

// ---------- live cards for runs ----------

const runCardEls = new Map(); // run id -> card
const runInfo = new Map(); // run id -> latest run

function runElapsed(r) {
  const s = Math.max(0, Math.round(((r.endedAt || Date.now()) - r.startedAt) / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

function renderRunCard(r) {
  let card = runCardEls.get(r.id);
  if (!card) {
    card = el('div', { class: 'run-card' });
    runCardEls.set(r.id, card);
    $('#run-cards').prepend(card);
  }
  const caller = r.caller && state.terms.has(r.caller) ? sessionTitle(r.caller) : '';
  const buttons = [];
  if (r.status === 'running') buttons.push(el('button', { onclick: () => api.stopRun(r.id) }, 'Stop'));
  else buttons.push(el('button', { onclick: () => showRunResult(r) }, r.status === 'done' ? 'Result' : 'Details'));
  if (r.cli === 'claude' && r.sessionId && r.status !== 'running') {
    buttons.push(el('button', { title: 'Continue this run as a normal session', onclick: () => openSessionRow({ path: r.cwd }, { id: r.sessionId, agent: 'claude', cwd: r.cwd }) }, 'Open'));
  }
  card.className = `run-card ${r.status}`;
  card.replaceChildren(...[
    el('div', { class: 'run-head' },
      el('span', { class: 'agent-icon', html: AGENTS[r.cli].badge }),
      el('b', {}, r.agent),
      el('span', { class: 'run-state' }, r.status === 'running' ? el('span', { class: 'spinner' }) : null, ` ${{ running: 'Working', done: 'Done', failed: 'Failed', stopped: 'Stopped' }[r.status]} · `, el('span', { class: 'run-time' }, runElapsed(r))),
      el('button', { class: 'icon-btn small run-x', title: 'Dismiss', html: '<svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>', onclick: () => dismissRun(r.id) })),
    el('div', { class: 'run-task', title: r.task }, r.task.replace(/\s+/g, ' ')),
    el('div', { class: 'run-activity', title: r.activity }, r.activity),
    caller ? el('div', { class: 'run-caller' }, `for ${caller}`) : null,
    el('div', { class: 'run-actions' }, ...buttons)].filter(Boolean));
  // Finished cards go away by themselves after a while, unless the pointer is on them.
  clearTimeout(card.dismissTimer);
  if (r.status !== 'running') card.dismissTimer = setTimeout(function later() { if (card.matches(':hover')) card.dismissTimer = setTimeout(later, 5000); else dismissRun(r.id); }, 120000);
}

function dismissRun(id) {
  runCardEls.get(id)?.remove();
  runCardEls.delete(id);
}

function showRunResult(r) {
  const box = $('#ssh-prompt');
  box.firstElementChild.replaceChildren(
    el('h2', {}, `${r.agent}: ${r.status === 'done' ? 'result' : r.status}`),
    el('p', { class: 'prompt-text muted' }, r.task.length > 300 ? `${r.task.slice(0, 300)}…` : r.task),
    el('pre', { class: 'run-result' }, (r.status === 'done' ? r.result : [r.error, r.result].filter(Boolean).join('\n\n')) || '(no answer)'),
    el('div', { class: 'modal-actions' },
      el('button', { onclick: () => navigator.clipboard.writeText(r.result || r.error || '') }, 'Copy'),
      el('button', { class: 'primary', onclick: () => box.classList.add('hidden') }, 'Close')));
  box.classList.remove('hidden');
}

api.onRun((r) => { runInfo.set(r.id, r); renderRunCard(r); });
setInterval(() => {
  for (const [id, card] of runCardEls) {
    const r = runInfo.get(id);
    if (r?.status === 'running') { const t = card.querySelector('.run-time'); if (t) t.textContent = runElapsed(r); }
  }
}, 1000);

// ---------- @ in the prompt box ----------

const mentionPop = $('#mention-pop');
let mentionState = null; // { start, matches, index }

function mentionAtCaret() {
  const v = inputEl.value;
  const caret = inputEl.selectionStart;
  const m = /(^|[\s(（，,])@([\w-]*)$/.exec(v.slice(0, caret));
  return m ? { start: caret - m[2].length - 1, query: m[2].toLowerCase(), caret } : null;
}

function updateMentions() {
  const at = agentsList.length ? mentionAtCaret() : null;
  const matches = at ? agentsList.filter((a) => a.name.toLowerCase().includes(at.query)).slice(0, 8) : [];
  if (!at || !matches.length) { mentionPop.classList.add('hidden'); mentionState = null; return; }
  mentionState = { ...at, matches, index: Math.min(mentionState?.index || 0, matches.length - 1) };
  mentionPop.replaceChildren(...matches.map((a, i) => el('div', {
    class: `mention-item${i === mentionState.index ? ' on' : ''}`,
    onmousedown: (e) => { e.preventDefault(); pickMention(i); },
  }, el('span', { class: 'agent-icon', html: AGENTS[a.cli].badge }), el('b', {}, `@${a.name}`), el('span', { class: 'muted' }, ` · ${AGENTS[a.cli].label} · ${{ read: 'reads', edit: 'edits', full: 'full access' }[a.access]}`))));
  mentionPop.classList.remove('hidden');
}

function pickMention(i) {
  const a = mentionState.matches[i];
  const v = inputEl.value;
  const before = `${v.slice(0, mentionState.start)}@${a.name} `;
  inputEl.value = before + v.slice(mentionState.caret);
  inputEl.selectionStart = inputEl.selectionEnd = before.length;
  mentionPop.classList.add('hidden');
  mentionState = null;
  autosizeInput();
  inputEl.focus();
}

inputEl.addEventListener('input', updateMentions);
inputEl.addEventListener('click', updateMentions);
inputEl.addEventListener('blur', () => setTimeout(() => mentionPop.classList.add('hidden'), 150));
// Capture phase on the box, so these keys reach the list before the prompt box sends or moves.
$('#composer').addEventListener('keydown', (e) => {
  if (!mentionState || mentionPop.classList.contains('hidden') || e.target !== inputEl) return;
  const n = mentionState.matches.length;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') mentionState.index = (mentionState.index + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
  else if (e.key === 'Enter' || e.key === 'Tab') { pickMention(mentionState.index); }
  else if (e.key === 'Escape') { mentionPop.classList.add('hidden'); mentionState = null; }
  else return;
  e.preventDefault();
  e.stopPropagation();
  if (mentionState) updateMentions();
}, true);

// A message that @-mentions agents gets a short note for the main agent on how to run them.
function withAgentNote(text) {
  const names = agentsList.filter((a) => new RegExp(`@${a.name}(?![\\w-])`, 'i').test(text)).map((a) => a.name);
  if (!names.length) return text;
  const list = names.map((n) => `@${n}`).join(', ');
  return `${text}\n\n(Crew: ${list} ${names.length > 1 ? 'are agents' : 'is an agent'} you can hand work to, in this folder. Run one with crew ask <name> "<complete task with the context it needs>": it waits and prints the agent's answer. To run several at once, add --bg to each (it prints a run id), then crew wait <ids>. Follow my message for what runs when, one after another or together. Give these commands a 10-minute timeout; on exit status 3, run crew wait <id> again. See crew --help.)`;
}

loadAgents();
