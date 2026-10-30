/* global api, Terminal, FitAddon, WebLinksAddon */
// `api` is the global exposed by preload.js.

const $ = (sel) => document.querySelector(sel);
const projectListEl = $('#project-list');
const terminalsEl = $('#terminals');

const SESSIONS_SHOWN = 8;

const state = {
  config: null,
  projects: [],
  filter: '',
  terms: new Map(), // sessionId -> { termId, term, fit, el, projectPath, cwd, agent, startedAt, exited }
  termToSession: new Map(), // termId -> sessionId
  pendingNew: new Map(), // sessionId -> { projectPath, agent }, for new sessions not yet written to disk
  showAll: new Set(), // project paths with all sessions expanded
  active: null, // sessionId
  lastProject: null,
  renaming: null, // sessionId being renamed inline
  agentOk: new Set(), // agents whose CLI was found, so later sessions skip the check
};

// ---------- helpers ----------

const norm = (p) => (p || '').replace(/[\\/]+$/, '').toLowerCase();

function relTime(ms) {
  const s = (Date.now() - ms) / 1000;
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d`;
  return new Date(ms).toLocaleDateString();
}

function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') e.className = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else if (k === 'html') e.innerHTML = v;
    else e.setAttribute(k, v);
  }
  for (const c of children) if (c != null) e.append(c);
  return e;
}

const icons = {
  folder: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  chev: '<svg class="chev" viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>',
  plus: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
  dots: '<svg viewBox="0 0 24 24"><circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/></svg>',
};

const AGENTS = {
  claude: { label: 'Claude', group: 'Claude', badge: '<svg class="agent-badge claude" viewBox="0 0 24 24"><path d="M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6 5.6 18.4"/></svg>' },
  codex: { label: 'Codex', group: 'GPT', badge: '<svg class="agent-badge codex" viewBox="0 0 24 24"><path d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z"/></svg>' }, // OpenAI mark
};

// SSH terminals aren't agents, but show in panes like them.
const SSH_KIND = { label: 'SSH', group: 'Terminal', badge: '<svg class="agent-badge ssh" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M12 15h5"/></svg>' };
const agentInfo = (agent) => AGENTS[agent] || SSH_KIND;

function findProject(p) {
  return state.projects.find((x) => norm(x.path) === norm(p));
}

function findSession(sessionId) {
  for (const p of state.projects) {
    const s = p.sessions.find((x) => x.id === sessionId);
    if (s) return s;
  }
  return null;
}

function sessionTitle(sessionId) {
  if (sessionId?.startsWith('ssh:')) return hostTerminalTitle(sessionId); // hosts.js
  const s = findSession(sessionId);
  if (s) return s.title;
  const agent = state.terms.get(sessionId)?.agent || state.pendingNew.get(sessionId)?.agent;
  return `New ${AGENTS[agent]?.label || ''} session`.replace('  ', ' ');
}

// ---------- sidebar ----------

function renderSidebar() {
  // Background refreshes would recreate the rename box and lose what's typed.
  if (state.renaming && document.activeElement?.classList.contains('rename-input')) return;
  renderHosts(); // the hosts' terminals and sessions (hosts.js)
  const q = state.filter.toLowerCase();
  projectListEl.innerHTML = '';

  for (const p of state.projects) {
    if (isHostWorkspace(p.path)) continue; // listed under its host (hosts.js)
    const nameHit = !q || p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q);
    let sessions = p.sessions;
    if (q && !nameHit) sessions = sessions.filter((s) => (s.title || '').toLowerCase().includes(q));
    if (q && !nameHit && sessions.length === 0) continue;

    // New sessions that haven't been written to disk yet go on top.
    const pending = [...state.pendingNew]
      .filter(([id, pn]) => norm(pn.projectPath) === norm(p.path) && !p.sessions.some((s) => s.id === id))
      .map(([id, pn]) => ({ id, agent: pn.agent, title: sessionTitle(id), mtime: Date.now(), pending: true }));
    sessions = [...pending, ...sessions];

    const collapsed = !q && !!state.config.collapsed[p.path];
    const row = el('div', {
      class: `row-item project${collapsed ? ' collapsed' : ''}${p.exists ? '' : ' missing'}`,
      title: p.path,
      onclick: () => toggleProject(p.path),
      oncontextmenu: (e) => { e.preventDefault(); projectMenu(e, p); },
    });
    row.append(
      el('span', { html: icons.folder }),
      el('span', { class: 'label' }, p.name),
      el('span', { html: icons.chev }),
    );
    const actions = el('span', { class: 'actions' },
      el('button', { class: 'icon-btn small', title: 'New session', html: icons.plus, onclick: (e) => { e.stopPropagation(); newSessionMenu(e, p.path); } }),
      el('button', { class: 'icon-btn small', title: 'More', html: icons.dots, onclick: (e) => { e.stopPropagation(); projectMenu(e, p); } }),
    );
    row.insertBefore(actions, row.lastChild);
    projectDragHandlers(row, p);
    projectListEl.append(row);

    // Sessions are grouped by agent (Claude first, then GPT); each group folds and pages on its own.
    const list = el('div', { class: 'sessions' });
    for (const agent of Object.keys(AGENTS)) {
      const group = sessions.filter((s) => (s.agent || 'claude') === agent);
      if (group.length === 0) continue;
      const key = `${p.path}::${agent}`;
      const folded = !q && !!state.config.collapsed[key];
      const head = el('div', {
        class: `row-item agent-group${folded ? ' collapsed' : ''}`,
        onclick: () => toggleProject(key),
      },
      el('span', { class: 'agent-icon', html: AGENTS[agent].badge }),
      el('span', { class: 'label' }, AGENTS[agent].group),
      el('span', { class: 'meta' }, String(group.length)),
      el('span', { class: 'actions' },
        el('button', { class: 'icon-btn small', title: `New ${AGENTS[agent].label} session`, html: icons.plus, onclick: (e) => { e.stopPropagation(); newSession(p.path, agent); } })),
      el('span', { html: icons.chev }));
      list.append(head);
      if (folded) continue;
      const limit = q || state.showAll.has(key) ? Infinity : SESSIONS_SHOWN;
      group.slice(0, limit).forEach((s) => list.append(sessionRow(p, s)));
      if (group.length > limit) {
        list.append(el('div', { class: 'row-item session more', onclick: () => { state.showAll.add(key); renderSidebar(); } },
          `Show ${group.length - limit} more`));
      }
    }
    if (sessions.length === 0) list.append(el('div', { class: 'empty-note' }, 'No sessions yet'));
    projectListEl.append(list);
  }

  if (!projectListEl.children.length) {
    projectListEl.append(el('div', { class: 'empty-note' }, q ? 'No matches' : 'No projects yet — add a folder with +'));
  }
  updatePaneHeads();
}

// Projects are ordered by hand: drag a project row above or below another one.
const PROJECT_MIME = 'application/x-crew-project';

function projectDragHandlers(row, p) {
  row.draggable = true;
  row.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData(PROJECT_MIME, p.path);
    e.dataTransfer.effectAllowed = 'move';
  });
  const clear = () => row.classList.remove('drop-before', 'drop-after');
  row.addEventListener('dragover', (e) => {
    if (!e.dataTransfer.types.includes(PROJECT_MIME)) return;
    e.preventDefault();
    const r = row.getBoundingClientRect();
    const after = e.clientY > r.top + r.height / 2;
    row.classList.toggle('drop-after', after);
    row.classList.toggle('drop-before', !after);
  });
  row.addEventListener('dragleave', clear);
  row.addEventListener('drop', (e) => {
    const from = e.dataTransfer.getData(PROJECT_MIME);
    const after = row.classList.contains('drop-after');
    clear();
    if (!from) return;
    e.preventDefault();
    moveProject(from, p.path, after);
  });
}

// Move project `from` next to project `to` (after it, or before it), and save the order.
async function moveProject(from, to, after) {
  const paths = state.projects.map((x) => x.path).filter((x) => norm(x) !== norm(from));
  const i = paths.findIndex((x) => norm(x) === norm(to));
  if (i < 0) return;
  paths.splice(after ? i + 1 : i, 0, from);
  state.config = await api.setConfig({ projectOrder: paths });
  await refresh();
}

function shiftProject(p, delta) {
  const paths = state.projects.map((x) => x.path);
  const i = paths.findIndex((x) => norm(x) === norm(p.path));
  const j = Math.max(0, Math.min(paths.length - 1, i + delta));
  if (i < 0 || i === j) return;
  moveProject(p.path, paths[j], delta > 0);
}

function sessionRow(p, s) {
  const running = state.terms.has(s.id) && !state.terms.get(s.id).exited;
  const row = el('div', {
    class: `row-item session${state.active === s.id ? ' active' : ''}${leafOf(s.id) ? ' shown' : ''}`,
    title: s.title || '',
    onclick: () => state.renaming !== s.id && openSessionRow(p, s),
    ondblclick: () => !s.pending && startRename(s.id),
    oncontextmenu: (e) => { e.preventDefault(); sessionMenu(e, p, s); },
  });
  if (running) row.append(statusEl(state.terms.get(s.id)));
  if (state.renaming === s.id) {
    row.append(renameInput(s));
    return row;
  }
  // Drag a session onto a pane to show it there.
  row.draggable = true;
  row.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData(SESSION_MIME, JSON.stringify({ projectPath: p.path, id: s.id }));
    e.dataTransfer.effectAllowed = 'move';
  });
  row.append(el('span', { class: 'label' }, s.title || 'Untitled'));
  row.append(el('span', { class: 'meta' }, s.pending ? '' : relTime(s.mtime)));
  const actions = el('span', { class: 'actions' });
  if (running) {
    actions.append(el('button', { class: 'icon-btn small', title: 'Close', html: '<svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>', onclick: (e) => { e.stopPropagation(); closeSession(s.id); } }));
  }
  actions.append(el('button', { class: 'icon-btn small', title: 'More', html: icons.dots, onclick: (e) => { e.stopPropagation(); sessionMenu(e, p, s); } }));
  row.append(actions);
  return row;
}

async function toggleProject(p) {
  const collapsed = { ...state.config.collapsed, [p]: !state.config.collapsed[p] };
  if (!collapsed[p]) delete collapsed[p];
  state.config = await api.setConfig({ collapsed });
  renderSidebar();
}

// ---------- context menus ----------

const menuEl = $('#menu');
function showMenu(e, items, above) {
  menuEl.innerHTML = '';
  for (const it of items) {
    const cls = [it.danger && 'danger', it.checked && 'checked', it.separator && 'separator'].filter(Boolean).join(' ');
    menuEl.append(el('button', { class: cls, onclick: () => { hideMenu(); it.action(); } }, it.label));
  }
  menuEl.classList.remove('hidden');
  const r = menuEl.getBoundingClientRect();
  // `above`: a DOMRect to open the menu over, e.g. a composer chip.
  const x = above ? above.left : e.clientX;
  const y = above ? above.top - r.height - 6 : e.clientY;
  menuEl.style.left = `${Math.max(8, Math.min(x, innerWidth - r.width - 8))}px`;
  menuEl.style.top = `${Math.max(8, Math.min(y, innerHeight - r.height - 8))}px`;
}
function hideMenu() { menuEl.classList.add('hidden'); }
document.addEventListener('mousedown', (e) => { if (!menuEl.contains(e.target)) hideMenu(); });

function newSessionMenu(e, projectPath) {
  showMenu(e, Object.entries(AGENTS).map(([agent, a]) => ({
    label: `New ${a.label} session`, action: () => newSession(projectPath, agent),
  })));
}

function projectMenu(e, p) {
  showMenu(e, [
    { label: 'New Claude session', action: () => newSession(p.path, 'claude') },
    { label: 'New Codex session', action: () => newSession(p.path, 'codex') },
    { label: 'Move up', action: () => shiftProject(p, -1) },
    { label: 'Move down', action: () => shiftProject(p, 1) },
    { label: 'Move to top', action: () => shiftProject(p, -Infinity) },
    { label: 'Open in Git', action: () => openGit(p.path) },
    { label: 'Open folder', action: () => api.revealProject(p.path) },
    { label: 'Copy path', action: () => navigator.clipboard.writeText(p.path) },
    { label: 'Hide from list', danger: true, action: async () => { await api.hideProject(p.path); refresh(); } },
  ]);
}

function sessionMenu(e, p, s) {
  const items = [
    { label: 'Open', action: () => openSessionRow(p, s) },
    { label: 'Open in split right', action: () => openBeside(p, s, 'row') },
    { label: 'Open in split down', action: () => openBeside(p, s, 'column') },
  ];
  if (state.terms.has(s.id)) {
    items.push({ label: 'Restart', action: () => restartSession(s.id) });
    items.push({ label: 'Close', danger: true, action: () => closeSession(s.id) });
  }
  if (!s.pending) items.push({ label: 'Rename', action: () => startRename(s.id) });
  items.push({ label: 'Copy session ID', action: () => navigator.clipboard.writeText(s.id) });
  if (!s.pending) items.push({ label: 'Delete', danger: true, action: () => deleteSession(s) });
  showMenu(e, items);
}

// ---------- rename / delete ----------

function startRename(sessionId) {
  state.renaming = sessionId;
  renderSidebar();
}

function renameInput(s) {
  const input = el('input', { class: 'rename-input', type: 'text', spellcheck: 'false' });
  input.value = s.title || '';
  let done = false;
  const finish = async (save) => {
    if (done) return;
    done = true;
    state.renaming = null;
    const title = input.value.trim();
    if (save && title && title !== s.title) {
      s.title = title; // show it right away; the file watcher confirms it shortly
      await api.renameSession(s.id, title, s.agent);
      if (state.active === s.id) $('#head-title').textContent = title;
    }
    renderSidebar();
  };
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') finish(true);
    else if (e.key === 'Escape') finish(false);
    e.stopPropagation();
  });
  input.addEventListener('blur', () => finish(true));
  input.addEventListener('click', (e) => e.stopPropagation());
  requestAnimationFrame(() => { input.focus(); input.select(); });
  return input;
}

async function deleteSession(s) {
  if (!(await api.deleteSession(s.id, s.title, s.agent))) return;
  closeSession(s.id);
  await refresh();
}

// ---------- terminals ----------

const lightTheme = {
  background: '#ffffff', foreground: '#1f1f1f', cursor: '#1f1f1f', cursorAccent: '#ffffff',
  selectionBackground: '#d9d4cc',
  black: '#1f1f1f', red: '#c4462d', green: '#2f8a46', yellow: '#a06a00', blue: '#2f62c4', magenta: '#9d3fb3', cyan: '#1a8790', white: '#bdb8b0',
  brightBlack: '#77736d', brightRed: '#d9573c', brightGreen: '#3fa45b', brightYellow: '#b98200', brightBlue: '#4a7de0', brightMagenta: '#b55acb', brightCyan: '#2aa3ad', brightWhite: '#8d8984',
};
const darkTheme = {
  background: '#181818', foreground: '#e6e3de', cursor: '#e6e3de', cursorAccent: '#181818',
  selectionBackground: '#44403b',
};

function termOptions() {
  return {
    fontFamily: '"Cascadia Mono", "Cascadia Code", Consolas, "Microsoft YaHei Mono", "Microsoft YaHei", monospace',
    fontSize: state.config.fontSize || 14,
    lineHeight: 1.15,
    cursorBlink: true,
    allowProposedApi: true,
    scrollback: 10000,
    theme: state.config.theme === 'dark' ? darkTheme : lightTheme,
    // OSC 8 hyperlinks the agents print: file links open the image viewer / default app.
    linkHandler: { activate: (e, uri) => openLink(e, uri), allowNonHttpProtocols: true },
  };
}

function openSessionRow(p, s) {
  const agent = s.agent || state.pendingNew.get(s.id)?.agent || 'claude';
  openSession(p.path, s.id, !s.pending, { agent, cwd: s.cwd });
}

// A new session that never got a first prompt has nothing to resume.
function isOnDisk(sessionId) {
  return !!findSession(sessionId);
}

// `prompt`: a new session's first message, given to the agent as it starts.
async function openSession(projectPath, sessionId, resume, { agent = 'claude', cwd, prompt } = {}) {
  if (!isHostWorkspace(projectPath)) state.lastProject = projectPath; // "New session" stays on projects
  if (state.terms.has(sessionId)) {
    const t = state.terms.get(sessionId);
    if (t.exited) await respawn(sessionId, isOnDisk(sessionId));
    activate(sessionId);
    return;
  }

  makeTermEntry(sessionId, { projectPath, cwd, agent }).firstPrompt = prompt;
  activate(sessionId);
  if (!resume) state.pendingNew.set(sessionId, { projectPath, agent });
  await respawn(sessionId, resume);
  renderSidebar();
}

// A terminal for a session (an agent, or an SSH shell on a host), parked until a pane shows it.
function makeTermEntry(sessionId, { projectPath, cwd, agent }) {
  const container = el('div', { class: 'term' });
  termStash.append(container);
  const term = new Terminal(termOptions());
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  // Links open in the side browser; Ctrl+click opens them in the default browser.
  term.loadAddon(new WebLinksAddon.WebLinksAddon((e, uri) => openLink(e, uri)));
  term.registerLinkProvider(fileLinkProvider(term));
  term.open(container);

  const entry = {
    termId: null, term, fit, el: container, projectPath, cwd: cwd || projectPath, agent, startedAt: 0, exited: false,
    draft: '', attachments: [], fileSettings: {}, live: {}, overrides: {}, // composer state, see renderComposer()
  };
  state.terms.set(sessionId, entry);
  // Each pane resizes on its own when the layout is split.
  entry.resizeObserver = new ResizeObserver(() => safeFit(entry));
  entry.resizeObserver.observe(container);

  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== 'keydown') return true;
    // Ctrl+C copies when there is a selection, otherwise it goes to the agent.
    if (e.ctrlKey && !e.shiftKey && e.key === 'c' && term.hasSelection()) {
      navigator.clipboard.writeText(term.getSelection());
      term.clearSelection();
      return false;
    }
    if ((e.ctrlKey && e.key === 'v') || (e.ctrlKey && e.shiftKey && e.key === 'V')) {
      e.preventDefault();
      navigator.clipboard.readText().then((t) => t && term.paste(t));
      return false;
    }
    // Shift+Enter inserts a newline in the prompt (Claude reads Alt+Enter, Codex reads Ctrl+J).
    if (e.shiftKey && e.key === 'Enter' && entry.agent !== 'ssh') {
      if (entry.termId) api.write(entry.termId, entry.agent === 'codex' ? '\n' : '\x1b\r');
      return false;
    }
    if (e.key === 'Enter' && entry.exited) {
      respawn(currentId(entry), isOnDisk(currentId(entry)));
      return false;
    }
    return true;
  });
  term.onData((d) => { if (entry.termId && !entry.exited) api.write(entry.termId, d); });
  term.onWriteParsed(() => scheduleScreenRead(entry));
  term.onResize(({ cols, rows }) => { if (entry.termId) api.resize(entry.termId, cols, rows); });
  return entry;
}

// A new Codex session is re-keyed once its rollout file shows up, so look the id up by entry.
function currentId(entry) {
  for (const [id, t] of state.terms) if (t === entry) return id;
  return null;
}

async function respawn(sessionId, resume) {
  const entry = state.terms.get(sessionId);
  if (!entry) return;
  if (entry.termId) { api.kill(entry.termId); state.termToSession.delete(entry.termId); }
  entry.termId = null;
  if (entry.agent === 'ssh') { await connectHostShell(sessionId, entry); return; } // hosts.js
  if (!(await agentReady(entry))) return;
  hideMissing(entry);
  safeFit(entry);
  const { termId, startedAt } = await api.spawn({
    agent: entry.agent, cwd: entry.cwd, sessionId, resume, cols: entry.term.cols, rows: entry.term.rows,
    prompt: resume ? undefined : entry.firstPrompt,
  });
  entry.firstPrompt = undefined; // sent once
  entry.termId = termId;
  entry.startedAt = startedAt;
  entry.live = {};
  entry.exited = false;
  state.termToSession.set(termId, sessionId);
  renderComposer();
  renderSidebar();
}

// ---------- missing agent CLI ----------

// Crew runs the agents' own `claude` / `codex` commands. If one isn't installed, the pane shows
// what to install and can run the official installer, then starts the session.

async function agentReady(entry) {
  if (state.agentOk.has(entry.agent)) return true;
  const info = await api.checkAgent(entry.agent);
  if (info.installed) { state.agentOk.add(entry.agent); return true; }
  entry.exited = true;
  showMissing(entry, info);
  renderComposer();
  renderSidebar();
  return false;
}

function hideMissing(entry) {
  entry.missingEl?.remove();
  entry.missingEl = null;
}

function showMissing(entry, info, note = '') {
  hideMissing(entry);
  const actions = el('div', { class: 'missing-actions' },
    info.npmMissing
      ? el('button', { class: 'primary', onclick: () => window.open('https://nodejs.org/') }, 'Get Node.js')
      : el('button', { class: 'primary', onclick: () => installAgent(entry, info) }, `Install ${info.name}`),
    el('button', { onclick: () => window.open(info.page) }, 'Install guide'),
    el('button', { onclick: () => recheckAgent(entry) }, 'Check again'));
  if (note) actions.append(el('button', { onclick: () => hideMissing(entry) }, 'Show installer output'));
  entry.missingEl = el('div', { class: 'agent-missing' }, el('div', { class: 'missing-card' },
    el('h3', {}, `${info.name} isn't installed`),
    el('p', {}, 'Crew runs the official ', el('code', {}, info.command), ' command, and it wasn\'t found on this computer.'),
    el('p', {}, info.npmMissing
      ? `${info.name} installs with npm, which comes with Node.js. Install Node.js, then click Check again.`
      : 'It can be installed with:'),
    el('pre', {}, info.installCommand),
    note ? el('p', { class: 'missing-note' }, note) : null,
    actions,
    el('p', { class: 'hint' }, 'You sign in once, the first time the session starts. Installed somewhere else? Set the command in Settings.')));
  entry.el.append(entry.missingEl);
}

async function installAgent(entry, info) {
  hideMissing(entry);
  entry.term.reset();
  entry.term.write(`\x1b[90mInstalling ${info.name}: ${info.installCommand}\x1b[0m\r\n\r\n`);
  safeFit(entry);
  entry.installing = true;
  entry.exited = false;
  entry.termId = await api.installAgent({ agent: entry.agent, cols: entry.term.cols, rows: entry.term.rows });
  state.termToSession.set(entry.termId, currentId(entry));
  entry.term.focus();
  renderComposer();
}

async function finishInstall(entry, code) {
  const info = await api.checkAgent(entry.agent);
  const sessionId = currentId(entry);
  if (info.installed) {
    state.agentOk.add(entry.agent);
    entry.term.write(`\r\n\x1b[32m${info.name} is installed. Starting the session…\x1b[0m\r\n`);
    await sleep(1000);
    entry.term.reset();
    await respawn(sessionId, isOnDisk(sessionId));
    return;
  }
  entry.exited = true;
  entry.term.write('\r\n\x1b[90m[Press Enter to go back]\x1b[0m\r\n');
  showMissing(entry, info, code === 0
    ? `The installer finished, but the ${info.command} command still can't be found. Restarting Crew may help, or set the command in Settings.`
    : `The installer stopped with exit code ${code}.`);
  renderComposer();
}

function recheckAgent(entry) {
  state.agentOk.delete(entry.agent);
  const sessionId = currentId(entry);
  respawn(sessionId, isOnDisk(sessionId));
}

function newSession(projectPath, agent = state.config.defaultAgent || 'claude') {
  if (agent !== state.config.defaultAgent) api.setConfig({ defaultAgent: agent }).then((c) => { state.config = c; });
  state.config.defaultAgent = agent;
  openSession(projectPath, crypto.randomUUID(), false, { agent });
}

// Show a session in the focused pane, or focus the pane already showing it. `focusInput: false`
// leaves keyboard focus where it is (a click into a pane's terminal).
function activate(sessionId, { focusInput = true } = {}) {
  const prev = state.terms.get(state.active);
  if (prev && state.active !== sessionId) prev.draft = inputEl.value;
  const switching = state.active !== sessionId;
  if (sessionId) {
    const shown = leafOf(sessionId);
    if (shown) state.focus = shown.id;
    else { focusedLeaf().session = sessionId; renderLayout(); }
  }
  state.active = sessionId;
  markFocus();
  const entry = state.terms.get(sessionId);
  $('#empty').classList.toggle('hidden', !!entry || leaves().length > 1);
  $('#main-head').classList.toggle('idle', !entry);
  if (entry) {
    $('#head-title').textContent = sessionTitle(sessionId);
    $('#head-path').textContent = `${agentInfo(entry.agent).label} · ${entry.where || hostWhere(entry.projectPath) || entry.cwd}`;
    if (switching) { inputEl.value = entry.draft; autosizeInput(); renderAttachments(); }
    loadSettings(sessionId);
    requestAnimationFrame(() => { safeFit(entry); if (focusInput) (entry.agent === 'ssh' ? entry.term : inputEl).focus(); });
  } else {
    $('#head-title').textContent = '';
    $('#head-path').textContent = '';
    renderEmptyPicker();
  }
  renderComposer();
  renderSidebar();
  updateGitButton();
}

function closeSession(sessionId) {
  const entry = state.terms.get(sessionId);
  if (!entry) return;
  if (entry.termId) { api.kill(entry.termId); state.termToSession.delete(entry.termId); }
  entry.resizeObserver.disconnect();
  entry.term.dispose();
  entry.el.remove();
  state.terms.delete(sessionId);
  state.pendingNew.delete(sessionId);
  const leaf = leafOf(sessionId);
  if (leaf) {
    // With one pane, show another running session that isn't on screen; split panes just empty.
    leaf.session = leaves().length === 1 ? [...state.terms.keys()].filter((id) => !leafOf(id)).pop() || null : null;
    renderLayout();
  }
  if (state.active === sessionId) {
    state.active = null;
    activate(focusedLeaf().session);
  } else {
    renderSidebar();
  }
}

async function restartSession(sessionId) {
  const entry = state.terms.get(sessionId);
  if (!entry) return;
  entry.term.reset();
  await respawn(sessionId, isOnDisk(sessionId));
  activate(sessionId);
}

function safeFit(entry) {
  if (!entry.el.classList.contains('visible')) return;
  try { entry.fit.fit(); } catch { /* not laid out yet */ }
}

api.onData((termId, data) => {
  const id = state.termToSession.get(termId);
  if (id) state.terms.get(id)?.term.write(data);
});

api.onExit((termId, code) => {
  const id = state.termToSession.get(termId);
  if (!id) return;
  state.termToSession.delete(termId);
  const entry = state.terms.get(id);
  if (!entry || entry.termId !== termId) return;
  if (entry.installing) {
    entry.installing = false;
    entry.termId = null;
    finishInstall(entry, code);
    return;
  }
  entry.exited = true;
  entry.term.write(entry.agent === 'ssh'
    ? '\r\n\x1b[90m[Disconnected — press Enter to reconnect]\x1b[0m\r\n'
    : `\r\n\x1b[90m[Process exited with code ${code} — press Enter to resume]\x1b[0m\r\n`);
  renderComposer();
  renderSidebar();
});

// ---------- split panes ----------

// The terminal area is an editor-style layout: a tree of splits whose leaves are panes, each
// showing one session or nothing. The focused pane's session is state.active, so the header and
// the composer act on it. Sessions not on screen keep running, parked in termStash.

const SESSION_MIME = 'application/x-crew-session';
const termStash = el('div', { class: 'hidden' });
document.body.append(termStash);
const paneUi = new Map(); // leaf id -> { pane, title, icon }
let paneSeq = 0;

const newLeaf = (session = null) => ({ id: ++paneSeq, session });
state.layout = newLeaf(); // split: { dir: 'row' | 'column', ratio, children: [a, b] }
state.focus = state.layout.id;

function leaves(node = state.layout) {
  return node.children ? node.children.flatMap((c) => leaves(c)) : [node];
}
const focusedLeaf = () => leaves().find((l) => l.id === state.focus) || leaves()[0];
const leafOf = (sessionId) => (sessionId ? leaves().find((l) => l.session === sessionId) : undefined);

function parentOf(target, node = state.layout) {
  if (!node.children) return null;
  for (const c of node.children) {
    if (c === target) return node;
    const p = parentOf(target, c);
    if (p) return p;
  }
  return null;
}

function replaceNode(old, node) {
  const parent = parentOf(old);
  if (!parent) state.layout = node;
  else parent.children[parent.children.indexOf(old)] = node;
}

// 'row' puts the new pane to the right, 'column' below. The new pane starts empty and focused.
function splitPane(dir, leaf = focusedLeaf()) {
  const fresh = newLeaf();
  replaceNode(leaf, { dir, ratio: 0.5, children: [leaf, fresh] });
  state.focus = fresh.id;
  renderLayout();
  saveLayout();
  activate(null);
}

// The pane goes away; its session keeps running and can be reopened from the sidebar.
function closePane(leaf) {
  const parent = parentOf(leaf);
  if (!parent) return;
  const sibling = parent.children.find((c) => c !== leaf);
  replaceNode(parent, sibling);
  if (state.focus === leaf.id) state.focus = leaves(sibling)[0].id;
  renderLayout();
  saveLayout();
  activate(focusedLeaf().session);
}

function focusPane(leaf, opts) {
  state.focus = leaf.id;
  activate(leaf.session, opts);
}

function openBeside(p, s, dir) {
  const shown = leafOf(s.id);
  if (shown) shown.session = null; // move it rather than focus where it already is
  splitPane(dir);
  openSessionRow(p, s);
}

function openInPane(leaf, projectPath, id) {
  // Dropping a session that's in another pane swaps the two panes' sessions.
  const from = leafOf(id);
  if (from === leaf) return focusPane(leaf);
  if (from) from.session = leaf.session;
  leaf.session = null;
  state.focus = leaf.id;
  if (from) renderLayout();
  openSessionRow({ path: projectPath }, findSession(id) || { id, pending: true });
}

async function startInPane(leaf, agent) {
  state.focus = leaf.id;
  const p = state.lastProject || state.projects[0]?.path || (await api.addProject());
  if (!p) return;
  if (!findProject(p)) await refresh();
  newSession(p, agent);
}

function renderLayout() {
  const multi = leaves().length > 1;
  document.body.classList.toggle('split', multi);
  const shown = new Set(leaves().map((l) => l.session).filter((id) => state.terms.has(id)));
  // Moving a terminal in the DOM can reset its scroll, so remember where each one was.
  const scroll = new Map();
  for (const [id, t] of state.terms) {
    const b = t.term.buffer.active;
    scroll.set(id, b.viewportY === b.baseY ? null : b.viewportY);
    t.el.classList.toggle('visible', shown.has(id));
    if (!shown.has(id)) termStash.append(t.el);
  }
  paneUi.clear();
  terminalsEl.replaceChildren(buildNode(state.layout));
  requestAnimationFrame(() => {
    for (const id of shown) {
      const t = state.terms.get(id);
      if (!t) continue;
      safeFit(t);
      if (scroll.get(id) == null) t.term.scrollToBottom();
      else t.term.scrollToLine(scroll.get(id));
    }
  });
  markFocus();
  updatePaneHeads();
}

function buildNode(node) {
  if (!node.children) return paneEl(node);
  const box = el('div', { class: `pane-split ${node.dir}` });
  const [a, b] = node.children.map(buildNode);
  a.style.flex = `${node.ratio} 1 0`;
  b.style.flex = `${1 - node.ratio} 1 0`;
  box.append(a, el('div', { class: 'pane-divider', onmousedown: (e) => dragDivider(e, node, box, a, b) }), b);
  return box;
}

function paneEl(leaf) {
  const entry = state.terms.get(leaf.session);
  const btn = (title, svg, action) => el('button', {
    class: 'icon-btn', title, html: `<svg viewBox="0 0 24 24">${svg}</svg>`,
    onclick: (e) => { e.stopPropagation(); action(); },
  });
  const icon = el('span', { class: 'agent-icon' });
  const status = el('span', { class: 'pane-status' });
  const title = el('span', { class: 'pane-title' });
  const head = el('div', { class: 'pane-head' }, icon, status, title, el('span', { class: 'pane-actions' },
    btn('Split right', '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/>', () => splitPane('row', leaf)),
    btn('Split down', '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 12h18"/>', () => splitPane('column', leaf)),
    btn('Close pane (the session keeps running)', '<path d="M18 6 6 18M6 6l12 12"/>', () => closePane(leaf))));
  const body = el('div', { class: 'pane-body' });
  if (entry) body.append(entry.el);
  else {
    body.append(el('div', { class: 'pane-empty' },
      el('p', { class: 'hint' }, 'Pick a session on the left, or drag one here.'),
      el('div', { class: 'pane-empty-actions' }, ...Object.entries(AGENTS).map(([agent, a]) =>
        el('button', { onclick: () => startInPane(leaf, agent) }, `New ${a.label} session`)))));
  }
  const pane = el('div', { class: 'pane' }, head, body);
  // Capture phase, so a click into the terminal focuses the pane before xterm takes it.
  pane.addEventListener('mousedown', () => { if (state.focus !== leaf.id) focusPane(leaf, { focusInput: false }); }, true);
  pane.addEventListener('dragover', (e) => {
    if (!e.dataTransfer.types.includes(SESSION_MIME)) return;
    e.preventDefault();
    pane.classList.add('drop');
  });
  pane.addEventListener('dragleave', (e) => { if (!pane.contains(e.relatedTarget)) pane.classList.remove('drop'); });
  pane.addEventListener('drop', (e) => {
    const data = e.dataTransfer.getData(SESSION_MIME);
    pane.classList.remove('drop');
    if (!data) return;
    e.preventDefault();
    const { projectPath, id } = JSON.parse(data);
    openInPane(leaf, projectPath, id);
  });
  paneUi.set(leaf.id, { pane, title, icon, status });
  return pane;
}

function markFocus() {
  for (const [id, ui] of paneUi) ui.pane.classList.toggle('focused', id === state.focus);
}

function updatePaneHeads() {
  for (const leaf of leaves()) {
    const ui = paneUi.get(leaf.id);
    if (!ui) continue;
    const entry = state.terms.get(leaf.session);
    ui.title.textContent = entry ? sessionTitle(leaf.session) : 'Empty pane';
    ui.title.title = entry ? `${agentInfo(entry.agent).label} · ${entry.where || hostWhere(entry.projectPath) || entry.cwd}` : '';
    ui.icon.innerHTML = entry ? agentInfo(entry.agent).badge : '';
    ui.status.replaceChildren(...(entry && !entry.exited ? [statusEl(entry)] : []));
  }
}

function dragDivider(e, node, box, a, b) {
  e.preventDefault();
  const row = node.dir === 'row';
  document.body.classList.add('resizing', row ? 'resizing-col' : 'resizing-row');
  const move = (ev) => {
    const r = box.getBoundingClientRect();
    const ratio = row ? (ev.clientX - r.left) / r.width : (ev.clientY - r.top) / r.height;
    node.ratio = Math.min(0.85, Math.max(0.15, ratio));
    a.style.flex = `${node.ratio} 1 0`;
    b.style.flex = `${1 - node.ratio} 1 0`;
  };
  const up = () => {
    document.body.classList.remove('resizing', 'resizing-col', 'resizing-row');
    saveLayout();
    removeEventListener('mousemove', move);
    removeEventListener('mouseup', up);
  };
  addEventListener('mousemove', move);
  addEventListener('mouseup', up);
}

// The split shape is remembered across launches; the panes come back empty.
function saveLayout() {
  const shape = (n) => (n.children ? { dir: n.dir, ratio: n.ratio, children: n.children.map(shape) } : {});
  try { localStorage.setItem('paneLayout', JSON.stringify(shape(state.layout))); } catch { /* ignore */ }
}

function restoreLayout() {
  const build = (n) => (n?.children?.length === 2
    ? { dir: n.dir === 'column' ? 'column' : 'row', ratio: Math.min(0.85, Math.max(0.15, Number(n.ratio) || 0.5)), children: n.children.map(build) }
    : newLeaf());
  try {
    const saved = JSON.parse(localStorage.getItem('paneLayout') || 'null');
    if (saved) { state.layout = build(saved); state.focus = leaves()[0].id; }
  } catch { /* ignore */ }
}

$('#btn-split-right').addEventListener('click', () => splitPane('row'));
$('#btn-split-down').addEventListener('click', () => splitPane('column'));
// Capture phase, so the keys reach here before the terminal turns them into control characters.
document.addEventListener('keydown', (e) => {
  if (e.ctrlKey && !e.altKey && (e.code === 'Backslash')) {
    e.preventDefault();
    e.stopPropagation();
    splitPane(e.shiftKey ? 'column' : 'row');
  }
}, true);

// ---------- empty state ----------

function renderEmptyPicker() {
  const sel = $('#empty-project');
  const current = sel.value || state.lastProject;
  sel.innerHTML = '';
  for (const p of state.projects) if (!isHostWorkspace(p.path)) sel.append(el('option', { value: p.path }, `${p.name}  —  ${p.path}`));
  sel.append(el('option', { value: '__add__' }, 'Add a project folder…'));
  if (current && findProject(current)) sel.value = findProject(current).path;
  for (const b of document.querySelectorAll('#agent-toggle button')) {
    b.classList.toggle('on', b.dataset.agent === (state.config.defaultAgent || 'claude'));
  }
  $('#empty-start').textContent = `Start ${AGENTS[state.config.defaultAgent || 'claude'].label}`;
}

for (const b of document.querySelectorAll('#agent-toggle button')) {
  b.addEventListener('click', async () => {
    state.config = await api.setConfig({ defaultAgent: b.dataset.agent });
    renderEmptyPicker();
  });
}

$('#empty-start').addEventListener('click', async () => {
  let p = $('#empty-project').value;
  if (p === '__add__' || !p) {
    p = await api.addProject();
    if (!p) return;
    await refresh();
  }
  newSession(p);
});

// ---------- top-level actions ----------

// Codex chooses its own session id, so a new Codex terminal starts under a temporary id and is
// moved to the real one when a rollout file appears for the same folder after it started.
function adoptCodexSessions() {
  for (const [tempId, pn] of state.pendingNew) {
    if (pn.agent !== 'codex') continue;
    const entry = state.terms.get(tempId);
    if (!entry || !entry.startedAt) continue;
    const match = state.projects
      .flatMap((p) => p.sessions)
      .filter((s) => s.agent === 'codex' && norm(s.cwd) === norm(entry.cwd) && s.created >= entry.startedAt - 5000 && !state.terms.has(s.id))
      .sort((a, b) => a.created - b.created)[0];
    if (!match) continue;
    state.terms.delete(tempId);
    state.terms.set(match.id, entry);
    if (entry.termId) { state.termToSession.set(entry.termId, match.id); api.rekey(entry.termId, match.id); }
    state.pendingNew.delete(tempId);
    if (state.active === tempId) state.active = match.id;
    const leaf = leafOf(tempId);
    if (leaf) leaf.session = match.id;
  }
}

async function refresh() {
  state.projects = await api.listProjects();
  adoptCodexSessions();
  // Drop pending markers once the agent has written the session file.
  for (const [id] of state.pendingNew) {
    if (findSession(id)) state.pendingNew.delete(id);
  }
  renderSidebar();
  if (state.active) {
    $('#head-title').textContent = sessionTitle(state.active);
    loadSettings(state.active);
  } else {
    renderEmptyPicker();
  }
}

$('#btn-new').addEventListener('click', async (e) => {
  const r = e.currentTarget.getBoundingClientRect(); // currentTarget is cleared after the first await
  const p = state.lastProject || state.projects[0]?.path || (await api.addProject());
  if (!p) return;
  if (!findProject(p)) await refresh();
  newSessionMenu({ clientX: r.left + 12, clientY: r.bottom + 2 }, p);
});

$('#btn-add-project').addEventListener('click', async () => {
  const p = await api.addProject();
  if (p) await refresh();
});

function toggleSearch(force) {
  const wrap = $('#search-wrap');
  const show = force ?? wrap.classList.contains('hidden');
  wrap.classList.toggle('hidden', !show);
  if (show) $('#search').focus();
  else { $('#search').value = ''; state.filter = ''; renderSidebar(); }
}
$('#btn-search').addEventListener('click', () => toggleSearch());
$('#search').addEventListener('input', (e) => { state.filter = e.target.value; renderSidebar(); });
$('#search').addEventListener('keydown', (e) => { if (e.key === 'Escape') toggleSearch(false); });
document.addEventListener('keydown', (e) => {
  if (e.ctrlKey && e.key.toLowerCase() === 'k') { e.preventDefault(); toggleSearch(true); }
});

$('#btn-restart').addEventListener('click', () => state.active && restartSession(state.active));
$('#btn-close').addEventListener('click', () => state.active && closeSession(state.active));

// ---------- settings ----------

function applyTheme() {
  document.documentElement.dataset.theme = state.config.theme;
  for (const t of state.terms.values()) {
    t.term.options.theme = state.config.theme === 'dark' ? darkTheme : lightTheme;
    t.term.options.fontSize = state.config.fontSize || 14;
    safeFit(t);
  }
}

$('#btn-settings').addEventListener('click', () => {
  $('#cfg-command').value = state.config.claudeCommand;
  $('#cfg-args').value = state.config.extraArgs;
  $('#cfg-codex-command').value = state.config.codexCommand;
  $('#cfg-codex-args').value = state.config.codexArgs;
  $('#cfg-theme').value = state.config.theme;
  $('#cfg-language').value = state.config.language || 'auto';
  $('#cfg-font').value = state.config.fontSize;
  $('#cfg-tray').checked = state.config.closeToTray !== false;
  $('#cfg-usage').checked = state.config.showUsage !== false;
  refreshEgo();
  $('#settings').classList.remove('hidden');
});
$('#cfg-cancel').addEventListener('click', () => $('#settings').classList.add('hidden'));
$('#cfg-save').addEventListener('click', async () => {
  state.config = await api.setConfig({
    claudeCommand: $('#cfg-command').value.trim() || 'claude',
    extraArgs: $('#cfg-args').value.trim(),
    codexCommand: $('#cfg-codex-command').value.trim() || 'codex',
    codexArgs: $('#cfg-codex-args').value.trim(),
    theme: $('#cfg-theme').value,
    fontSize: Math.min(28, Math.max(9, parseInt($('#cfg-font').value, 10) || 14)),
    closeToTray: $('#cfg-tray').checked,
    showUsage: $('#cfg-usage').checked,
    language: $('#cfg-language').value,
  });
  applyLanguage(state.config.language); // i18n.js: reloads in the new language
  applyTheme();
  renderUsage();
  $('#settings').classList.add('hidden');
});

// ego lite as the agents' browser: whether it and its official skill are installed (browser.js).
async function refreshEgo() {
  const s = await api.egoStatus();
  const both = s.skill.claude && s.skill.codex;
  $('#ego-app').textContent = s.installed ? '✓ ego lite is installed' : '✗ ego lite isn\'t installed';
  $('#ego-app').classList.toggle('danger', !s.installed);
  $('#ego-skill').textContent = both ? '✓ Official skill installed for Claude Code and Codex'
    : s.skill.claude || s.skill.codex ? `✓ Official skill installed for ${s.skill.claude ? 'Claude Code' : 'Codex'} only`
      : '✗ Official skill not installed';
  $('#ego-skill').classList.toggle('danger', !s.skill.claude && !s.skill.codex);
  $('#ego-setup').classList.toggle('hidden', s.installed && both);
  $('#cfg-ego-min').checked = s.keepMinimized;
}
$('#cfg-ego-min').addEventListener('change', (e) => api.setEgoKeepMinimized(e.target.checked));
$('#ego-setup').addEventListener('click', () => { $('#settings').classList.add('hidden'); setUpEgo(); });
$('#ego-check').addEventListener('click', () => refreshEgo());

// ego's README sets ego lite up by pasting its setup prompt into an agent, which then follows
// ego's install guide (on Windows: it walks the user through the installer and first-run setup,
// and installs the official skill). Crew opens a new session in the home folder with that prompt,
// on Claude Code, or Codex when only Codex is installed.
async function setUpEgo(s) {
  s = s || await api.egoStatus();
  const agent = (await api.checkAgent('claude')).installed || !(await api.checkAgent('codex')).installed ? 'claude' : 'codex';
  openSession(s.home, crypto.randomUUID(), false, { agent, prompt: s.setupPrompt });
}

// Once, at startup: if ego lite isn't installed, offer to set it up. Nothing runs without a click.
async function offerEgoSetup() {
  if (state.config.egoSetupOffered) return;
  const s = await api.egoStatus();
  if (s.installed) return;
  state.config = await api.setConfig({ egoSetupOffered: true });
  const box = $('#ssh-prompt'); // the shared dialog (hosts.js)
  const close = () => box.classList.add('hidden');
  box.firstElementChild.replaceChildren(
    el('h2', {}, 'Set up ego lite?'),
    el('p', { class: 'prompt-text' }, 'Agents use ego lite, a free browser built for agents, for anything that needs a browser. It isn\'t installed yet.'),
    el('p', { class: 'prompt-text' }, 'Set up opens an agent session with ego\'s official setup prompt. The agent guides you through installing ego lite and its skill. You can also do this later in Settings.'),
    el('div', { class: 'modal-actions' },
      el('button', { onclick: close }, 'Not now'),
      el('button', { class: 'primary', onclick: () => { close(); setUpEgo(s); } }, 'Set up')));
  box.classList.remove('hidden');
}

// ---------- sidebar toggle ----------

function setSidebar(show, animate = true) {
  const sidebar = $('#sidebar');
  if (!animate) sidebar.style.transition = 'none';
  sidebar.style.marginLeft = show ? '0px' : `${-sidebar.offsetWidth}px`;
  sidebar.inert = !show; // no tabbing into it while it's off screen
  document.body.classList.toggle('sidebar-hidden', !show);
  $('#btn-sidebar').title = show ? 'Hide sidebar' : 'Show sidebar';
  if (!animate) { void sidebar.offsetWidth; sidebar.style.transition = ''; }
  try { localStorage.setItem('sidebarHidden', show ? '0' : '1'); } catch { /* ignore */ }
}
$('#btn-sidebar').addEventListener('click', () => setSidebar(document.body.classList.contains('sidebar-hidden')));
// After the saved width below is applied, before the first paint.
queueMicrotask(() => { try { if (localStorage.getItem('sidebarHidden') === '1') setSidebar(false, false); } catch { /* ignore */ } });

// ---------- sidebar resize ----------

(() => {
  const resizer = $('#resizer');
  const sidebar = $('#sidebar');
  const saved = parseInt(localStorage.getItem('sidebarWidth') || '', 10);
  if (saved) sidebar.style.width = `${saved}px`;
  resizer.addEventListener('mousedown', (e) => {
    e.preventDefault();
    resizer.classList.add('dragging');
    document.body.classList.add('resizing');
    const move = (ev) => { sidebar.style.width = `${Math.min(560, Math.max(200, ev.clientX))}px`; };
    const up = () => {
      resizer.classList.remove('dragging');
      document.body.classList.remove('resizing');
      localStorage.setItem('sidebarWidth', parseInt(sidebar.style.width, 10));
      removeEventListener('mousemove', move);
      removeEventListener('mouseup', up);
    };
    addEventListener('mousemove', move);
    addEventListener('mouseup', up);
  });
})();

// ---------- browser panel ----------

const browserEl = $('#browser');
const urlInput = $('#br-url');
const tabsEl = $('#br-tabs');
const HOME_URL = 'https://www.bing.com';

// Each tab is its own webview. Background tabs stay loaded (hidden), so switching keeps the page
// and its history. All tabs share the persistent partition, so sign-ins carry across tabs.
const browser = { tabs: [], active: null, open: false };
let urlEditing = false; // don't overwrite what the user is typing in the address bar

function normalizeUrl(s) {
  s = s.trim();
  if (!s) return null;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(s) || /^(about|data|file):/i.test(s)) return s;
  if (/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/i.test(s)) return `http://${s}`;
  if (!/\s/.test(s) && /^[^/]+\.[a-z]{2,}(:\d+)?(\/.*)?$/i.test(s)) return `https://${s}`;
  return `https://www.bing.com/search?q=${encodeURIComponent(s)}`;
}

const activeTab = () => browser.tabs.find((t) => t.id === browser.active) || null;

function saveTabs() {
  try {
    localStorage.setItem('browserTabs', JSON.stringify({
      urls: browser.tabs.map((t) => t.url),
      active: Math.max(0, browser.tabs.findIndex((t) => t.id === browser.active)),
    }));
  } catch { /* ignore */ }
}

function savedTabs() {
  try {
    const v = JSON.parse(localStorage.getItem('browserTabs') || 'null');
    if (v?.urls?.length) return v;
  } catch { /* ignore */ }
  const last = localStorage.getItem('browserUrl'); // single-page browser from earlier versions
  return { urls: [last || HOME_URL], active: 0 };
}

function updateNav() {
  const t = activeTab();
  const ready = t && t.ready;
  $('#br-back').disabled = !ready || !t.view.canGoBack();
  $('#br-forward').disabled = !ready || !t.view.canGoForward();
  if (!urlEditing) urlInput.value = t ? t.url : '';
}

function hostOf(url) {
  try { return new URL(url).hostname.replace(/^www\./, ''); } catch { return url; }
}

function renderTabs() {
  tabsEl.innerHTML = '';
  for (const t of browser.tabs) {
    const fav = t.favicon
      ? el('img', { class: 'fav', src: t.favicon, onerror: () => { t.favicon = null; renderTabs(); } })
      : el('span', { class: 'fav blank', html: '<svg viewBox="0 0 24 24" width="16" height="16"><circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a14 14 0 0 1 0 18a14 14 0 0 1 0-18"/></svg>' });
    tabsEl.append(el('div', {
      class: `br-tab${t.id === browser.active ? ' active' : ''}${t.loading ? ' loading' : ''}`,
      title: `${t.title || ''}\n${t.url}`.trim(),
      onclick: () => switchTab(t.id),
      onauxclick: (e) => { if (e.button === 1) closeTab(t.id); }, // middle-click closes, as in browsers
    },
    fav,
    el('span', { class: 'title' }, t.title || hostOf(t.url)),
    el('button', {
      class: 'icon-btn tab-close',
      title: 'Close tab',
      html: '<svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>',
      onclick: (e) => { e.stopPropagation(); closeTab(t.id); },
    })));
  }
}

function newTab(url = HOME_URL, { activate = true } = {}) {
  const view = document.createElement('webview');
  view.setAttribute('partition', 'persist:browser');
  view.setAttribute('allowpopups', '');
  view.setAttribute('src', url);
  const t = { id: crypto.randomUUID(), view, url, title: '', favicon: null, ready: false, loading: true };
  const onNav = (e) => {
    t.url = e.url;
    saveTabs();
    if (t.id === browser.active) updateNav();
    renderTabs();
  };
  view.addEventListener('did-navigate', onNav);
  view.addEventListener('did-navigate-in-page', (e) => e.isMainFrame && onNav(e));
  view.addEventListener('page-title-updated', (e) => { t.title = e.title; renderTabs(); });
  view.addEventListener('page-favicon-updated', (e) => { t.favicon = e.favicons?.[0] || null; renderTabs(); });
  view.addEventListener('did-start-loading', () => { t.loading = true; renderTabs(); });
  view.addEventListener('did-stop-loading', () => { t.loading = false; renderTabs(); if (t.id === browser.active) updateNav(); });
  view.addEventListener('dom-ready', () => { t.ready = true; if (t.id === browser.active) updateNav(); });
  browser.tabs.push(t);
  $('#browser-body').append(view);
  if (activate || !browser.active) switchTab(t.id);
  else { view.classList.add('background'); renderTabs(); }
  saveTabs();
  return t;
}

function switchTab(id) {
  browser.active = id;
  for (const t of browser.tabs) t.view.classList.toggle('background', t.id !== id);
  urlEditing = false;
  updateNav();
  renderTabs();
  saveTabs();
}

function closeTab(id) {
  const i = browser.tabs.findIndex((t) => t.id === id);
  if (i < 0) return;
  browser.tabs[i].view.remove();
  browser.tabs.splice(i, 1);
  if (!browser.tabs.length) { saveTabs(); closeBrowser(); return; }
  if (browser.active === id) switchTab(browser.tabs[Math.min(i, browser.tabs.length - 1)].id);
  else { renderTabs(); saveTabs(); }
}

// Show the panel (restoring the last tabs); with a URL, open it in a new tab.
function openBrowser(url) {
  if (!browser.open) {
    browser.open = true;
    browserEl.classList.remove('hidden');
    $('#browser-resizer').classList.remove('hidden');
    document.body.classList.add('browser-open');
    localStorage.setItem('browserOpen', '1');
    const saved = savedTabs();
    saved.urls.forEach((u, i) => newTab(u, { activate: i === saved.active }));
  }
  if (url) newTab(url);
}

// Closing the panel unloads every tab but remembers them for the next open.
function closeBrowser() {
  saveTabs();
  for (const t of browser.tabs) t.view.remove();
  browser.tabs = [];
  browser.active = null;
  browser.open = false;
  tabsEl.innerHTML = '';
  browserEl.classList.add('hidden');
  $('#browser-resizer').classList.add('hidden');
  document.body.classList.remove('browser-open');
  localStorage.setItem('browserOpen', '0');
  state.terms.get(state.active)?.term.focus();
}

$('#btn-browser').addEventListener('click', () => (browser.open ? closeBrowser() : openBrowser()));
$('#br-close').addEventListener('click', closeBrowser);
$('#br-new-tab').addEventListener('click', () => { newTab(); urlInput.focus(); });
$('#br-back').addEventListener('click', () => { const t = activeTab(); if (t?.ready && t.view.canGoBack()) t.view.goBack(); });
$('#br-forward').addEventListener('click', () => { const t = activeTab(); if (t?.ready && t.view.canGoForward()) t.view.goForward(); });
$('#br-reload').addEventListener('click', () => { const t = activeTab(); if (t?.ready) t.view.reload(); });
$('#br-external').addEventListener('click', () => { const t = activeTab(); if (t) window.open(t.url); });
$('#br-form').addEventListener('submit', (e) => {
  e.preventDefault();
  const url = normalizeUrl(urlInput.value);
  if (!url) return;
  const t = activeTab();
  if (!t) newTab(url);
  else if (t.ready) t.view.loadURL(url);
  else t.view.setAttribute('src', url);
  urlInput.value = url;
  urlEditing = false;
  urlInput.blur();
});
urlInput.addEventListener('focus', () => urlInput.select());
urlInput.addEventListener('input', () => { urlEditing = true; });
urlInput.addEventListener('blur', () => { urlEditing = false; });
urlInput.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { urlEditing = false; urlInput.value = activeTab()?.url || ''; urlInput.blur(); }
});

// Pop-ups and target=_blank links from pages (routed by the main process) open as new tabs.
api.onBrowserNewTab((url) => (browser.open ? newTab(url) : openBrowser(url)));

// Ctrl+T / Ctrl+W / Ctrl+Tab while the browser is open (also forwarded from inside pages by main).
function browserShortcut(key, shift) {
  if (!browser.open) return false;
  if (key === 't') { newTab(); urlInput.focus(); return true; }
  if (key === 'w') { if (browser.active) closeTab(browser.active); return true; }
  if (key === 'Tab' && browser.tabs.length > 1) {
    const i = browser.tabs.findIndex((t) => t.id === browser.active);
    const n = browser.tabs.length;
    switchTab(browser.tabs[(i + (shift ? n - 1 : 1)) % n].id);
    return true;
  }
  if (key === 'l') { urlInput.focus(); return true; }
  return false;
}
api.onBrowserShortcut(({ key, shift }) => browserShortcut(key, shift));
document.addEventListener('keydown', (e) => {
  // Only from the browser panel: in the terminal or composer these keys belong to the agent.
  if (!e.ctrlKey || !browserEl.contains(document.activeElement)) return;
  if (browserShortcut(e.key.length === 1 ? e.key.toLowerCase() : e.key, e.shiftKey)) e.preventDefault();
});

(() => {
  const resizer = $('#browser-resizer');
  const saved = parseInt(localStorage.getItem('browserWidth') || '', 10);
  if (saved) browserEl.style.width = `${saved}px`;
  resizer.addEventListener('mousedown', (e) => {
    e.preventDefault();
    resizer.classList.add('dragging');
    document.body.classList.add('resizing');
    const move = (ev) => {
      // Leave the terminal at least 360px next to the sidebar.
      const max = innerWidth - $('#sidebar').offsetWidth - 360;
      browserEl.style.width = `${Math.max(320, Math.min(max, innerWidth - ev.clientX))}px`;
    };
    const up = () => {
      resizer.classList.remove('dragging');
      document.body.classList.remove('resizing');
      localStorage.setItem('browserWidth', parseInt(browserEl.style.width, 10));
      removeEventListener('mousemove', move);
      removeEventListener('mouseup', up);
    };
    addEventListener('mousemove', move);
    addEventListener('mouseup', up);
  });
})();

// ---------- composer ----------

// A real text box under the terminal. Messages and the chip commands are pasted into the
// session's own prompt and submitted, so the agent still runs exactly as in a terminal.

const composerEl = $('#composer');
const inputEl = $('#composer-input');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Labels for the aliases Claude Code writes to settings.json ("model": "opus").
const CLAUDE_ALIASES = { fable: 'Fable 5.1', opus: 'Opus 5.5', sonnet: 'Sonnet 5.5', haiku: 'Haiku 5.5' };
const CLAUDE_EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'];
const CLAUDE_MODES = [['manual', 'Manual'], ['acceptEdits', 'Accept edits'], ['plan', 'Plan'], ['auto', 'Auto']];
const MODE_LABELS = {
  manual: 'Manual', default: 'Manual', acceptEdits: 'Accept edits', plan: 'Plan', auto: 'Auto',
  bypassPermissions: 'Bypass', dontAsk: "Don't ask",
  untrusted: 'Untrusted', 'on-request': 'On request', 'on-failure': 'On failure', never: 'Never ask',
};

const cap = (t) => (t ? t[0].toUpperCase() + t.slice(1) : '');

function modelLabel(agent, m) {
  if (!m) return 'Model';
  if (agent === 'codex') {
    // Session files say "gpt-6-luna"; show it the way Codex's picker spells it ("GPT-6-Luna").
    const known = cachedCodexModels()?.find((x) => x.label.toLowerCase() === m.toLowerCase());
    return known ? known.label : m;
  }
  if (CLAUDE_ALIASES[m]) return CLAUDE_ALIASES[m];
  const x = m.match(/^claude-([a-z]+)-(\d+)-(\d+)/);
  return x ? `${cap(x[1])} ${x[2]}.${x[3]}` : m;
}

function effortLabel(e) {
  return e === 'xhigh' ? 'Extra high' : cap(e);
}

function currentSettings(entry) {
  // Session file < what the agent's status line shows now < a model the user just picked.
  return { ...entry.fileSettings, ...entry.live, ...entry.overrides };
}

async function loadSettings(sessionId) {
  const entry = state.terms.get(sessionId);
  if (!entry || entry.agent === 'ssh') return;
  const s = await api.sessionState(sessionId, entry.agent);
  // Once the file reports a different model (e.g. after the next reply), it supersedes the pick.
  if (entry.fileSettings.model !== undefined && s.model !== entry.fileSettings.model) delete entry.overrides.model;
  entry.fileSettings = s;
  if (state.active === sessionId) renderComposer();
}

function screenLines(entry, count) {
  const buf = entry.term.buffer.active;
  const lines = [];
  for (let y = Math.max(0, entry.term.rows - count); y < entry.term.rows; y++) {
    lines.push(buf.getLine(buf.baseY + y)?.translateToString(true) || '');
  }
  return lines;
}

// Read mode / effort / model off the agent's own status line at the bottom of the screen.
function readScreen(entry) {
  const text = screenLines(entry, 8).join('\n');
  const live = {};
  if (entry.agent === 'codex') {
    const m = text.match(/^\s*([\w.-]+)\s+(minimal|low|medium|high|xhigh|max|ultra)\s+·/m);
    if (m) { live.model = m[1]; live.effort = m[2]; }
  } else {
    if (/auto mode on/i.test(text)) live.mode = 'auto';
    else if (/accept edits on/i.test(text)) live.mode = 'acceptEdits';
    else if (/plan mode on/i.test(text)) live.mode = 'plan';
    else if (/bypass permissions on/i.test(text)) live.mode = 'bypassPermissions';
    else if (/don'?t ask on/i.test(text)) live.mode = 'dontAsk';
    else if (/\? for shortcuts/.test(text)) live.mode = 'manual';
    const e = text.match(/\b(low|medium|high|xhigh|max)\s+·\s+\/effort/);
    if (e) live.effort = e[1];
  }
  return live;
}

// While working, both agents keep a status line above the prompt: Claude "✻ Cogitating… (12s ·
// esc to interrupt)", Codex "• Working (5s • esc to interrupt)". It's gone once the turn ends.
function isBusy(entry) {
  if (entry.exited) return false;
  const text = screenLines(entry, 14).join('\n');
  return /esc to interrupt/i.test(text) || /^\s*[^\s\w]\s+\w[\w-]*…\s*\(\d/m.test(text);
}

const statusEl = (entry) => (entry.busy
  ? el('span', { class: 'spinner', title: 'Working' })
  : el('span', { class: 'dot', title: 'Idle' }));

function scheduleScreenRead(entry) {
  if (entry.screenTimer) return;
  entry.screenTimer = setTimeout(() => {
    entry.screenTimer = null;
    const live = readScreen(entry);
    const busy = isBusy(entry);
    if (busy !== !!entry.busy) {
      entry.busy = busy;
      renderSidebar();
      updatePaneHeads();
    }
    const changed = Object.keys(live).some((k) => live[k] !== entry.live[k]);
    Object.assign(entry.live, live);
    // A pick stays shown until the status line confirms it: Codex briefly redraws the old
    // values right after a change.
    for (const k of Object.keys(entry.overrides)) {
      if (entry.agent === 'codex' && live[k] && live[k].toLowerCase() === String(entry.overrides[k]).toLowerCase()) delete entry.overrides[k];
    }
    if (changed && state.terms.get(state.active) === entry) renderComposer();
  }, 400);
}

function renderComposer() {
  const entry = state.terms.get(state.active);
  composerEl.classList.toggle('hidden', !entry || entry.agent === 'ssh');
  if (!entry || entry.agent === 'ssh') return;
  const s = currentSettings(entry);
  const codex = entry.agent === 'codex';
  inputEl.placeholder = codex ? 'Ask Codex to do anything' : 'How can I help you today?';
  const proj = findProject(entry.projectPath);
  $('#chip-project').textContent = `${hostNameFor(entry.projectPath) || proj?.name || entry.projectPath.split(/[\\/]/).pop()} ▾`;
  $('#chip-model').textContent = modelLabel(entry.agent, s.model);
  $('#chip-effort').textContent = s.effort ? effortLabel(s.effort) : '';
  $('#chip-mode').textContent = MODE_LABELS[s.mode] || (codex ? 'Permissions' : 'Manual');
  $('#composer-hint').textContent = entry.missingEl ? 'Install the agent above to start this session'
    : entry.installing ? 'Installing…'
      : entry.exited ? 'Session stopped — press Enter to resume it' : '';
  $('#composer-send').disabled = !canSend();
}

function autosizeInput() {
  inputEl.style.height = 'auto';
  inputEl.style.height = `${Math.min(inputEl.scrollHeight, 220)}px`;
  $('#composer-send').disabled = !canSend();
}

// Bracketed paste, so newlines stay inside the message and "/" doesn't open the command menu
// mid-typing; then Enter submits it.
function sendText(entry, text) {
  if (!entry?.termId || entry.exited) return false;
  api.write(entry.termId, `\x1b[200~${text.replace(/\r\n?|\n/g, '\r')}\x1b[201~`);
  setTimeout(() => api.write(entry.termId, '\r'), 150);
  return true;
}

function activeEntry() {
  return state.terms.get(state.active);
}

// What is typed in Claude's own prompt box (the last "❯" line), or null if none is on screen.
function promptText(entry) {
  const line = screenLines(entry, 12).reverse().find((l) => /^\s*❯/.test(l));
  return line == null ? null : line.replace(/^\s*❯\s?/, '').trim();
}

// Slash commands from the chips. A command pasted while Claude is still finishing the previous
// one can be dropped, so commands run one at a time, wait for an empty prompt, and are re-sent
// once if Claude never echoes them.
function sendCommand(entry, cmd) {
  return queue(entry, async () => {
    for (let attempt = 0; attempt < 2 && !entry.exited; attempt++) {
      for (let i = 0; i < 20 && promptText(entry) !== ''; i++) await sleep(200);
      sendText(entry, cmd);
      await sleep(1500);
      // Echoed above the prompt box = submitted (the box itself would show it unsent).
      const lines = screenLines(entry, entry.term.rows);
      const box = lines.map((l) => /^\s*❯/.test(l)).lastIndexOf(true);
      if (lines.slice(0, box).some((l) => l.includes(`❯ ${cmd}`))) return;
    }
  });
}

function canSend() {
  return !!inputEl.value.trim() || !!activeEntry()?.attachments.length;
}

const paste = (entry, text) => api.write(entry.termId, `\x1b[200~${text}\x1b[201~`);

async function submitComposer() {
  const entry = activeEntry();
  const text = inputEl.value.trim();
  const files = entry ? entry.attachments.slice() : [];
  if (!entry || (!text && !files.length)) return;
  if (entry.exited) {
    await respawn(state.active, isOnDisk(state.active));
    return; // keep the draft; it can be sent once the session is back up
  }
  if (!entry.termId) return;
  inputEl.value = '';
  entry.draft = '';
  entry.attachments = [];
  autosizeInput();
  renderAttachments();
  // Claude and Codex both turn a pasted image path into "[Image #N]", the same as pasting a
  // screenshot into them directly. Other files are referenced in the text (@path for Claude).
  const codex = entry.agent === 'codex';
  const refs = [];
  for (const f of files) {
    if (f.isImage) { paste(entry, f.path); await sleep(350); continue; }
    const q = /\s/.test(f.path) ? `"${f.path}"` : f.path;
    refs.push(codex ? q : `@${q}`);
  }
  const body = withAgentNote([refs.join(' '), text].filter(Boolean).join(' ')); // @agents (agents.js)
  if (body) paste(entry, `${files.some((f) => f.isImage) ? ' ' : ''}${body.replace(/\r\n?|\n/g, '\r')}`);
  // An Enter too soon after a long paste counts as part of it (Codex, at least), so wait longer.
  await sleep(files.length ? 300 : body.length > 200 ? 700 : 150);
  api.write(entry.termId, '\r');
}

inputEl.addEventListener('input', autosizeInput);
inputEl.addEventListener('keydown', (e) => {
  if (e.isComposing || e.keyCode === 229) return; // IME (e.g. Chinese input) still composing
  const entry = activeEntry();
  if (!entry) return;
  const empty = !inputEl.value && !entry.attachments.length;
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    if (canSend()) submitComposer();
    else if (entry.exited) respawn(state.active, isOnDisk(state.active));
    else if (entry.termId) api.write(entry.termId, '\r'); // confirm a prompt shown in the terminal
    return;
  }
  // With an empty box, keys that drive the agent's own menus and prompts go to the terminal.
  const passthrough = { Escape: '\x1b', ArrowUp: '\x1b[A', ArrowDown: '\x1b[B' };
  if (empty && passthrough[e.key] && entry.termId && !entry.exited) {
    e.preventDefault();
    api.write(entry.termId, passthrough[e.key]);
    return;
  }
  if (e.key === 'Tab' && e.shiftKey && entry.termId) { // cycle permission mode, as in the terminal
    e.preventDefault();
    api.write(entry.termId, '\x1b[Z');
  }
});
$('#composer-send').addEventListener('click', submitComposer);

// ---------- file links & image viewer ----------

// Web links open in the side browser (Ctrl+click: default browser). File links and Windows paths
// open images in a viewer over the app and other files in their default program.
function openLink(e, uri) {
  if (/^file:/i.test(uri)) return openFileLink(uri);
  if (/^https?:/i.test(uri)) return e?.ctrlKey ? window.open(uri) : openBrowser(uri);
  return undefined;
}

function filePathOf(link) {
  if (!/^file:/i.test(link)) return link;
  let p = decodeURIComponent(link.replace(/^file:\/*/i, ''));
  if (/^[A-Za-z]:/.test(p)) p = p.replace(/\//g, '\\'); // file:///C:/x/y.png -> C:\x\y.png
  else p = `/${p}`;
  return p;
}

async function openFileLink(link) {
  const p = filePathOf(link);
  if (IMAGE_EXT.test(p)) return showImage(p);
  if (await api.fileExists(p)) api.openFile(p);
}

// file:///… links, and Windows paths that end in a file extension ("C:\dir\out.png").
const FILE_LINK_RE = /file:\/\/\/[^\s"'<>`]+|[A-Za-z]:[\\/](?:[^\s"'<>`|*?\\/]+[\\/])*[^\s"'<>`|*?\\/]+\.[A-Za-z0-9]{1,8}/g;

// The logical line around row y (rows joined where the terminal soft-wrapped them), plus the
// cell position of every character so matches map back to the screen even next to CJK text.
function logicalLine(term, y) {
  const buf = term.buffer.active;
  let start = y;
  while (start > 0 && buf.getLine(start)?.isWrapped) start--;
  let text = '';
  const pos = [];
  for (let row = start; row < buf.length; row++) {
    const line = buf.getLine(row);
    if (!line || (row > start && !line.isWrapped)) break;
    for (let x = 0; x < term.cols; x++) {
      const cell = line.getCell(x);
      if (!cell || cell.getWidth() === 0) continue; // second half of a wide character
      const ch = cell.getChars() || ' ';
      for (let k = 0; k < ch.length; k++) pos.push({ x: x + 1, y: row + 1, w: cell.getWidth() });
      text += ch;
    }
  }
  return { text, pos };
}

function fileLinkProvider(term) {
  return {
    provideLinks(y, callback) {
      const { text, pos } = logicalLine(term, y - 1);
      const links = [];
      for (const m of text.matchAll(FILE_LINK_RE)) {
        const raw = m[0].replace(/[.,;:)\]}'"`]+$/, ''); // trailing punctuation isn't part of the path
        const a = pos[m.index];
        const b = pos[m.index + raw.length - 1];
        if (!a || !b || y < a.y || y > b.y) continue;
        links.push({
          range: { start: { x: a.x, y: a.y }, end: { x: b.x + b.w - 1, y: b.y } },
          text: raw,
          decorations: { underline: true, pointerCursor: true },
          activate: () => openFileLink(raw),
        });
      }
      callback(links.length ? links : undefined);
    },
  };
}

const viewerEl = $('#viewer');
let viewerPath = null;

function showImage(p, src = fileUrl(p)) {
  viewerPath = p;
  const img = $('#viewer-img');
  $('#viewer-error').classList.add('hidden');
  img.classList.remove('hidden');
  img.onerror = () => { img.classList.add('hidden'); $('#viewer-error').classList.remove('hidden'); };
  img.src = src;
  $('#viewer-name').textContent = p.split(/[\\/]/).pop();
  $('#viewer-name').title = p;
  viewerEl.classList.remove('hidden');
}

function closeViewer() {
  viewerEl.classList.add('hidden');
  $('#viewer-img').removeAttribute('src');
  viewerPath = null;
}

viewerEl.addEventListener('click', (e) => { if (e.target === viewerEl || e.target.id === 'viewer-stage') closeViewer(); });
$('#viewer-close').addEventListener('click', closeViewer);
$('#viewer-open').addEventListener('click', () => viewerPath && api.openFile(viewerPath));
$('#viewer-show').addEventListener('click', () => viewerPath && api.showFile(viewerPath));
$('#viewer-copy').addEventListener('click', () => viewerPath && navigator.clipboard.writeText(viewerPath));
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !viewerEl.classList.contains('hidden')) { e.preventDefault(); e.stopPropagation(); closeViewer(); }
}, true);

// ---------- composer attachments ----------

const IMAGE_EXT = /\.(png|jpe?g|gif|webp|bmp)$/i;
const fileUrl = (p) => `file:///${p.replace(/\\/g, '/').split('/').map(encodeURIComponent).join('/').replace(/^([A-Za-z])%3A/, '$1:')}`;

function addAttachment(att) {
  const entry = activeEntry();
  if (!entry || entry.attachments.some((a) => a.path === att.path)) return;
  entry.attachments.push(att);
  renderAttachments();
  autosizeInput();
  inputEl.focus();
}

function addFiles(paths) {
  for (const p of paths) {
    const isImage = IMAGE_EXT.test(p);
    addAttachment({ path: p, isImage, thumb: isImage ? fileUrl(p) : null });
  }
}

function renderAttachments() {
  const box = $('#composer-attachments');
  const entry = activeEntry();
  box.innerHTML = '';
  const list = entry ? entry.attachments : [];
  box.classList.toggle('hidden', !list.length);
  for (const a of list) {
    const name = a.path.split(/[\\/]/).pop();
    const remove = el('button', {
      class: 'att-remove',
      title: 'Remove',
      html: '<svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>',
      onclick: () => {
        entry.attachments = entry.attachments.filter((x) => x !== a);
        renderAttachments();
        autosizeInput();
      },
    });
    box.append(a.isImage
      ? el('div', { class: 'att att-image', title: a.path, onclick: (e) => { if (!e.target.closest('.att-remove')) showImage(a.path, a.thumb); } },
        el('img', { src: a.thumb, alt: name }), remove)
      : el('div', { class: 'att att-file', title: a.path },
        el('span', { html: '<svg viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>' }),
        el('span', { class: 'att-name' }, name), remove));
  }
}

// Ctrl+V of a screenshot (or images copied in a browser): save it and show a thumbnail.
// Files copied in Explorer arrive as files with paths.
inputEl.addEventListener('paste', async (e) => {
  const files = [...(e.clipboardData?.files || [])];
  if (!files.length) return; // plain text: let the textarea handle it
  e.preventDefault();
  for (const f of files) {
    const p = api.pathForFile(f);
    if (p) { addFiles([p]); continue; }
    if (!f.type.startsWith('image/')) continue;
    const saved = await api.saveAttachment(new Uint8Array(await f.arrayBuffer()), f.type);
    addAttachment({ path: saved, isImage: true, thumb: URL.createObjectURL(f) });
  }
  const text = e.clipboardData.getData('text/plain');
  if (text && !files.every((f) => api.pathForFile(f))) inputEl.setRangeText(text, inputEl.selectionStart, inputEl.selectionEnd, 'end');
});

$('#composer-attach').addEventListener('click', async () => {
  const paths = await api.pickFiles();
  if (paths.length) addFiles(paths);
});
composerEl.addEventListener('dragover', (e) => e.preventDefault());
composerEl.addEventListener('drop', (e) => {
  e.preventDefault();
  const paths = [...e.dataTransfer.files].map((f) => api.pathForFile(f)).filter(Boolean);
  if (paths.length) addFiles(paths);
});

// Claude's permission mode can only be changed by cycling with Shift+Tab, so press it until the
// status line shows the wanted mode.
function setClaudeMode(entry, target) {
  queue(entry, async () => {
    for (let i = 0; i < 6; i++) {
      Object.assign(entry.live, readScreen(entry));
      if (entry.live.mode === target) break;
      api.write(entry.termId, '\x1b[Z');
      await sleep(400);
    }
    renderComposer();
  });
}

// Codex's /model and /permissions take no arguments: open its picker and hand over to the terminal.
function openCodexPicker(entry, command) {
  sendText(entry, command);
  entry.term.focus();
}

// Claude's model list isn't stored anywhere readable (it's built into Claude Code and depends on
// the account), so it's read from Claude's own /model picker, and picks are made in that picker.

const MODEL_CACHE_KEY = 'claudeModels';
const MODEL_CACHE_MS = 24 * 3600 * 1000;

function cachedModels() {
  try {
    const c = JSON.parse(localStorage.getItem(MODEL_CACHE_KEY) || 'null');
    return c && Date.now() - c.at < MODEL_CACHE_MS ? c.models : null;
  } catch { return null; }
}

// Parse the visible rows of the picker: "   ❯ 2.  Opus 5.5 ✔             For complex work…"
function readPicker(entry) {
  const all = screenLines(entry, entry.term.rows);
  const start = all.map((l) => /Select model/.test(l)).lastIndexOf(true);
  if (start < 0) return null;
  const lines = all.slice(start); // numbered lists in earlier replies aren't picker rows
  const rows = [];
  let highlighted = null;
  let more = 0;
  for (const l of lines) {
    const m = l.match(/^\s*(❯|↑|↓)?\s*(\d+)\.\s+(.+?)(\s+✔)?(?:\s{2,}(.*?))?\s*$/);
    if (m) {
      const index = Number(m[2]);
      rows.push({ index, label: m[3].trim(), current: !!m[4], description: (m[5] || '').trim() });
      if (m[1] === '❯') highlighted = index;
    }
    const mm = l.match(/…\s*\+(\d+)\s+models?/);
    if (mm) more = Number(mm[1]);
  }
  return { rows, highlighted, more };
}

async function waitForPicker(entry) {
  for (let i = 0; i < 25; i++) {
    const p = readPicker(entry);
    if (p && p.rows.length && p.highlighted) return p;
    await sleep(200);
  }
  return null;
}

// Arrow keys sent faster than ~200ms apart get merged by Claude, so space them out.
async function pressKey(entry, seq, times) {
  for (let i = 0; i < times; i++) {
    api.write(entry.termId, seq);
    await sleep(250);
  }
}

async function openPicker(entry) {
  for (let i = 0; i < 20 && promptText(entry) !== ''; i++) await sleep(200);
  sendText(entry, '/model');
  return waitForPicker(entry);
}

function queue(entry, fn) {
  entry.commandQueue = (entry.commandQueue || Promise.resolve()).then(fn).catch((err) => console.error(err));
  return entry.commandQueue;
}

function loadClaudeModels(entry) {
  return queue(entry, async () => {
    let p = await openPicker(entry);
    if (!p) return null;
    const byIndex = new Map(p.rows.map((r) => [r.index, r]));
    if (p.more) {
      // The list wraps: Up from the first row lands on the last, which scrolls the rest into view.
      await pressKey(entry, '\x1b[A', p.highlighted);
      p = readPicker(entry) || p;
      for (const r of p.rows) byIndex.set(r.index, { ...byIndex.get(r.index), ...r });
    }
    api.write(entry.termId, '\x1b'); // close without changing anything
    const models = [...byIndex.values()].sort((a, b) => a.index - b.index)
      .map(({ index, label, description }) => ({ index, label, description }));
    try { localStorage.setItem(MODEL_CACHE_KEY, JSON.stringify({ at: Date.now(), models })); } catch { /* ignore */ }
    return models;
  });
}

function selectClaudeModel(entry, model) {
  return queue(entry, async () => {
    let p = await openPicker(entry);
    if (!p) return;
    for (let attempt = 0; attempt < 3 && p.highlighted !== model.index; attempt++) {
      const delta = model.index - p.highlighted;
      await pressKey(entry, delta > 0 ? '\x1b[B' : '\x1b[A', Math.abs(delta));
      await sleep(200);
      p = readPicker(entry) || p;
    }
    const row = p.rows.find((r) => r.index === model.index);
    if (p.highlighted !== model.index || (row && row.label !== model.label)) {
      api.write(entry.termId, '\x1b'); // the list changed under us: cancel rather than pick the wrong model
      localStorage.removeItem(MODEL_CACHE_KEY);
      $('#composer-hint').textContent = 'Claude\'s model list changed — click the model again';
      return;
    }
    api.write(entry.termId, '\r');
    // "Default (recommended)" names the model it resolves to in its description.
    entry.overrides.model = /^Default/.test(model.label) ? (model.description.split('·')[0].trim() || 'Default') : model.label;
    renderComposer();
  });
}

$('#chip-model').addEventListener('click', async (e) => {
  const entry = activeEntry();
  if (!entry || entry.exited) return;
  if (entry.agent === 'codex') return codexModelMenu(e, entry);
  const rect = e.currentTarget.getBoundingClientRect();
  let models = cachedModels();
  if (!models) {
    $('#composer-hint').textContent = 'Reading Claude\'s model list…';
    models = await loadClaudeModels(entry);
    renderComposer();
    if (!models?.length) { $('#composer-hint').textContent = 'Couldn\'t read the model list — try again when Claude is idle'; return; }
  }
  const current = modelLabel('claude', currentSettings(entry).model);
  showMenu(e, [
    ...models.map((m) => ({
      label: m.label,
      checked: m.label === current,
      action: () => selectClaudeModel(entry, m),
    })),
    { label: 'Refresh model list', separator: true, action: () => { localStorage.removeItem(MODEL_CACHE_KEY); loadClaudeModels(entry); } },
  ], rect);
});

$('#chip-effort').addEventListener('click', (e) => {
  const entry = activeEntry();
  if (!entry || entry.exited) return;
  if (entry.agent === 'codex') return codexEffortMenu(e, entry);
  const current = currentSettings(entry).effort;
  showMenu(e, CLAUDE_EFFORTS.map((lvl) => ({
    label: effortLabel(lvl),
    checked: lvl === current,
    action: () => { entry.live.effort = lvl; sendCommand(entry, `/effort ${lvl}`); renderComposer(); },
  })), e.currentTarget.getBoundingClientRect());
});

$('#chip-mode').addEventListener('click', (e) => {
  const entry = activeEntry();
  if (!entry || entry.exited) return;
  if (entry.agent === 'codex') return openCodexPicker(entry, '/permissions');
  const current = currentSettings(entry).mode;
  showMenu(e, CLAUDE_MODES.map(([mode, label]) => ({
    label,
    checked: mode === current || (mode === 'manual' && current === 'default'),
    action: () => setClaudeMode(entry, mode),
  })), e.currentTarget.getBoundingClientRect());
});

$('#chip-project').addEventListener('click', (e) => {
  const entry = activeEntry();
  if (!entry) return;
  showMenu(e, [
    { label: 'New Claude session here', action: () => newSession(entry.projectPath, 'claude') },
    { label: 'New Codex session here', action: () => newSession(entry.projectPath, 'codex') },
    { label: 'Open folder', action: () => api.revealProject(entry.cwd) },
    { label: 'Copy path', action: () => navigator.clipboard.writeText(entry.cwd) },
  ], e.currentTarget.getBoundingClientRect());
});

// ---------- Codex model / effort menus ----------

// Codex's /model picker has two steps: "Select Model and Effort", then "Select Reasoning Level
// for <model>" (whose "More reasoning…" row opens "Advanced Reasoning" with Max). Both lists
// depend on the account, so they're read from the picker and picks are made in it, as for Claude.

const CODEX_CACHE_KEY = 'codexModels';
const CODEX_HEADINGS = /Select Model and Effort|Select Reasoning Level|Advanced Reasoning/;

function cachedCodexModels() {
  try {
    const c = JSON.parse(localStorage.getItem(CODEX_CACHE_KEY) || 'null');
    return c && Date.now() - c.at < MODEL_CACHE_MS ? c.models : null;
  } catch { return null; }
}

// "› 3. High (current)    Greater reasoning depth…" -> { index: 3, label: 'High', current: true, … }
function readCodexPicker(entry) {
  const all = screenLines(entry, entry.term.rows);
  const start = all.map((l) => CODEX_HEADINGS.test(l)).lastIndexOf(true);
  if (start < 0) return null;
  const heading = all[start].trim();
  const rows = [];
  let highlighted = null;
  for (const l of all.slice(start + 1)) {
    const m = l.match(/^\s*(›)?\s*(\d+)\.\s+(.+?)(?:\s{2,}(.*?))?\s*$/);
    if (!m) continue;
    let label = m[3].trim();
    const current = /\(current\)/.test(label);
    const isDefault = /\(default\)/.test(label);
    label = label.replace(/\s*\((current|default)\)/g, '').trim();
    rows.push({ index: Number(m[2]), label, description: (m[4] || '').trim(), current, isDefault });
    if (m[1]) highlighted = Number(m[2]);
  }
  return { heading, rows, highlighted };
}

async function waitForCodexStep(entry, re) {
  for (let i = 0; i < 25; i++) {
    const p = readCodexPicker(entry);
    if (p && re.test(p.heading) && p.rows.length && p.highlighted) return p;
    await sleep(200);
  }
  return null;
}

// Move the highlight to row `index` of the step showing now and press Enter.
async function chooseCodexRow(entry, index) {
  let p = readCodexPicker(entry);
  for (let attempt = 0; attempt < 3 && p && p.highlighted !== index; attempt++) {
    const delta = index - p.highlighted;
    await pressKey(entry, delta > 0 ? '\x1b[B' : '\x1b[A', Math.abs(delta));
    await sleep(200);
    p = readCodexPicker(entry);
  }
  if (!p || p.highlighted !== index) return false;
  api.write(entry.termId, '\r');
  return true;
}

async function openCodexModelStep(entry) {
  sendText(entry, '/model'); // the queue keeps these one at a time
  return waitForCodexStep(entry, /Select Model/);
}

// Read the effort rows of the step showing now, following "More reasoning…" into Advanced Reasoning.
async function readCodexEfforts(entry) {
  const p = await waitForCodexStep(entry, /Select Reasoning Level/);
  if (!p) return [];
  const efforts = [];
  for (const r of p.rows) {
    if (!/^More reasoning/i.test(r.label)) {
      efforts.push({ path: [r.index], label: r.label, description: r.description, isDefault: r.isDefault });
      continue;
    }
    if (!(await chooseCodexRow(entry, r.index))) continue;
    const adv = await waitForCodexStep(entry, /Advanced Reasoning/);
    for (const a of adv?.rows || []) efforts.push({ path: [r.index, a.index], label: a.label, description: a.description });
    api.write(entry.termId, '\x1b'); // back to the reasoning levels
    await waitForCodexStep(entry, /Select Reasoning Level/);
  }
  return efforts;
}

function loadCodexModels(entry) {
  return queue(entry, async () => {
    const first = await openCodexModelStep(entry);
    if (!first) return null;
    const models = [];
    for (const m of first.rows) {
      await waitForCodexStep(entry, /Select Model/);
      if (!(await chooseCodexRow(entry, m.index))) break;
      models.push({ index: m.index, label: m.label, description: m.description, efforts: await readCodexEfforts(entry) });
      api.write(entry.termId, '\x1b'); // back to the model list
    }
    await waitForCodexStep(entry, /Select Model/);
    api.write(entry.termId, '\x1b'); // close without changing anything
    if (models.length !== first.rows.length) return null;
    try { localStorage.setItem(CODEX_CACHE_KEY, JSON.stringify({ at: Date.now(), models })); } catch { /* ignore */ }
    return models;
  });
}

const codexEffortValue = (label) => (/^extra high$/i.test(label) ? 'xhigh' : label.toLowerCase());

// Pick model + effort in Codex's picker. Enter saves it as Codex's default, like Claude's /model.
function selectCodex(entry, model, effort) {
  return queue(entry, async () => {
    const step = await openCodexModelStep(entry);
    const row = step?.rows.find((r) => r.index === model.index);
    if (!row || row.label !== model.label) {
      if (step) api.write(entry.termId, '\x1b');
      localStorage.removeItem(CODEX_CACHE_KEY);
      $('#composer-hint').textContent = 'Codex\'s model list changed — click again';
      return;
    }
    if (!(await chooseCodexRow(entry, model.index))) return;
    if (!(await waitForCodexStep(entry, /Select Reasoning Level/))) return;
    for (let i = 0; i < effort.path.length; i++) {
      if (i > 0 && !(await waitForCodexStep(entry, /Advanced Reasoning/))) return;
      if (!(await chooseCodexRow(entry, effort.path[i]))) return;
    }
    entry.overrides.model = model.label;
    entry.overrides.effort = codexEffortValue(effort.label);
    renderComposer();
  });
}

async function withCodexModels(entry) {
  let models = cachedCodexModels();
  if (models) return models;
  $('#composer-hint').textContent = 'Reading Codex\'s models and effort levels…';
  models = await loadCodexModels(entry);
  renderComposer();
  if (!models?.length) $('#composer-hint').textContent = 'Couldn\'t read Codex\'s model list — try again when Codex is idle';
  return models;
}

function currentCodexModel(entry, models) {
  const m = (currentSettings(entry).model || '').toLowerCase();
  return models.find((x) => x.label.toLowerCase() === m);
}

async function codexModelMenu(e, entry) {
  const rect = e.currentTarget.getBoundingClientRect();
  const models = await withCodexModels(entry);
  if (!models?.length) return;
  const current = currentCodexModel(entry, models);
  const effortNow = currentSettings(entry).effort;
  showMenu(e, [
    ...models.map((m) => ({
      label: m.label,
      checked: m === current,
      // Keep the current effort if the new model has it, otherwise use that model's default.
      action: () => selectCodex(entry, m,
        m.efforts.find((x) => codexEffortValue(x.label) === effortNow) || m.efforts.find((x) => x.isDefault) || m.efforts[0]),
    })),
    { label: 'Refresh model list', separator: true, action: () => { localStorage.removeItem(CODEX_CACHE_KEY); loadCodexModels(entry); } },
  ], rect);
}

async function codexEffortMenu(e, entry) {
  const rect = e.currentTarget.getBoundingClientRect();
  const models = await withCodexModels(entry);
  if (!models?.length) return;
  const model = currentCodexModel(entry, models);
  if (!model) { $('#composer-hint').textContent = 'Current Codex model not found — use Refresh model list'; return; }
  const effortNow = currentSettings(entry).effort;
  showMenu(e, model.efforts.map((x) => ({
    label: x.label,
    checked: codexEffortValue(x.label) === effortNow,
    action: () => selectCodex(entry, model, x),
  })), rect);
}

// ---------- usage bar ----------

// Plan usage per agent: the 5-hour window first (percent used, time until it resets), then the
// weekly one. Claude's numbers come from Crew's status line hook, Codex's from its own logs.
let usage = {};

function untilText(ms) {
  const m = Math.max(1, Math.round(ms / 60000));
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

const windowName = (min) => (min === 300 ? '5-hour' : min === 10080 ? 'Weekly' : min % 1440 === 0 ? `${min / 1440}-day` : `${Math.round(min / 60)}-hour`);

function usageItem(agent, u) {
  const now = Date.now();
  // A window whose reset time has passed starts again from zero.
  const wins = u.windows.map((w) => (w.resetsAt && w.resetsAt <= now ? { ...w, used: 0, resetsAt: null } : w));
  const [first, second] = wins;
  const pct = (w) => `${Math.round(w.used)}%`;
  const level = (w) => (w.used >= 90 ? 'high' : w.used >= 70 ? 'warn' : '');
  const lines = [`${AGENTS[agent].label}${u.plan ? ` (${u.plan} plan)` : ''}`];
  for (const w of wins) {
    lines.push(`${windowName(w.minutes)} limit: ${pct(w)} used${w.resetsAt ? `, resets in ${untilText(w.resetsAt - now)} (${new Date(w.resetsAt).toLocaleString([], { weekday: 'short', hour: '2-digit', minute: '2-digit' })})` : ''}`);
  }
  lines.push(`Updated ${relTime(u.at) === 'now' ? 'just now' : `${relTime(u.at)} ago`}`);
  const item = el('span', { class: `sb-item${now - u.at > 6 * 3600e3 ? ' stale' : ''}`, title: lines.join('\n') },
    el('span', { class: 'agent-icon', html: AGENTS[agent].badge }),
    el('b', { class: level(first) }, pct(first)),
    first.resetsAt ? el('span', {}, untilText(first.resetsAt - now)) : null);
  if (second) item.append(el('span', { class: 'week' }, `· ${second.minutes === 10080 ? 'week' : windowName(second.minutes)} ${pct(second)}`));
  return item;
}

function renderUsage() {
  const on = state.config.showUsage !== false;
  if (document.body.classList.contains('no-statusbar') === on) {
    document.body.classList.toggle('no-statusbar', !on);
    requestAnimationFrame(() => { for (const t of state.terms.values()) safeFit(t); }); // the bar's height comes from the terminals
  }
  if (!on) return;
  const items = Object.keys(AGENTS).filter((a) => usage[a]?.windows?.length).map((a) => usageItem(a, usage[a]));
  $('#sb-usage').replaceChildren(...(items.length ? items : [el('span', {}, 'Checking plan usage…')]));
}

// Main keeps the numbers and refreshes them in the background (it tells us when they change);
// `force` waits for a fresh check, for the refresh button.
async function loadUsage(force = false) {
  if (state.config.showUsage === false) return;
  if (force) $('#sb-refresh').classList.add('spin');
  try { usage = await api.getUsage(force); } catch { /* keep the last numbers */ }
  if (force) $('#sb-refresh').classList.remove('spin');
  renderUsage();
}
$('#sb-refresh').addEventListener('click', () => loadUsage(true));
api.onUsageChanged(() => loadUsage());

// ---------- boot ----------

(async () => {
  state.config = await api.getConfig();
  applyLanguage(state.config.language); // i18n.js
  applyTheme();
  restoreLayout();
  renderLayout();
  await refresh();
  activate(null);
  if (localStorage.getItem('browserOpen') === '1') openBrowser();
  api.onProjectsChanged(refresh);
  setInterval(renderSidebar, 60_000); // keep relative times fresh
  loadUsage();
  setInterval(() => loadUsage(), 30_000); // also counts the reset times down
  addEventListener('focus', () => loadUsage());
  offerEgoSetup();
})();
