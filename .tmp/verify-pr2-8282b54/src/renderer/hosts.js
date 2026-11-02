/* global api, $, el, showMenu, sessionTitle, state, AGENTS, relTime, norm, leaves, leafOf, focusPane, splitPane, activate, respawn,
   makeTermEntry, newSession, openSessionRow, sessionMenu, statusEl, safeFit, renderSidebar, screenLines, SSH_KIND */
// SSH hosts: the list in the sidebar, the host editor, and what Crew asks while an agent's
// crew-ssh call waits (host-key trust, passwords, approval of commands). The hosts and their
// secrets live in the main process (ssh.js); this side only ever sees display data.

const hostListEl = $('#host-list');
const hostIcon = '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="7" rx="1.5"/><rect x="3" y="13" width="18" height="7" rx="1.5"/><path d="M7 7.5h.01M7 16.5h.01"/></svg>';
let hostsList = [];
let hostsCollapsed = localStorage.getItem('hostsCollapsed') === '1';

const hostAddress = (h) => `${h.user ? `${h.user}@` : ''}${h.host}${h.port && h.port !== 22 ? `:${h.port}` : ''}`;

async function loadHosts() {
  hostsList = await api.listHosts();
  // Host workspaces leave the Projects list (this renders the hosts too). Before boot has the
  // config, only the hosts can be drawn; boot's own render does the rest.
  if (state.config) renderSidebar(); else renderHosts();
}

function renderHosts() {
  $('#hosts-head').classList.toggle('collapsed', hostsCollapsed);
  hostListEl.classList.toggle('hidden', hostsCollapsed);
  if (hostsCollapsed) return;
  if (!hostsList.length) {
    hostListEl.replaceChildren(el('div', { class: 'host-empty' }, 'Add a server here and the agents can manage it with crew-ssh. ',
      el('a', { onclick: () => editHost(null) }, 'Add a host'), ' or ', el('a', { onclick: importHosts }, 'import ~/.ssh/config'), '.'));
    return;
  }
  const groups = new Map();
  for (const h of [...hostsList].sort((a, b) => a.name.localeCompare(b.name))) {
    const g = h.group || '';
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(h);
  }
  const rows = [];
  for (const [g, list] of [...groups].sort(([a], [b]) => (a ? 1 : 0) - (b ? 1 : 0) || a.localeCompare(b))) {
    if (g) rows.push(el('div', { class: 'host-group' }, g));
    for (const h of list) rows.push(...hostRows(h));
  }
  hostListEl.replaceChildren(...rows);
}

// A host, and under it (unless folded) its terminal and its agent sessions, like a project.
function hostRows(h) {
  const open = !hostFolded.has(h.id);
  const shownHere = state.terms.has(termKey(h)) && leaves().some((l) => l.session && hostOfSession(l.session)?.id === h.id);
  const row = el('div', {
    class: `row-item host-row${h.connected ? ' connected' : ''}${shownHere ? ' shown' : ''}${open ? '' : ' collapsed'}`,
    title: `${h.name}\n${hostAddress(h)}\nClick to open its terminal and agent${h.notes ? `\n\nNotes for agents: ${h.notes}` : ''}`,
    onclick: () => openHost(h),
    oncontextmenu: (e) => { e.preventDefault(); hostMenu(e, h); },
  },
  el('span', { class: 'host-icon', html: hostIcon }),
  el('span', { class: 'label' }, h.name),
  el('span', { class: 'meta' }, hostAddress(h)),
  el('span', { class: 'actions' },
    el('button', { class: 'icon-btn small', title: 'New agent session on this host', html: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>', onclick: (e) => { e.stopPropagation(); newHostSessionMenu(e, h); } }),
    el('button', { class: 'icon-btn small', title: 'More', html: '<svg viewBox="0 0 24 24"><circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/></svg>', onclick: (e) => { e.stopPropagation(); hostMenu(e, h); } })),
  el('span', { class: 'host-fold', title: open ? 'Fold' : 'Unfold', html: '<svg class="chev" viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>', onclick: (e) => { e.stopPropagation(); toggleHostFold(h); } }));
  if (!open) return [row];

  const children = [];
  const term = state.terms.get(termKey(h));
  children.push(el('div', {
    class: `row-item session host-child host-term${state.active === termKey(h) ? ' active' : ''}${leafOf(termKey(h)) ? ' shown' : ''}`,
    title: `Your terminal on ${h.name}`,
    onclick: () => showHostTerminal(h),
  },
  term && !term.exited ? el('span', { class: 'dot', title: 'Connected' }) : null,
  el('span', { class: 'agent-icon', html: SSH_KIND.badge }),
  el('span', { class: 'label' }, 'Terminal')));
  for (const s of hostSessions(h)) {
    children.push(el('div', {
      class: `row-item session host-child${state.active === s.id ? ' active' : ''}${leafOf(s.id) ? ' shown' : ''}`,
      title: s.title || '',
      onclick: () => openHost(h, { session: s }),
      oncontextmenu: (e) => { e.preventDefault(); sessionMenu(e, { path: h.workspace }, s); },
    },
    state.terms.has(s.id) && !state.terms.get(s.id).exited ? statusEl(state.terms.get(s.id)) : null,
    sessionIcon(s),
    el('span', { class: 'label' }, s.title || 'Untitled'),
    el('span', { class: 'meta' }, s.pending ? '' : relTime(s.mtime))));
  }
  return [row, el('div', { class: 'host-children' }, ...children)];
}

const hostFolded = new Set((() => { try { return JSON.parse(localStorage.getItem('hostFolded') || '[]'); } catch { return []; } })());
function toggleHostFold(h) {
  if (hostFolded.has(h.id)) hostFolded.delete(h.id); else hostFolded.add(h.id);
  localStorage.setItem('hostFolded', JSON.stringify([...hostFolded]));
  renderHosts();
}

// ---------- a host's workspace: its terminal beside an agent ----------

const termKey = (h) => `ssh:${h.id}`;
const hostById = (id) => hostsList.find((h) => h.id === id);

function isHostWorkspace(p) {
  return !!p && hostsList.some((h) => h.workspace && norm(h.workspace) === norm(p));
}

// The host a pane's session belongs to: its terminal, or an agent running in its workspace.
function hostOfSession(id) {
  if (id.startsWith('ssh:')) return hostById(id.slice(4));
  const entry = state.terms.get(id);
  const p = entry?.projectPath || state.pendingNew.get(id)?.projectPath;
  return p ? hostsList.find((h) => h.workspace && norm(h.workspace) === norm(p)) : null;
}

function hostTerminalTitle(id) {
  const h = hostById(id.slice(4));
  return h ? `${h.name} · terminal` : 'SSH terminal';
}

// Agent sessions run in the host's workspace, newest first; new ones not on disk yet on top.
function hostSessions(h) {
  if (!h.workspace) return [];
  const p = state.projects.find((x) => norm(x.path) === norm(h.workspace));
  const pending = [...state.pendingNew]
    .filter(([id, pn]) => norm(pn.projectPath) === norm(h.workspace) && !p?.sessions.some((s) => s.id === id))
    .map(([id, pn]) => ({ id, agent: pn.agent, title: sessionTitle(id), mtime: Date.now(), pending: true }));
  return [...pending, ...(p?.sessions || [])];
}

// Called by respawn() for SSH terminals: (re)connects the shell. The id is chosen here, so the
// host's first output (its banner, the prompt) can't arrive before the pane knows about it.
async function connectHostShell(sessionId, entry) {
  const h = hostById(sessionId.slice(4));
  if (!h) { entry.term.write('\r\n\x1b[31mThis host is no longer saved in Crew.\x1b[0m\r\n'); entry.exited = true; return; }
  const termId = crypto.randomUUID();
  entry.termId = termId;
  entry.exited = false;
  entry.where = hostAddress(h);
  state.termToSession.set(termId, sessionId);
  safeFit(entry);
  entry.term.write(`\x1b[90mConnecting to ${h.name} (${hostAddress(h)})…\x1b[0m\r\n`);
  renderHosts();
  const r = await api.sshShell({ hostId: h.id, termId, cols: entry.term.cols, rows: entry.term.rows });
  if (r.error) {
    state.termToSession.delete(termId);
    entry.termId = null;
    entry.exited = true;
    entry.term.write(`\x1b[31m${r.error}\x1b[0m\r\n\x1b[90m[Press Enter to try again]\x1b[0m\r\n`);
  }
  renderSidebar();
  renderHosts();
}

async function ensureHostTerminal(h) {
  const id = termKey(h);
  const existing = state.terms.get(id);
  if (existing) {
    if (existing.exited) await respawn(id);
    activate(id);
    return;
  }
  const entry = makeTermEntry(id, { projectPath: h.workspace, cwd: h.workspace, agent: 'ssh' });
  entry.where = hostAddress(h);
  activate(id);
  await respawn(id);
}

async function showHostTerminal(h) {
  const id = termKey(h);
  const shown = leafOf(id);
  if (shown) { focusPane(shown); return; }
  await ensureHostTerminal(h);
}

// Opens the host's workspace: its terminal, and beside it an agent session (the given one, else
// its latest, else a new one), each in a pane of its own.
async function openHost(h, { session = null, agent = null, provider = null } = {}) {
  h.workspace = (await api.hostWorkspace(h.id)) || h.workspace; // also refreshes CLAUDE.md / AGENTS.md
  const target = session || (agent ? null : hostSessions(h)[0]) || null;

  // The terminal: where it's shown already, else in the focused pane.
  let termLeaf = leafOf(termKey(h));
  if (!termLeaf) {
    await ensureHostTerminal(h);
    termLeaf = leafOf(termKey(h));
  }
  if (target && leafOf(target.id)) { focusPane(leafOf(target.id)); return; }

  // The agent: in another pane of this host if there is one, else a new pane to the right.
  const other = leaves().find((l) => l !== termLeaf && l.session && hostOfSession(l.session)?.id === h.id)
    || (leaves().length > 1 ? leaves().find((l) => l !== termLeaf && !l.session) : null);
  if (other) state.focus = other.id;
  else { state.focus = termLeaf.id; splitPane('row', termLeaf); }

  const before = new Set(state.terms.keys());
  if (target) openSessionRow({ path: h.workspace }, target);
  else newSession(h.workspace, agent || state.config.defaultAgent || 'claude', provider);
  for (const id of state.terms.keys()) if (!before.has(id)) trustWorkspace(id, h);
}

// The agents ask whether to trust a new folder (Claude's prompt starts on "No, exit"). A host's
// workspace is Crew's own folder holding only CLAUDE.md / AGENTS.md, so Crew answers yes there,
// and nowhere else.
function trustWorkspace(sessionId, h) {
  const started = Date.now();
  let lastKey = 0;
  const tick = setInterval(() => {
    const entry = state.terms.get(sessionId);
    if (!entry || Date.now() - started > 20000) { clearInterval(tick); return; }
    if (norm(entry.cwd) !== norm(h.workspace) || !entry.termId) return;
    const text = screenLines(entry, 30).join('\n');
    if (!/trust (this|the files in this) (folder|directory)|Quick safety check|Do you trust/i.test(text)) return;
    if (Date.now() - lastKey < 500) return;
    lastKey = Date.now();
    const selected = /^\s*[❯›>]\s*(.*)$/m.exec(text)?.[1] || '';
    if (/^(\d\.\s*)?Yes/i.test(selected.trim())) { api.write(entry.termId, '\r'); clearInterval(tick); } else api.write(entry.termId, '\x1b[B');
  }, 300);
}

// "test-box · tester@203.0.113.10" for a host's sessions, where the header shows a folder.
function hostWhere(projectPath) {
  const h = isHostWorkspace(projectPath) && hostsList.find((x) => norm(x.workspace) === norm(projectPath));
  return h ? `${h.name} · ${hostAddress(h)}` : null;
}
const hostNameFor = (projectPath) => (isHostWorkspace(projectPath) ? hostsList.find((x) => norm(x.workspace) === norm(projectPath))?.name : null);

// A host's agent sessions: Claude, Codex, or a custom provider (Crew's own agent, which gets the
// host's instructions from the workspace's AGENTS.md like the others).
function hostAgentItems(h, onHost) {
  return [
    ...Object.entries(AGENTS).map(([agent, a]) => ({ label: `New ${a.label} session${onHost ? ` on ${h.name}` : ''}`, action: () => openHost(h, { agent }) })),
    ...state.providers.map((p, i) => ({ label: `New ${p.name} session${onHost ? ` on ${h.name}` : ''}`, divider: i === 0, action: () => openHost(h, { agent: p.cli, provider: p.id }) })),
  ];
}

// A session row's icon: its provider's, else Claude's or Codex's.
function sessionIcon(s) {
  const prov = providerOf(s.id);
  return prov ? el('span', { class: 'agent-icon' }, providerIcon(prov)) : el('span', { class: 'agent-icon', html: (AGENTS[s.agent] || AGENTS.claude).badge });
}

function newHostSessionMenu(e, h) {
  showMenu(e, hostAgentItems(h, true));
}

function hostMenu(e, h) {
  const items = [
    { label: 'Open terminal and agent', action: () => openHost(h) },
    ...hostAgentItems(h, false),
    { label: 'Edit', separator: true, action: () => editHost(h) },
    { label: 'Test connection', action: () => testHost(h) },
  ];
  if (h.connected) items.push({ label: 'Disconnect', action: () => api.disconnectHost(h.id) });
  items.push({ label: 'Copy name', action: () => navigator.clipboard.writeText(h.name) });
  items.push({ label: 'Delete', danger: true, action: () => deleteHost(h) });
  showMenu(e, items);
}

async function testHost(h) {
  toast(`Connecting to ${h.name}…`);
  const r = await api.testHost(h.id);
  toast(r.ok ? `${h.name}: connected · ${r.message}` : `${h.name}: ${r.message}`, r.ok ? 'ok' : 'error');
}

async function deleteHost(h) {
  if (!(await confirmBox(`Delete ${h.name}?`, 'Its saved password and settings are removed from Crew. The server itself isn\'t touched.', 'Delete'))) return;
  await api.deleteHost(h.id);
}

async function importHosts() {
  const r = await api.importHosts();
  toast(r.missing ? 'No ~/.ssh/config file was found.' : r.added ? `Imported ${r.added} host${r.added === 1 ? '' : 's'} from ~/.ssh/config.` : 'Every host in ~/.ssh/config is already here.', r.added ? 'ok' : '');
}

async function installSkill() {
  const done = await api.installSkill();
  toast(done.length ? `Installed the crew-ssh skill for ${done.map((p) => (/[\\/]\.codex[\\/]/.test(p) ? 'Codex' : 'Claude Code')).join(' and ')}. New sessions pick it up.` : 'Couldn\'t write the skill files.', done.length ? 'ok' : 'error');
}

$('#btn-add-host').addEventListener('click', (e) => { e.stopPropagation(); editHost(null); });
$('#btn-hosts-more').addEventListener('click', (e) => {
  e.stopPropagation();
  const r = e.currentTarget.getBoundingClientRect();
  showMenu({ clientX: r.left, clientY: r.bottom + 2 }, [
    { label: 'Add host…', action: () => editHost(null) },
    { label: 'Import from ~/.ssh/config', action: importHosts },
    { label: 'Teach Claude Code and Codex to use crew-ssh', separator: true, action: installSkill },
    { label: 'How agents use these hosts', action: explainHosts },
  ]);
});
$('#hosts-toggle').addEventListener('click', () => {
  hostsCollapsed = !hostsCollapsed;
  localStorage.setItem('hostsCollapsed', hostsCollapsed ? '1' : '0');
  renderHosts();
});

function explainHosts() {
  infoBox('How agents use these hosts', [
    'Sessions started in Crew have a crew-ssh command. Ask the agent things like "check the disk on web-1" or "restart nginx on the staging servers" and it runs crew-ssh with the host\'s name.',
    'While a host\'s terminal is open, the agent shares it with you: it reads what\'s on screen and types into it (commands, answers to prompts, keys like Ctrl+C), and you see everything it types. Passwords and codes asked in the terminal are yours to type.',
    'Crew signs in with the host\'s saved key, ssh-agent or password. The agent never sees credentials.',
    'Commands that only read (df, ps, systemctl status, journalctl…) run at once. Anything that could change something waits for you to approve it here. Change this per host.',
    'The first connection to a host shows its key fingerprint to trust. If the key later changes, Crew blocks the connection.',
    'Use "Teach Claude Code and Codex to use crew-ssh" once so the agents know the command without being told.',
  ]);
}

// ---------- the host editor ----------

const modalEl = $('#host-modal');
const closeHostModal = () => modalEl.classList.add('hidden');

function field(labelText, input, hint) {
  return el('label', {}, labelText, input, hint ? el('span', { class: 'field-hint' }, hint) : null);
}

function textInput(value, attrs = {}) {
  const i = el('input', { type: 'text', spellcheck: 'false', ...attrs });
  i.value = value ?? '';
  return i;
}

async function editHost(h) {
  const canSeal = await api.canSeal();
  const isNew = !h;
  h = h || { name: '', host: '', port: 22, user: '', auth: 'auto', keyPath: '', group: '', notes: '', approval: 'changes' };
  const card = modalEl.firstElementChild;
  const name = textInput(h.name, { placeholder: 'e.g. web-1' });
  const address = textInput(h.host, { placeholder: 'example.com or 203.0.113.10' });
  const port = textInput(h.port || 22, { inputmode: 'numeric' });
  const user = textInput(h.user, { placeholder: 'e.g. root or ubuntu' });
  const auth = el('select', {},
    el('option', { value: 'auto' }, 'Automatic (key, ssh-agent or saved password)'),
    el('option', { value: 'key' }, 'Private key file'),
    el('option', { value: 'agent' }, 'ssh-agent'),
    el('option', { value: 'password' }, 'Password'));
  auth.value = h.auth || 'auto';
  const keyPath = textInput(h.keyPath, { placeholder: 'Default: ~/.ssh/id_ed25519, id_ecdsa or id_rsa' });
  const browse = el('button', { class: 'small-btn', type: 'button', onclick: async () => { const p = await api.pickKey(); if (p) keyPath.value = p; } }, 'Browse…');
  const passphrase = textInput('', { type: 'password', placeholder: h.hasPassphrase ? 'Saved (type to replace)' : 'Only if the key has one' });
  const password = textInput('', { type: 'password', placeholder: h.hasPassword ? 'Saved (type to replace)' : 'Leave empty to be asked when needed' });
  const forget = { password: false, passphrase: false };
  const forgetLink = (key, labelText) => el('a', { class: 'forget', onclick: (e) => { forget[key] = true; e.target.replaceWith(el('span', { class: 'field-hint' }, 'Will be forgotten when you save.')); } }, labelText);
  const group = textInput(h.group, { placeholder: 'Optional, e.g. production' });
  const approval = el('select', {},
    el('option', { value: 'changes' }, 'Ask before anything that could change the host (recommended)'),
    el('option', { value: 'all' }, 'Ask before every command'),
    el('option', { value: 'auto' }, 'Run everything without asking'));
  approval.value = h.approval || 'changes';
  const notes = el('textarea', { rows: '3', spellcheck: 'false', placeholder: 'Instructions for agents on this host, e.g. "Production: never restart the database. The app lives in /srv/app and runs under pm2."' });
  notes.value = h.notes || '';
  const status = el('div', { class: 'host-status hidden' });

  const keyRow = el('div', { class: 'row' }, field('Key file', el('span', { class: 'with-btn' }, keyPath, browse)), field('Key passphrase', passphrase, h.hasPassphrase ? forgetLink('passphrase', 'Forget the saved passphrase') : null));
  const passRow = el('div', { class: 'row' }, field('Password', password, h.hasPassword ? forgetLink('password', 'Forget the saved password') : canSeal ? 'Saved encrypted with Windows data protection.' : 'This system can\'t encrypt it, so it isn\'t saved.'));
  const syncAuth = () => {
    keyRow.classList.toggle('hidden', !['auto', 'key'].includes(auth.value));
    passRow.classList.toggle('hidden', !['auto', 'password'].includes(auth.value));
  };
  auth.addEventListener('change', syncAuth);
  syncAuth();

  const values = () => ({
    id: h.id, name: name.value, host: address.value, port: port.value, user: user.value, auth: auth.value,
    keyPath: keyPath.value, group: group.value, notes: notes.value, approval: approval.value,
    password: canSeal ? password.value : '', passphrase: canSeal ? passphrase.value : '',
    clearPassword: forget.password, clearPassphrase: forget.passphrase,
  });
  const save = async () => {
    if (!address.value.trim()) { address.focus(); return null; }
    const saved = await api.saveHost(values());
    h = { ...h, ...saved };
    return saved;
  };
  const test = async () => {
    const saved = await save();
    if (!saved) return;
    status.className = 'host-status';
    status.textContent = `Connecting to ${saved.name}…`;
    const r = await api.testHost(saved.id);
    status.className = `host-status ${r.ok ? 'ok' : 'error'}`;
    status.textContent = r.ok ? `Connected: ${r.message}` : r.message;
  };

  const activityBox = el('div', { class: 'host-activity' });
  if (!isNew) {
    const items = await api.hostActivity(h.id);
    if (items.length) {
      activityBox.append(el('div', { class: 'host-activity-title' }, 'Recent agent commands'),
        ...items.map((a) => el('div', { class: `host-act ${a.decision}` },
          el('span', { class: 'when' }, relTime(a.at)),
          el('code', {}, a.command),
          el('span', { class: 'result' }, a.decision === 'denied' ? 'denied' : a.error ? 'failed' : `exit ${a.code}${a.decision === 'approved' ? ' · approved' : ''}`))));
    }
  }

  card.replaceChildren(
    el('h2', {}, isNew ? 'Add SSH host' : `Edit ${h.name}`),
    el('div', { class: 'row' }, field('Name', name, 'What you and the agents call it'), field('Group', group)),
    el('div', { class: 'row' }, field('Address', address), el('label', { class: 'narrow' }, 'Port', port), field('User', user)),
    field('Sign in with', auth),
    keyRow,
    passRow,
    field('Agents', approval),
    field('Notes for agents', notes),
    activityBox,
    status,
    el('div', { class: 'modal-actions' },
      isNew ? null : el('button', { class: 'danger-link', onclick: async () => { closeHostModal(); await deleteHost(h); } }, 'Delete'),
      el('span', { class: 'spacer' }),
      el('button', { onclick: test }, 'Test connection'),
      el('button', { onclick: closeHostModal }, 'Cancel'),
      el('button', { class: 'primary', onclick: async () => { if (await save()) closeHostModal(); } }, 'Save')));
  modalEl.classList.remove('hidden');
  requestAnimationFrame(() => (isNew ? address : name).focus());
}

modalEl.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') closeHostModal();
});

// ---------- small dialogs ----------

let toastTimer = null;
function toast(text, kind = '') {
  let t = $('#host-toast');
  if (!t) { t = el('div', { id: 'host-toast' }); document.body.append(t); }
  t.className = kind;
  t.textContent = text;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add('hidden'), kind === 'error' ? 8000 : 4000);
}

function confirmBox(title, text, ok) {
  return new Promise((resolve) => {
    const box = $('#ssh-prompt');
    const done = (v) => { box.classList.add('hidden'); resolve(v); };
    box.firstElementChild.replaceChildren(el('h2', {}, title), el('p', { class: 'prompt-text' }, text),
      el('div', { class: 'modal-actions' }, el('button', { onclick: () => done(false) }, 'Cancel'), el('button', { class: 'primary danger', onclick: () => done(true) }, ok)));
    box.classList.remove('hidden');
  });
}

function infoBox(title, paragraphs) {
  const box = $('#ssh-prompt');
  box.firstElementChild.replaceChildren(el('h2', {}, title), ...paragraphs.map((p) => el('p', { class: 'prompt-text' }, p)),
    el('div', { class: 'modal-actions' }, el('button', { class: 'primary', onclick: () => box.classList.add('hidden') }, 'OK')));
  box.classList.remove('hidden');
}

// ---------- what Crew asks while an agent's call waits ----------

// Host-key, password and 2FA prompts come one at a time in a dialog; approvals stack as cards.
const promptQueue = [];
let promptShown = null;

api.onSshPrompt((p) => {
  if (p.kind === 'approve') { showApproval(p); return; }
  promptQueue.push(p);
  if (!promptShown) nextPrompt();
});

api.onSshPromptCancel((id) => {
  document.querySelector(`.ssh-card[data-id="${id}"]`)?.remove();
  const i = promptQueue.findIndex((p) => p.id === id);
  if (i >= 0) promptQueue.splice(i, 1);
  if (promptShown?.id === id) { $('#ssh-prompt').classList.add('hidden'); promptShown = null; nextPrompt(); }
});

function nextPrompt() {
  const p = promptQueue.shift();
  promptShown = p || null;
  if (!p) return;
  const box = $('#ssh-prompt');
  const answer = (v) => { api.answerSsh(p.id, v); box.classList.add('hidden'); promptShown = null; nextPrompt(); };
  const card = box.firstElementChild;
  const actions = (...buttons) => el('div', { class: 'modal-actions' }, ...buttons);
  if (p.kind === 'hostkey') {
    card.replaceChildren(el('h2', {}, `Trust ${p.host}?`),
      el('p', { class: 'prompt-text' }, `This is the first connection to ${p.address}. Check that its ${p.type} key fingerprint matches the server's:`),
      el('code', { class: 'fingerprint' }, p.fingerprint),
      el('p', { class: 'prompt-text muted' }, 'On the server: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub'),
      actions(el('button', { onclick: () => answer(null) }, 'Cancel'), el('button', { class: 'primary', onclick: () => answer('trust') }, 'Trust and connect')));
  } else if (p.kind === 'hostkey-changed') {
    card.replaceChildren(el('h2', { class: 'danger-title' }, `${p.host}'s key has changed`),
      el('p', { class: 'prompt-text' }, `The ${p.type} key of ${p.address} isn't the one you trusted before. This happens when a server is reinstalled, but it can also mean someone is intercepting the connection. Crew blocked it.`),
      el('div', { class: 'fp-label' }, 'Trusted before'), el('code', { class: 'fingerprint' }, p.previous),
      el('div', { class: 'fp-label' }, 'Now'), el('code', { class: 'fingerprint' }, p.fingerprint),
      actions(el('button', { class: 'primary', onclick: () => answer(null) }, 'Keep blocking'), el('button', { class: 'danger-link', onclick: () => answer('trust') }, 'Replace the key and connect')));
  } else if (p.kind === 'password' || p.kind === 'passphrase') {
    const input = el('input', { type: 'password' });
    const remember = el('input', { type: 'checkbox' });
    const submit = () => answer({ value: input.value, remember: remember.checked });
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') submit(); });
    card.replaceChildren(el('h2', {}, p.kind === 'password' ? `Password for ${p.host}` : 'Key passphrase'),
      el('p', { class: 'prompt-text' }, p.kind === 'password' ? 'An agent is connecting to this host. The password stays in Crew.' : `For ${p.keyFile}, to connect to ${p.host}.`),
      input,
      el('label', { class: 'check' }, remember, 'Remember it (saved encrypted)'),
      actions(el('button', { onclick: () => answer(null) }, 'Cancel'), el('button', { class: 'primary', onclick: submit }, 'Connect')));
    requestAnimationFrame(() => input.focus());
  } else if (p.kind === 'keyboard') {
    const inputs = p.prompts.map((q) => el('input', { type: q.echo ? 'text' : 'password' }));
    const submit = () => answer(inputs.map((i) => i.value));
    card.replaceChildren(el('h2', {}, `Sign in to ${p.host}`),
      ...(p.instructions ? [el('p', { class: 'prompt-text' }, p.instructions)] : []),
      ...p.prompts.map((q, i) => el('label', {}, q.prompt, inputs[i])),
      actions(el('button', { onclick: () => answer(null) }, 'Cancel'), el('button', { class: 'primary', onclick: submit }, 'Continue')));
    inputs.at(-1)?.addEventListener('keydown', (e) => { if (e.key === 'Enter') submit(); });
    requestAnimationFrame(() => inputs[0]?.focus());
  }
  box.classList.remove('hidden');
}

function showApproval(p) {
  const who = p.agent && AGENTS[p.agent] ? AGENTS[p.agent].label : 'An agent';
  const session = p.session && state.terms.has(p.session) ? sessionTitle(p.session) : '';
  const answer = (v) => { api.answerSsh(p.id, v); card.remove(); };
  const verb = { upload: 'upload a file to', download: 'download a file from', input: 'type into your terminal on' }[p.action] || 'run on';
  const card = el('div', { class: 'ssh-card' },
    el('div', { class: 'ssh-card-head' },
      el('span', { class: 'host-icon', html: hostIcon }),
      el('span', {}, el('b', {}, who), session ? ` · ${session}` : '', ` wants to ${verb} `, el('b', {}, p.host))),
    el('pre', { class: 'ssh-cmd' }, p.command),
    p.cwd ? el('div', { class: 'ssh-cwd' }, `in ${p.cwd}`) : null,
    el('div', { class: 'ssh-card-actions' },
      el('button', { onclick: () => answer(null) }, 'Deny'),
      el('span', { class: 'spacer' }),
      el('button', { title: 'Run everything the agents send to this host for the next 15 minutes', onclick: () => answer('host') }, 'Allow 15 min'),
      el('button', { class: 'primary', onclick: () => answer('once') }, 'Allow')));
  card.dataset.id = p.id;
  $('#ssh-cards').append(card);
}

api.onHostsChanged(loadHosts);
loadHosts();
