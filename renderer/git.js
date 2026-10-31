/* global api, $, el, state, norm, showMenu, safeFit */
// Git view: a SourceTree-style workspace for the active project's repository. A toolbar with the
// everyday commands, a sidebar of branches, tags, remotes and stashes, the commit graph, and the
// working copy with staging down to single hunks. Every action runs the real git CLI (main.js).

const gitEl = $('#git');
const LOG_PAGE = 1000;
const ROW_H = 24;
const LANE_W = 14;
const DIFF_LINES_SHOWN = 4000;
const LANE_COLORS = ['#4a8fd8', '#d97757', '#3fa45b', '#a46fd6', '#d6a23a', '#3aa8a8', '#d65a8a', '#7f8f3a'];

const git = {
  open: false,
  root: null,
  gitDir: null,
  view: localStorage.getItem('gitView') === 'status' ? 'status' : 'history',
  head: null, // HEAD commit hash, or null before the first commit
  numstat: { staged: new Map(), unstaged: new Map() }, // path -> [lines added, removed]
  foldedSections: {}, // section title -> folded
  branch: '', // current branch; empty when HEAD is detached
  locals: [], // { name, hash, upstream, ahead, behind, gone }
  remoteBranches: [], // { name, remote, hash }
  remotes: [],
  tags: [], // { name, hash }
  stashes: [], // { ref, message }
  operation: null, // 'merge' | 'rebase' | 'cherry-pick' | 'revert' in progress
  commits: [],
  limit: LOG_PAGE,
  staged: [],
  unstaged: [],
  selected: 'wc', // 'wc' (uncommitted changes), a commit hash, or a stash ref
  picked: new Set(), // selected file rows, as `${kind}:${path}`
  anchor: null, // last plainly clicked row, for Shift-click ranges
  file: null, // file whose diff is shown: { kind, path, orig, status }
  fullDiff: false,
  detail: null, // { rev, parent, meta, files } for the selected commit or stash
  detailCache: new Map(),
  refSig: null,
  statusSig: null,
  busy: false,
  reloading: false,
  diffSeq: 0,
  collapsed: (() => { try { return JSON.parse(localStorage.getItem('gitNavCollapsed') || '{}'); } catch { return {}; } })(),
};

const repoRoots = new Map(); // folder -> repository root, for folders known to be in one

const run = (args, input) => api.git(git.root, args, input);
const pad2 = (n) => String(n).padStart(2, '0');
const short = (h) => (h || '').slice(0, 7);
const isHash = (s) => /^[0-9a-f]{40}$/.test(s);
const joinPath = (...parts) => parts.join('\\').replace(/[\\/]+/g, '\\');

function fmtDate(sec) {
  const d = new Date(sec * 1000);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function splitPath(p) {
  const i = p.lastIndexOf('/');
  return i < 0 ? ['', p] : [p.slice(0, i), p.slice(i + 1)];
}

function chunked(base, paths, size = 200) {
  const out = [];
  for (let i = 0; i < paths.length; i += size) out.push([...base, '--', ...paths.slice(i, i + size)]);
  return out;
}

async function repoRoot(folder) {
  if (!folder) return null;
  if (repoRoots.has(folder)) return repoRoots.get(folder);
  const root = await api.gitRoot(folder);
  if (root) repoRoots.set(folder, root);
  return root;
}

// ---------- status bar & dialogs ----------

let barTimer = null;
function gitBar(text, kind) {
  const bar = $('#git-status-bar');
  clearTimeout(barTimer);
  bar.className = kind;
  bar.replaceChildren(el('span', { class: 'git-bar-text' }, text));
  if (kind === 'error') {
    bar.append(el('button', { class: 'icon-btn small', title: 'Dismiss', html: '<svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>', onclick: () => bar.classList.add('hidden') }));
  }
  if (kind === 'ok') barTimer = setTimeout(() => bar.classList.add('hidden'), 4000);
}

// A small form in a modal. Resolves to the field values, or null when cancelled.
function gitDialog({ title, text, fields = [], ok = 'OK', danger = false }) {
  return new Promise((resolve) => {
    const dlg = $('#git-dialog');
    const card = dlg.firstElementChild;
    card.replaceChildren(el('h2', {}, title));
    if (text) card.append(el('p', { class: 'git-dlg-text' }, text));
    const inputs = {};
    for (const f of fields) {
      let input;
      if (f.type === 'check') {
        input = el('input', { type: 'checkbox' });
        input.checked = !!f.value;
        card.append(el('label', { class: 'check' }, input, f.label));
      } else if (f.type === 'select') {
        input = el('select', {}, ...f.options.map((o) => el('option', { value: o }, o)));
        if (f.value) input.value = f.value;
        card.append(el('label', {}, f.label, input));
      } else {
        input = f.type === 'textarea' ? el('textarea', { rows: '3', spellcheck: 'false' }) : el('input', { type: 'text', spellcheck: 'false' });
        input.value = f.value || '';
        if (f.placeholder) input.placeholder = f.placeholder;
        card.append(el('label', {}, f.label, input));
      }
      inputs[f.id] = { input, field: f };
    }
    const done = (v) => {
      dlg.classList.add('hidden');
      document.removeEventListener('keydown', onKey, true);
      resolve(v);
    };
    const submit = () => {
      const v = {};
      for (const [id, { input, field }] of Object.entries(inputs)) {
        v[id] = input.type === 'checkbox' ? input.checked : field.type === 'textarea' ? input.value.trimEnd() : input.value.trim();
        if (field.required && !v[id]) { input.focus(); return; }
      }
      done(v);
    };
    const onKey = (e) => {
      if (e.key === 'Escape') { e.preventDefault(); done(null); }
      if (e.key === 'Enter' && e.target.tagName !== 'TEXTAREA') { e.preventDefault(); submit(); }
    };
    card.append(el('div', { class: 'modal-actions' },
      el('button', { onclick: () => done(null) }, 'Cancel'),
      el('button', { class: `primary${danger ? ' danger' : ''}`, onclick: submit }, ok)));
    dlg.classList.remove('hidden');
    document.addEventListener('keydown', onKey, true);
    requestAnimationFrame(() => (card.querySelector('input[type=text], textarea, select') || card.querySelector('.primary')).focus());
  });
}

const gitConfirm = (title, text, ok, danger = true) => gitDialog({ title, text, ok, danger });

// "Push" -> "Pushing", "Check out main" -> "Checking out main", for the loading overlay.
function progressive(label) {
  const [word, ...rest] = label.split(' ');
  const doubled = { Commit: 'Committing', Pop: 'Popping', Reset: 'Resetting', Stop: 'Stopping' }[word];
  let ing = doubled;
  if (!ing && word === 'Cherry-pick') ing = 'Cherry-picking';
  if (!ing) ing = /[^e]e$/.test(word) ? `${word.slice(0, -1)}ing` : `${word}ing`;
  return [ing, ...rest].join(' ');
}

// A spinner over the view while git works. With `delay`, only for work that isn't instant.
let loadingTimer = null;
function showLoading(text, delay = 0) {
  clearTimeout(loadingTimer);
  const show = () => {
    $('#git-loading-text').textContent = text;
    $('#git-loading').classList.remove('hidden');
  };
  if (delay) loadingTimer = setTimeout(show, delay); else show();
}
function hideLoading() {
  clearTimeout(loadingTimer);
  $('#git-loading').classList.add('hidden');
}

// Runs git commands one after another, stopping at the first failure, then reloads the view.
// Each step is an args array or { args, input }. `quiet` only reports failures.
async function gitOp(label, steps, { quiet = false } = {}) {
  if (git.busy || !git.root) return false;
  git.busy = true;
  gitEl.classList.add('busy');
  showLoading(`${progressive(label)}…`, quiet ? 300 : 0);
  if (!quiet) gitBar(`${label}…`, 'busy');
  let ok = true;
  let msg = '';
  try {
    for (const s of steps) {
      const { args, input } = Array.isArray(s) ? { args: s } : s;
      const r = await run(args, input);
      msg = (r.stderr || r.stdout).trim();
      if (r.code !== 0) { ok = false; break; }
    }
    await gitReload(true);
  } finally {
    git.busy = false;
    gitEl.classList.remove('busy');
    hideLoading(); // after the reload, so the view isn't briefly out of date
  }
  if (!ok) gitBar(`${label} failed. ${msg || 'git exited with an error.'}`, 'error');
  else if (!quiet) gitBar(`${label}: done.${msg ? ` ${msg.split('\n').filter(Boolean).pop()}` : ''}`, 'ok');
  return ok;
}

// ---------- loading ----------

function parseStatus(out) {
  const parts = out.split('\0');
  const staged = [];
  const unstaged = [];
  for (let i = 0; i < parts.length; i++) {
    const s = parts[i];
    if (s.length < 4) continue;
    const x = s[0];
    const y = s[1];
    const p = s.slice(3);
    const orig = x === 'R' || x === 'C' ? parts[++i] : undefined;
    const conflict = x === 'U' || y === 'U' || (x === 'A' && y === 'A') || (x === 'D' && y === 'D');
    if (conflict) { unstaged.push({ path: p, status: 'U' }); continue; }
    if (x === '?') { unstaged.push({ path: p, status: '?' }); continue; }
    if (x !== ' ' && x !== '!') staged.push({ path: p, orig, status: x });
    if (y !== ' ' && y !== '!') unstaged.push({ path: p, status: y });
  }
  const byPath = (a, b) => a.path.localeCompare(b.path);
  git.staged = staged.sort(byPath);
  git.unstaged = unstaged.sort(byPath);
}

function parseTrack(track) {
  return {
    ahead: +(/ahead (\d+)/.exec(track)?.[1] || 0),
    behind: +(/behind (\d+)/.exec(track)?.[1] || 0),
    gone: track.includes('gone'),
  };
}

async function loadRefs() {
  const [sym, refs, stash, remotes] = await Promise.all([
    run(['symbolic-ref', '-q', '--short', 'HEAD']),
    run(['for-each-ref', '--format=%(refname)%00%(refname:short)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track)', 'refs/heads', 'refs/remotes', 'refs/tags']),
    run(['stash', 'list', '--format=%gd%x00%s']),
    run(['remote']),
  ]);
  git.branch = sym.code === 0 ? sym.stdout.trim() : '';
  git.locals = [];
  git.remoteBranches = [];
  git.tags = [];
  for (const line of refs.stdout.split('\n')) {
    if (!line) continue;
    const [full, name, obj, deref, upstream, track] = line.split('\0');
    if (full.startsWith('refs/heads/')) git.locals.push({ name, hash: obj, upstream, ...parseTrack(track) });
    else if (full.startsWith('refs/remotes/')) {
      if (full.endsWith('/HEAD')) continue;
      git.remoteBranches.push({ name, remote: name.split('/')[0], hash: obj });
    } else if (full.startsWith('refs/tags/')) git.tags.push({ name, hash: deref || obj });
  }
  git.tags.sort((a, b) => b.name.localeCompare(a.name, undefined, { numeric: true }));
  git.stashes = stash.stdout.split('\n').filter(Boolean).map((l) => {
    const [ref, message] = l.split('\0');
    return { ref, message };
  });
  git.remotes = remotes.stdout.split('\n').map((s) => s.trim()).filter(Boolean);

  const ops = [['MERGE_HEAD', 'merge'], ['rebase-merge', 'rebase'], ['rebase-apply', 'rebase'], ['CHERRY_PICK_HEAD', 'cherry-pick'], ['REVERT_HEAD', 'revert']];
  git.operation = null;
  for (const [file, op] of ops) {
    if (git.gitDir && (await api.fileExists(joinPath(git.gitDir, file)))) { git.operation = op; break; }
  }
}

async function loadLog() {
  const args = ['log', '--date-order', `-n${git.limit}`, '--format=%x1e%H%x00%P%x00%D%x00%an%x00%ae%x00%at%x00%s', '--branches', '--remotes', '--tags'];
  if (git.head) args.push('HEAD');
  const r = await run(args);
  const locals = new Set(git.locals.map((b) => b.name));
  git.commits = r.code !== 0 ? [] : r.stdout.split('\x1e').filter((s) => s.trim()).map((rec) => {
    const [hash, parents, refs, author, email, at, subject] = rec.replace(/\n$/, '').split('\0');
    return {
      hash,
      parents: parents ? parents.split(' ') : [],
      refs: refs ? refs.split(', ').map((s) => {
        if (s.startsWith('HEAD -> ')) return { kind: 'head', name: s.slice(8) };
        if (s === 'HEAD') return { kind: 'detached', name: 'HEAD' };
        if (s.startsWith('tag: ')) return { kind: 'tag', name: s.slice(5) };
        return { kind: locals.has(s) ? 'local' : 'remote', name: s };
      }) : [],
      author,
      email,
      at: +at,
      subject,
    };
  });
  git.graphLanes = buildGraph(git.commits);
}

// Lays the commits out in lanes: each lane holds the commit it's waiting for. A commit takes the
// first lane waiting for it (others waiting for it join in), its first parent continues in that
// lane, and further parents (merges) get their own lane.
function buildGraph(commits) {
  let lanes = [];
  let max = 1;
  for (const c of commits) {
    const before = lanes;
    let col = before.indexOf(c.hash);
    if (col < 0) { col = before.indexOf(null); if (col < 0) col = before.length; }
    const after = before.map((h) => (h === c.hash ? null : h));
    after[col] = c.parents[0] || null;
    const parentCols = c.parents.length ? [col] : [];
    for (const p of c.parents.slice(1)) {
      let j = after.indexOf(p);
      if (j < 0) { j = after.indexOf(null); if (j < 0) j = after.length; after[j] = p; }
      parentCols.push(j);
    }
    for (let i = 0; i < after.length; i++) if (after[i] === undefined) after[i] = null;
    while (after.length && after[after.length - 1] === null) after.pop();
    c.graph = { col, before, parentCols };
    max = Math.max(max, before.length, after.length, col + 1);
    lanes = after;
  }
  return max;
}

// Reloads what changed since last time: the file status, or the refs and history.
// Lines added / removed per file (git diff --numstat), for the file rows and the header. Binary
// files and untracked ones have none.
async function loadNumstat() {
  const parse = (out) => {
    const m = new Map();
    for (const line of out.split('\0').filter(Boolean)) {
      const [a, d, p] = line.split('\t');
      if (p && a !== '-') m.set(p, [+a || 0, +d || 0]);
    }
    return m;
  };
  const [st, un] = await Promise.all([run(['diff', '--cached', '--numstat', '-z', '--no-renames']), run(['diff', '--numstat', '-z', '--no-renames'])]);
  git.numstat = { staged: parse(st.stdout || ''), unstaged: parse(un.stdout || '') };
}

async function gitReload(force = false) {
  if (!git.open || !git.root) return;
  if (git.reloading) { if (force) git.forceAgain = true; return; } // run again once this one is done
  git.reloading = true;
  const root = git.root;
  try {
    const [st, refs, head] = await Promise.all([
      run(['status', '--porcelain=v1', '-z', '-uall']),
      run(['for-each-ref', '--format=%(objectname) %(refname)']),
      run(['rev-parse', '-q', '--verify', 'HEAD']),
    ]);
    if (root !== git.root) return;
    const refSig = refs.stdout + head.stdout;
    const refsChanged = force || refSig !== git.refSig;
    const statusChanged = force || st.stdout !== git.statusSig;
    git.refSig = refSig;
    git.statusSig = st.stdout;
    git.head = head.code === 0 ? head.stdout.trim() : null;
    if (statusChanged) { parseStatus(st.stdout); await loadNumstat(); }
    if (refsChanged) {
      await loadRefs();
      await loadLog();
      if (root !== git.root) return;
    }
    if (!refsChanged && !statusChanged) return;
    if (git.selected !== 'wc' && !git.commits.some((c) => c.hash === git.selected) && !git.stashes.some((s) => s.ref === git.selected)) git.selected = 'wc';
    renderGit({ logToo: refsChanged || statusChanged, detailToo: refsChanged || git.selected === 'wc' || git.view === 'status' });
  } finally {
    git.reloading = false;
    if (git.forceAgain) { git.forceAgain = false; gitReload(true); }
  }
}

// ---------- open / close ----------

async function openGit(folder) {
  let root = await repoRoot(folder);
  if (!root) {
    const ok = await gitConfirm('Not a Git repository', `${folder} isn't managed by Git yet. Create a repository here with git init?`, 'Initialize', false);
    if (!ok) return;
    const r = await api.git(folder, ['init']);
    if (r.code !== 0) { gitBar(`git init failed. ${r.stderr}`, 'error'); return; }
    root = await repoRoot(folder);
    if (!root) return;
  }
  if (typeof files !== 'undefined' && files.open) closeFiles(); // they share the right side (files.js)
  const switching = norm(root) !== norm(git.root);
  git.root = root;
  if (switching) {
    Object.assign(git, { selected: 'wc', picked: new Set(), file: null, detail: null, refSig: null, statusSig: null, limit: LOG_PAGE, commits: [], staged: [], unstaged: [] });
    git.detailCache.clear();
    $('#git-log-rows').replaceChildren();
    $('#git-files').replaceChildren();
    showDiffPlaceholder();
    const gd = await api.git(root, ['rev-parse', '--absolute-git-dir']);
    git.gitDir = gd.code === 0 ? gd.stdout.trim() : null;
  }
  if (!git.open) {
    git.open = true;
    // Until the user drags the edge, take a bit over half of the room beside the sidebar.
    if (!localStorage.getItem('gitWidth')) gitEl.style.width = `${Math.max(460, Math.round((innerWidth - $('#sidebar').getBoundingClientRect().right) * 0.56))}px`;
    gitEl.classList.remove('hidden');
    $('#git-resizer').classList.remove('hidden');
    document.body.classList.add('git-open');
    $('#btn-git').classList.remove('hidden');
    fitTerminals();
  }
  gitHeader();
  if (switching) showLoading('Loading repository…', 200);
  try { await gitReload(true); } finally { hideLoading(); }
  if (switching && git.selected === 'wc' && !git.staged.length && !git.unstaged.length && git.head) selectRow(git.head);
}

function closeGit() {
  if (!git.open) return;
  git.open = false;
  gitEl.classList.add('hidden');
  $('#git-resizer').classList.add('hidden');
  document.body.classList.remove('git-open');
  updateGitButton();
  fitTerminals();
}

// The terminals get narrower or wider as the panel opens, closes or is resized.
function fitTerminals() {
  requestAnimationFrame(() => { for (const t of state.terms.values()) safeFit(t); });
}

function gitHeader() {
  if (!git.open) return;
  const repo = git.root.split(/[\\/]/).filter(Boolean).pop();
  $('#git-title').textContent = git.branch || (git.head ? `detached at ${short(git.head)}` : 'no commits yet');
  $('#git-title').title = `${repo} · ${git.root}`;
  // The lines added and removed by the uncommitted changes, staged and not.
  let add = 0;
  let del = 0;
  for (const m of [git.numstat.staged, git.numstat.unstaged]) for (const [a, d] of m.values()) { add += a; del += d; }
  $('#git-stats').replaceChildren(...(add || del ? [el('span', { class: 'git-add' }, `+${add}`), el('span', { class: 'git-del' }, `-${del}`)] : []));
  const current = git.locals.find((b) => b.name === git.branch);
  const track = [current?.ahead && `↑${current.ahead}`, current?.behind && `↓${current.behind}`].filter(Boolean).join(' ');
  $('#git-sub').textContent = current?.upstream ? `→ ${current.upstream}${track ? `  ${track}` : ''}` : repo;
  $('#git-sub').title = git.root;
}

// Called by activate(): show the Git button when the active session's folder is a repository,
// and follow the active session to its repository while the Git view is open.
async function updateGitButton() {
  const cwd = state.terms.get(state.active)?.cwd;
  const root = await repoRoot(cwd);
  if (state.terms.get(state.active)?.cwd !== cwd) return;
  $('#btn-git').classList.toggle('hidden', !root && !git.open);
  if (!git.open) return;
  if (root && norm(root) !== norm(git.root)) openGit(root);
}

$('#btn-git').addEventListener('click', () => {
  if (git.open) closeGit();
  else openGit(state.terms.get(state.active)?.cwd || git.root);
});
$('#git-close').addEventListener('click', closeGit);

setInterval(() => { if (git.open && !document.hidden && !git.busy) gitReload(); }, 3000);
window.addEventListener('focus', () => { if (git.open && !git.busy) gitReload(); });

// ---------- rendering ----------

function renderGit({ logToo = true, detailToo = true } = {}) {
  gitHeader();
  renderToolbar();
  renderNav();
  const status = git.view === 'status';
  $('#git-log').classList.toggle('hidden', status);
  $('#git-split').classList.toggle('hidden', status);
  $('#git-commit-box').classList.toggle('hidden', !status);
  if (logToo && !status) renderLog();
  if (detailToo) renderDetail();
}

function setView(view) {
  git.view = view;
  localStorage.setItem('gitView', view);
  if (view === 'status') git.selected = 'wc';
  renderGit();
}

function setBadge(id, n) {
  const b = $(`${id} .badge`);
  b.textContent = n > 99 ? '99+' : String(n);
  b.classList.toggle('hidden', !n);
}

function renderToolbar() {
  const current = git.locals.find((b) => b.name === git.branch);
  const changes = new Set([...git.staged, ...git.unstaged].map((f) => f.path)).size;
  setBadge('#gt-commit', changes);
  setBadge('#gt-pull', current?.behind || 0);
  setBadge('#gt-push', current?.ahead || 0);
}

function navGroup(key, title, items, count = items.length) {
  const collapsed = !!git.collapsed[key];
  const head = el('div', {
    class: `git-nav-group${collapsed ? ' collapsed' : ''}`,
    onclick: () => {
      git.collapsed[key] = !collapsed;
      localStorage.setItem('gitNavCollapsed', JSON.stringify(git.collapsed));
      renderNav();
    },
  }, el('span', { html: '<svg class="chev" viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>' }), el('span', {}, title), el('span', { class: 'count' }, String(count)));
  const box = el('div', { class: 'git-nav-items' });
  if (!collapsed) box.append(...items);
  return [head, box];
}

function navItem({ label, title, active, current, extra, onclick, ondblclick, oncontextmenu, indent }) {
  const item = el('div', {
    class: `git-nav-item${active ? ' active' : ''}${current ? ' current' : ''}${indent ? ' indent' : ''}`,
    title: title || label,
    onclick,
  }, el('span', { class: 'label' }, label));
  if (extra) item.append(el('span', { class: 'extra' }, extra));
  if (ondblclick) item.addEventListener('dblclick', ondblclick);
  if (oncontextmenu) item.addEventListener('contextmenu', (e) => { e.preventDefault(); oncontextmenu(e); });
  return item;
}

function renderNav() {
  const nav = $('#git-nav');
  const scroll = nav.scrollTop;
  nav.replaceChildren();
  const changes = new Set([...git.staged, ...git.unstaged].map((f) => f.path)).size;
  nav.append(el('div', { class: 'git-nav-title' }, 'Workspace'),
    navItem({ label: 'File status', active: git.view === 'status', extra: changes ? String(changes) : '', onclick: () => setView('status') }),
    navItem({ label: 'History', active: git.view === 'history', onclick: () => setView('history') }));

  nav.append(...navGroup('branches', 'Branches', git.locals.map((b) => navItem({
    label: b.name,
    current: b.name === git.branch,
    title: `${b.name}${b.upstream ? ` → ${b.upstream}` : ''}${b.gone ? ' (upstream gone)' : ''}\nDouble-click to check out`,
    extra: [b.ahead && `↑${b.ahead}`, b.behind && `↓${b.behind}`].filter(Boolean).join(' '),
    onclick: () => jumpTo(b.hash),
    ondblclick: () => checkout(b.name),
    oncontextmenu: (e) => branchMenu(e, b),
  }))));

  nav.append(...navGroup('tags', 'Tags', git.tags.map((t) => navItem({
    label: t.name,
    onclick: () => jumpTo(t.hash),
    oncontextmenu: (e) => tagMenu(e, t),
  }))));

  const remoteItems = [];
  for (const remote of git.remotes) {
    remoteItems.push(navItem({ label: remote, oncontextmenu: (e) => showMenu(e, [{ label: `Fetch ${remote}`, action: () => gitOp(`Fetch ${remote}`, [['fetch', '--prune', remote]]) }]) }));
    for (const b of git.remoteBranches.filter((x) => x.remote === remote)) {
      remoteItems.push(navItem({
        label: b.name.slice(remote.length + 1),
        title: `${b.name}\nDouble-click to check out as a local branch`,
        indent: true,
        onclick: () => jumpTo(b.hash),
        ondblclick: () => checkoutRemote(b),
        oncontextmenu: (e) => remoteBranchMenu(e, b),
      }));
    }
  }
  nav.append(...navGroup('remotes', 'Remotes', remoteItems, git.remoteBranches.length));

  nav.append(...navGroup('stashes', 'Stashes', git.stashes.map((s) => navItem({
    label: s.message.replace(/^(WIP on|On) [^:]+: /, '') || s.ref,
    title: `${s.ref}: ${s.message}`,
    active: git.selected === s.ref && git.view === 'history',
    onclick: () => { if (git.view !== 'history') setView('history'); selectRow(s.ref); },
    oncontextmenu: (e) => stashMenu(e, s),
  }))));
  nav.scrollTop = scroll;
}

function laneX(i) { return 10 + i * LANE_W; }

function graphSvg(c, width, { topLine = false } = {}) {
  const mid = ROW_H / 2;
  const color = (i) => LANE_COLORS[i % LANE_COLORS.length];
  const curve = (x1, y1, x2, y2, col) => (x1 === x2
    ? `<path d="M${x1} ${y1}V${y2}" stroke="${col}"/>`
    : `<path d="M${x1} ${y1}C${x1} ${(y1 + y2) / 2} ${x2} ${(y1 + y2) / 2} ${x2} ${y2}" stroke="${col}"/>`);
  let s = '';
  const { col, before, parentCols } = c.graph;
  before.forEach((h, i) => {
    if (h == null) return;
    if (h === c.hash) s += curve(laneX(i), 0, laneX(col), mid, color(i));
    else s += curve(laneX(i), 0, laneX(i), ROW_H, color(i));
  });
  if (topLine) s += curve(laneX(col), 0, laneX(col), mid, '#8a8580');
  for (const pc of parentCols) s += curve(laneX(col), mid, laneX(pc), ROW_H, color(pc));
  const head = c.hash === git.head;
  s += `<circle cx="${laneX(col)}" cy="${mid}" r="${head ? 4.5 : 3.5}" fill="${head ? 'var(--main-bg)' : color(col)}" stroke="${color(col)}" stroke-width="${head ? 2.5 : 1}"/>`;
  return `<svg class="git-graph" width="${width}" height="${ROW_H}" viewBox="0 0 ${width} ${ROW_H}">${s}</svg>`;
}

function refBadge(r) {
  const cls = { head: 'local head', local: 'local', remote: 'remote', tag: 'tag', detached: 'head' }[r.kind];
  return el('span', { class: `git-ref ${cls}`, title: r.kind === 'tag' ? `Tag ${r.name}` : r.name }, r.name);
}

function logRow({ graph, desc, date = '', author = '', hash = '', title, key, oncontextmenu, cls = '' }) {
  const row = el('div', {
    class: `git-row${git.selected === key ? ' selected' : ''}${cls ? ` ${cls}` : ''}`,
    onclick: () => selectRow(key),
  },
  el('span', { class: 'c-graph', html: graph }),
  el('span', { class: 'c-desc' }, ...desc),
  el('span', { class: 'c-date' }, date),
  el('span', { class: 'c-author', title: title || author }, author),
  el('span', { class: 'c-hash' }, hash));
  row.dataset.key = key;
  if (oncontextmenu) row.addEventListener('contextmenu', (e) => { e.preventDefault(); selectRow(key); oncontextmenu(e); });
  return row;
}

function renderLog() {
  const rows = $('#git-log-rows');
  const scroll = rows.scrollTop;
  const width = Math.min(git.graphLanes || 1, 16) * LANE_W + 12;
  gitEl.style.setProperty('--graph-w', `${Math.max(width, 64)}px`);
  const out = [];
  const dirty = git.staged.length || git.unstaged.length;
  if (dirty || !git.head) {
    const n = new Set([...git.staged, ...git.unstaged].map((f) => f.path)).size;
    out.push(logRow({
      key: 'wc',
      cls: 'wc',
      graph: `<svg class="git-graph" width="${width}" height="${ROW_H}"><circle cx="${laneX(0)}" cy="${ROW_H / 2}" r="4" fill="var(--main-bg)" stroke="#8a8580" stroke-width="1.5" stroke-dasharray="2 2"/>${git.commits[0]?.hash === git.head && git.commits[0]?.graph.col === 0 ? `<path d="M${laneX(0)} ${ROW_H / 2 + 4}V${ROW_H}" stroke="#8a8580" stroke-dasharray="2 2"/>` : ''}</svg>`,
      desc: [el('b', {}, 'Uncommitted changes'), el('span', { class: 'muted' }, n ? ` · ${n} file${n === 1 ? '' : 's'}` : ' · no commits yet')],
      date: '*',
      author: '*',
      hash: '*',
    }));
  }
  const linkTop = dirty && git.commits[0]?.hash === git.head;
  git.commits.forEach((c, i) => {
    out.push(logRow({
      key: c.hash,
      graph: graphSvg(c, width, { topLine: linkTop && i === 0 && c.graph.col === 0 }),
      desc: [...c.refs.map(refBadge), el('span', { class: 'subject' }, c.subject)],
      date: fmtDate(c.at),
      author: c.author,
      title: `${c.author} <${c.email}>`,
      hash: short(c.hash),
      cls: c.hash === git.head ? 'head' : '',
      oncontextmenu: (e) => commitMenu(e, c),
    }));
  });
  if (git.commits.length >= git.limit) {
    out.push(el('div', { class: 'git-more', onclick: async () => { git.limit += LOG_PAGE; await loadLog(); renderLog(); } }, `Showing the latest ${git.limit} commits. Load more`));
  }
  if (!git.commits.length && !dirty) out.push(el('div', { class: 'git-placeholder' }, 'No commits yet'));
  rows.replaceChildren(...out);
  rows.scrollTop = scroll;
}

function selectRow(key) {
  if (git.selected !== key) { git.picked = new Set(); git.file = null; git.anchor = null; }
  git.selected = key;
  for (const r of document.querySelectorAll('#git-log-rows .git-row')) r.classList.toggle('selected', r.dataset.key === key);
  if (git.stashes.some((s) => s.ref === key) || git.view === 'history') renderNav();
  renderDetail();
}

function jumpTo(hash) {
  if (git.view !== 'history') setView('history');
  selectRow(hash);
  const row = [...document.querySelectorAll('#git-log-rows .git-row')].find((r) => r.dataset.key === hash);
  if (row) row.scrollIntoView({ block: 'center' });
  else gitBar('That commit is older than the commits loaded. Use "Load more" at the end of the history.', 'ok');
}

$('#git-log-rows').addEventListener('keydown', (e) => {
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
  e.preventDefault();
  const rows = [...document.querySelectorAll('#git-log-rows .git-row')];
  const i = rows.findIndex((r) => r.dataset.key === git.selected);
  const next = rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
  if (next) { selectRow(next.dataset.key); next.scrollIntoView({ block: 'nearest' }); }
});

// ---------- details: working copy or a commit ----------

function renderDetail() {
  if (git.view === 'status' || git.selected === 'wc') renderWorkingCopy();
  else renderCommit(git.selected);
}

const GIT_FILE_ICON = '<svg viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>';
const STATUS_NAMES = { M: 'Modified', A: 'Added', D: 'Deleted', R: 'Renamed', C: 'Copied', T: 'Type changed', U: 'Conflicted', '?': 'Untracked' };

function fileRow(f, kind, list) {
  const key = `${kind}:${f.path}`;
  const [dir, base] = splitPath(f.path);
  const shown = git.file && git.file.kind === kind && git.file.path === f.path;
  const row = el('div', {
    class: `git-file${git.picked.has(key) ? ' picked' : ''}${shown ? ' shown' : ''}`,
    title: `${STATUS_NAMES[f.status] || f.status}: ${f.orig ? `${f.orig} → ` : ''}${f.path}`,
    onclick: (e) => pickFile(e, f, kind, list),
    oncontextmenu: (e) => { e.preventDefault(); if (!git.picked.has(key)) pickFile({}, f, kind, list); fileMenu(e, f, kind); },
  },
  el('span', { class: 'git-ficon', html: GIT_FILE_ICON }),
  el('span', { class: 'git-fname' }, base),
  el('span', { class: 'git-fdir' }, f.orig ? `${f.orig} →` : dir));
  const counts = kind === 'commit' ? null : git.numstat[kind]?.get(f.path);
  row.append(el('span', { class: 'git-fcount' }, ...(counts ? [counts[0] ? el('span', { class: 'git-add' }, `+${counts[0]}`) : null, counts[1] ? el('span', { class: 'git-del' }, `-${counts[1]}`) : null] : [])),
    el('span', { class: `git-st st-${f.status === '?' ? 'q' : f.status}` }, f.status === '?' ? 'U' : f.status));
  if (kind !== 'commit') {
    row.addEventListener('dblclick', () => (kind === 'staged' ? unstage([f]) : stage([f])));
    row.append(el('button', {
      class: 'git-file-act',
      title: kind === 'staged' ? 'Unstage' : 'Stage',
      onclick: (e) => { e.stopPropagation(); (kind === 'staged' ? unstage : stage)([f]); },
    }, kind === 'staged' ? '−' : '+'));
  }
  return row;
}

function pickFile(e, f, kind, list) {
  const key = `${kind}:${f.path}`;
  if (e.shiftKey && git.anchor?.kind === kind) {
    const a = list.findIndex((x) => x.path === git.anchor.path);
    const b = list.findIndex((x) => x.path === f.path);
    git.picked = new Set(list.slice(Math.min(a, b), Math.max(a, b) + 1).map((x) => `${kind}:${x.path}`));
  } else if (e.ctrlKey) {
    if (git.picked.has(key)) git.picked.delete(key); else git.picked.add(key);
    git.anchor = { kind, path: f.path };
  } else {
    git.picked = new Set([key]);
    git.anchor = { kind, path: f.path };
  }
  git.file = { ...f, kind };
  git.fullDiff = false;
  if (kind === 'commit') renderCommitFiles(); else renderWorkingCopy();
}

const pickedOf = (kind, list) => list.filter((f) => git.picked.has(`${kind}:${f.path}`));

function section(title, count, buttons, rows, emptyText) {
  const folded = !!git.foldedSections[title];
  const head = el('div', { class: `git-section-head${folded ? ' folded' : ''}` },
    el('span', { class: 'git-section-chev', html: '<svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>' }),
    el('span', { class: 'git-section-title' }, title),
    el('span', { class: 'git-section-count' }, String(count)),
    el('span', { class: 'spacer' }), ...buttons);
  head.addEventListener('click', (e) => {
    if (e.target.closest('button')) return;
    git.foldedSections[title] = !folded;
    renderDetail();
  });
  return el('div', { class: 'git-section' }, head,
    folded ? null : el('div', { class: 'git-file-list' }, ...(rows.length ? rows : [el('div', { class: 'git-empty' }, emptyText)])));
}

// A section's action as a small icon, its name in the tooltip.
const GIT_ACT_ICONS = {
  minus: '<svg viewBox="0 0 24 24"><path d="M6 12h12"/></svg>',
  plus: '<svg viewBox="0 0 24 24"><path d="M12 6v12M6 12h12"/></svg>',
  minusAll: '<svg viewBox="0 0 24 24"><rect x="4" y="4" width="16" height="16" rx="3"/><path d="M8 12h8"/></svg>',
  plusAll: '<svg viewBox="0 0 24 24"><rect x="4" y="4" width="16" height="16" rx="3"/><path d="M12 8v8M8 12h8"/></svg>',
};
const iconBtn = (icon, title, onclick, disabled) => {
  const b = el('button', { class: 'git-icon-act', title, html: GIT_ACT_ICONS[icon], onclick });
  b.disabled = !!disabled;
  return b;
};

const smallBtn = (label, onclick, disabled) => {
  const b = el('button', { class: 'git-small', onclick }, label);
  b.disabled = !!disabled;
  return b;
};

function renderWorkingCopy() {
  const box = $('#git-files');
  const scroll = [...box.querySelectorAll('.git-file-list')].map((l) => l.scrollTop);
  // Keep the shown file when it moved between the lists (e.g. it was just staged).
  if (git.file && git.file.kind !== 'commit') {
    const same = (git.file.kind === 'staged' ? git.staged : git.unstaged).find((f) => f.path === git.file.path);
    const other = (git.file.kind === 'staged' ? git.unstaged : git.staged).find((f) => f.path === git.file.path);
    if (same) git.file = { ...same, kind: git.file.kind };
    else if (other) git.file = { ...other, kind: git.file.kind === 'staged' ? 'unstaged' : 'staged' };
    else git.file = null;
  } else git.file = null;
  const live = new Set([...git.staged.map((f) => `staged:${f.path}`), ...git.unstaged.map((f) => `unstaged:${f.path}`)]);
  git.picked = new Set([...git.picked].filter((k) => live.has(k)));

  const parts = [];
  if (git.operation) {
    const actions = [smallBtn(`Abort ${git.operation}`, () => abortOperation())];
    if (git.operation === 'rebase' || git.operation === 'cherry-pick' || git.operation === 'revert') actions.unshift(smallBtn('Continue', () => continueOperation()));
    parts.push(el('div', { class: 'git-banner' }, el('span', {}, `A ${git.operation} is in progress. Resolve any conflicts and stage the files, then ${git.operation === 'merge' ? 'commit' : 'continue'}.`), ...actions));
  }
  const stagedPicked = pickedOf('staged', git.staged);
  const unstagedPicked = pickedOf('unstaged', git.unstaged);
  parts.push(section('Staged changes', git.staged.length, [
    iconBtn('minus', 'Unstage selected', () => unstage(stagedPicked), !stagedPicked.length),
    iconBtn('minusAll', 'Unstage all', () => unstage(git.staged, true), !git.staged.length),
  ], git.staged.map((f) => fileRow(f, 'staged', git.staged)), 'Nothing staged'));
  parts.push(section('Changes', git.unstaged.length, [
    iconBtn('plus', 'Stage selected', () => stage(unstagedPicked), !unstagedPicked.length),
    iconBtn('plusAll', 'Stage all', () => stage(git.unstaged, true), !git.unstaged.length),
  ], git.unstaged.map((f) => fileRow(f, 'unstaged', git.unstaged)), 'No changes'));
  box.replaceChildren(...parts);
  box.querySelectorAll('.git-file-list').forEach((l, i) => { l.scrollTop = scroll[i] || 0; });
  box.classList.add('wc');
  if (git.file) loadDiff(git.file);
  else showDiffPlaceholder();
}

async function renderCommit(rev) {
  const box = $('#git-files');
  box.classList.remove('wc');
  if (git.detail?.rev !== rev) {
    git.file = null;
    box.replaceChildren(el('div', { class: 'git-placeholder' }, 'Loading…'));
    showDiffPlaceholder();
    const detail = git.detailCache.get(rev) || (await loadCommitDetail(rev));
    if (git.selected !== rev) return;
    git.detail = detail;
    if (isHash(rev) && detail) git.detailCache.set(rev, detail);
  }
  renderCommitFiles();
}

async function loadCommitDetail(rev) {
  const meta = await run(['show', '-s', '--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B', rev]);
  if (meta.code !== 0) return null;
  const [hash, parents, an, ae, at, cn, ce, ct, body] = meta.stdout.split('\0');
  const parent = parents ? parents.split(' ')[0] : null;
  const r = parent
    ? await run(['diff', '--name-status', '-z', '-M', parent, hash])
    : await run(['diff-tree', '--root', '-r', '--name-status', '-z', '-M', '--no-commit-id', hash]);
  const tok = r.stdout.split('\0').filter((s) => s !== '');
  const files = [];
  for (let i = 0; i < tok.length; i++) {
    const st = tok[i];
    if (st[0] === 'R' || st[0] === 'C') { files.push({ status: st[0], orig: tok[i + 1], path: tok[i + 2] }); i += 2; } else { files.push({ status: st[0], path: tok[i + 1] }); i += 1; }
  }
  return {
    rev,
    hash,
    parent,
    parents: parents ? parents.split(' ') : [],
    author: `${an} <${ae}>`,
    committer: `${cn} <${ce}>`,
    at: +at,
    ct: +ct,
    body: body.trim(),
    files,
  };
}

function renderCommitFiles() {
  const box = $('#git-files');
  const d = git.detail;
  if (!d) { box.replaceChildren(el('div', { class: 'git-placeholder' }, 'Couldn\'t load this commit.')); return; }
  const scroll = box.querySelector('.git-file-list')?.scrollTop || 0;
  const [subject, ...rest] = d.body.split('\n');
  const metaRow = (k, v) => el('div', { class: 'git-meta-row' }, el('span', { class: 'k' }, k), v);
  const hashLink = (h) => el('a', { class: 'git-hash-link', title: 'Show this commit', onclick: () => jumpTo(h) }, short(h));
  const stash = git.stashes.find((s) => s.ref === d.rev);
  const meta = el('div', { class: 'git-meta' },
    el('div', { class: 'git-meta-subject' }, stash ? `${stash.ref}: ${subject}` : subject),
    rest.join('\n').trim() ? el('div', { class: 'git-meta-body' }, rest.join('\n').trim()) : null,
    metaRow('Commit', el('span', { class: 'mono sel' }, d.hash)),
    d.parents.length ? metaRow(d.parents.length > 1 ? 'Parents' : 'Parent', el('span', {}, ...d.parents.flatMap((p, i) => (i ? [', ', hashLink(p)] : [hashLink(p)])))) : null,
    metaRow('Author', el('span', { class: 'sel' }, d.author)),
    metaRow('Date', el('span', {}, fmtDate(d.at))),
    d.committer !== d.author ? metaRow('Committer', el('span', { class: 'sel' }, d.committer)) : null);
  if (git.file && !d.files.some((f) => f.path === git.file.path)) git.file = null;
  const rows = d.files.map((f) => fileRow(f, 'commit', d.files));
  const note = d.parents.length > 1 ? el('div', { class: 'git-empty' }, 'Changes compared with the first parent.') : null;
  box.replaceChildren(meta, el('div', { class: 'git-section' },
    el('div', { class: 'git-section-head' }, el('span', {}, `Files (${d.files.length})`)),
    el('div', { class: 'git-file-list' }, note, ...rows)));
  box.querySelector('.git-file-list').scrollTop = scroll;
  if (git.file) loadDiff(git.file);
  else showDiffPlaceholder();
}

// ---------- diff ----------

function showDiffPlaceholder(text = 'Select a file to view the diff') {
  git.diffSeq++;
  $('#git-diff').replaceChildren(el('div', { class: 'git-placeholder' }, text));
}

async function loadDiff(f) {
  const seq = ++git.diffSeq;
  const paths = f.orig ? [f.orig, f.path] : [f.path];
  let args;
  let okCodes = [0];
  if (f.kind === 'staged') args = ['diff', '--cached', '-M', '--', ...paths];
  else if (f.kind === 'unstaged') {
    if (f.status === '?') { args = ['diff', '--no-index', '--', '/dev/null', f.path]; okCodes = [0, 1]; } else args = ['diff', '--', f.path];
  } else {
    const d = git.detail;
    args = d.parent ? ['diff', '-M', d.parent, d.hash, '--', ...paths] : ['diff-tree', '-p', '--root', '-M', '--no-commit-id', d.hash, '--', ...paths];
  }
  const r = await run(args);
  if (seq !== git.diffSeq) return;
  if (!okCodes.includes(r.code)) { showDiffPlaceholder(r.stderr.trim() || 'Couldn\'t load the diff.'); return; }
  renderDiff(r.stdout, f);
}

function parseDiff(text) {
  const lines = text.split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  const header = [];
  const hunks = [];
  let h = null;
  for (const l of lines) {
    if (l.startsWith('@@')) { h = { head: l, lines: [] }; hunks.push(h); } else if (h) h.lines.push(l); else header.push(l);
  }
  return { header, hunks };
}

function renderDiff(text, f) {
  const box = $('#git-diff');
  const sameFile = box.dataset.file === `${f.kind}:${f.path}`;
  const scroll = sameFile ? box.scrollTop : 0;
  box.dataset.file = `${f.kind}:${f.path}`;
  const { header, hunks } = parseDiff(text);
  const title = el('div', { class: 'git-diff-title' },
    el('span', { class: `git-st st-${f.status === '?' ? 'q' : f.status}` }, f.status),
    el('span', { class: 'sel' }, f.orig ? `${f.orig} → ${f.path}` : f.path));
  if (f.kind !== 'commit') {
    title.append(el('span', { class: 'spacer' }),
      f.kind === 'staged' ? smallBtn('Unstage file', () => unstage([f])) : smallBtn(f.status === 'U' ? 'Mark resolved' : 'Stage file', () => stage([f])));
    if (f.kind === 'unstaged' && f.status !== 'U') title.append(smallBtn('Discard file', () => discard([f])));
  }
  const out = [title];
  if (!hunks.length) {
    const binary = header.some((l) => l.startsWith('Binary files') || l.startsWith('GIT binary patch'));
    const mode = header.find((l) => l.startsWith('old mode') || l.startsWith('new mode'));
    out.push(el('div', { class: 'git-placeholder' }, binary ? 'Binary file: no text diff.' : mode ? `File mode changed (${header.filter((l) => / mode /.test(l)).join(', ')}).` : text.trim() ? 'No content changes.' : 'No changes.'));
    box.replaceChildren(...out);
    return;
  }
  const combined = hunks[0].head.startsWith('@@@'); // conflicted file: one column per side
  const hunkActions = f.kind !== 'commit' && f.status !== '?' && !combined;
  let budget = git.fullDiff ? Infinity : DIFF_LINES_SHOWN;
  for (const hunk of hunks) {
    if (budget <= 0) break;
    // "@@ -a,b +c,d @@", or "@@@ -a,b -a2,b2 +c,d @@@" for a conflicted file
    let oldN = +(/^@@+ -(\d+)/.exec(hunk.head)?.[1] || 0);
    let newN = +(/ \+(\d+)(?:,\d+)? @@/.exec(hunk.head)?.[1] || 0);
    const head = el('div', { class: 'git-hunk-head' }, el('span', { class: 'mono' }, hunk.head));
    if (hunkActions) {
      head.append(el('span', { class: 'spacer' }));
      if (f.kind === 'staged') head.append(smallBtn('Unstage hunk', () => applyHunk(header, hunk, 'unstage')));
      else head.append(smallBtn('Stage hunk', () => applyHunk(header, hunk, 'stage')), smallBtn('Discard hunk', () => applyHunk(header, hunk, 'discard')));
    }
    const body = el('div', { class: 'git-hunk' });
    for (const l of hunk.lines) {
      if (budget-- <= 0) break;
      const c = combined ? (l.slice(0, 2).includes('+') ? '+' : l.slice(0, 2).includes('-') ? '-' : ' ') : l[0];
      const txt = (combined ? l.slice(2) : l.slice(1)).replace(/\r$/, '');
      let a = '';
      let b = '';
      let cls = 'ctx';
      if (l[0] === '\\') { cls = 'meta'; } else if (c === '+') { cls = 'add'; b = newN++; } else if (c === '-') { cls = 'del'; a = oldN++; } else { a = oldN++; b = newN++; }
      body.append(el('div', { class: `git-line ${cls}` },
        el('span', { class: 'ln' }, String(a)), el('span', { class: 'ln' }, String(b)),
        el('span', { class: 'sign' }, cls === 'meta' ? '' : c), el('span', { class: 'code' }, cls === 'meta' ? l : txt)));
    }
    out.push(el('div', { class: 'git-hunk-box' }, head, body));
  }
  if (budget < 0 || (budget === 0 && !git.fullDiff)) {
    out.push(el('div', { class: 'git-more', onclick: () => { git.fullDiff = true; renderDiff(text, f); } }, 'Large diff: the rest is hidden. Show the full diff'));
  }
  box.replaceChildren(...out);
  box.scrollTop = scroll;
}

// ---------- working copy actions ----------

async function stage(files, all = false) {
  if (!files.length) return;
  const steps = all ? [['add', '-A']] : chunked(['add', '-A'], files.map((f) => f.path));
  await gitOp('Stage', steps, { quiet: true });
}

async function unstage(files, all = false) {
  if (!files.length) return;
  const paths = files.flatMap((f) => (f.orig ? [f.orig, f.path] : [f.path]));
  let steps;
  if (!git.head) steps = all ? [['rm', '-r', '-q', '--cached', '.']] : chunked(['rm', '-r', '-q', '--cached'], paths);
  else steps = all ? [['reset', '-q']] : chunked(['reset', '-q', 'HEAD'], paths);
  await gitOp('Unstage', steps, { quiet: true });
}

async function discard(files) {
  if (!files.length) return;
  const untracked = files.filter((f) => f.status === '?');
  const tracked = files.filter((f) => f.status !== '?');
  const what = files.length === 1 ? files[0].path : `${files.length} files`;
  const ok = await gitConfirm('Discard changes', `Discard the unstaged changes to ${what}?${untracked.length ? ` ${untracked.length === files.length ? 'It is untracked and will be deleted.' : `${untracked.length} untracked file(s) will be deleted.`}` : ''} This can't be undone.`, 'Discard');
  if (!ok) return;
  await gitOp('Discard', [
    ...chunked(['checkout'], tracked.map((f) => f.path)),
    ...chunked(['clean', '-f', '-q'], untracked.map((f) => f.path)),
  ], { quiet: true });
}

async function applyHunk(header, hunk, mode) {
  const patch = `${[...header, hunk.head, ...hunk.lines].join('\n')}\n`;
  const args = {
    stage: ['apply', '--cached', '--whitespace=nowarn', '-'],
    unstage: ['apply', '--cached', '-R', '--whitespace=nowarn', '-'],
    discard: ['apply', '-R', '--whitespace=nowarn', '-'],
  }[mode];
  if (mode === 'discard' && !(await gitConfirm('Discard hunk', 'Discard this hunk from the working copy? This can\'t be undone.', 'Discard'))) return;
  await gitOp({ stage: 'Stage hunk', unstage: 'Unstage hunk', discard: 'Discard hunk' }[mode], [{ args, input: patch }], { quiet: true });
}

function fileMenu(e, f, kind) {
  const list = kind === 'staged' ? git.staged : kind === 'unstaged' ? git.unstaged : git.detail?.files || [];
  const picked = kind === 'commit' ? [f] : pickedOf(kind, list);
  const abs = joinPath(git.root, f.path);
  const items = [];
  if (kind === 'unstaged') {
    const conflicted = picked.some((x) => x.status === 'U');
    items.push({ label: picked.length > 1 ? `Stage ${picked.length} files` : conflicted ? 'Mark resolved' : 'Stage', action: () => stage(picked) });
    if (!conflicted) items.push({ label: picked.length > 1 ? `Discard ${picked.length} files` : 'Discard changes', danger: true, action: () => discard(picked) });
  }
  if (kind === 'staged') items.push({ label: picked.length > 1 ? `Unstage ${picked.length} files` : 'Unstage', action: () => unstage(picked) });
  if (kind === 'unstaged' && f.status === '?') {
    items.push({ label: 'Ignore this file', action: () => ignorePath(f.path) });
    const dir = splitPath(f.path)[0];
    if (dir) items.push({ label: `Ignore everything in ${dir}/`, action: () => ignorePath(`${dir}/`) });
  }
  if (kind === 'staged' || kind === 'unstaged') {
    items.push({ label: 'Open file', separator: true, action: () => api.openFile(abs) });
  } else items.push({ label: 'Open current version', action: () => api.openFile(abs) });
  items.push({ label: 'Show in folder', action: () => api.showFile(abs) });
  items.push({ label: 'Copy path', action: () => navigator.clipboard.writeText(f.path) });
  if (kind === 'commit' && git.detail?.hash) {
    items.push({ label: 'Restore this version to the working copy', separator: true, action: async () => {
      if (await gitConfirm('Restore file', `Replace ${f.path} in the working copy with its version from ${short(git.detail.hash)}?`, 'Restore')) gitOp('Restore file', [['checkout', git.detail.hash, '--', f.path]]);
    } });
  }
  showMenu(e, items);
}

async function ignorePath(pattern) {
  if (!(await api.gitIgnore(git.root, `/${pattern}`))) { gitBar('Couldn\'t update .gitignore.', 'error'); return; }
  gitBar(`Added /${pattern} to .gitignore.`, 'ok');
  gitReload(true);
}

// ---------- commit ----------

$('#git-commit-go').addEventListener('click', () => doCommit());
$('#git-commit-more').addEventListener('click', () => $('#git-commit-opts').classList.toggle('hidden'));
$('#git-commit-cancel').addEventListener('click', () => { $('#git-msg').value = ''; $('#git-amend').checked = false; setView('history'); });
$('#git-msg').addEventListener('keydown', (e) => { if (e.key === 'Enter' && e.ctrlKey) { e.preventDefault(); doCommit(); } });
$('#git-amend').addEventListener('change', async (e) => {
  if (!e.target.checked || $('#git-msg').value.trim() || !git.head) return;
  const r = await run(['log', '-1', '--format=%B']);
  if (r.code === 0) $('#git-msg').value = r.stdout.trim();
});

async function doCommit() {
  const msg = $('#git-msg').value.trim();
  const amend = $('#git-amend').checked;
  if (!msg) { $('#git-msg').focus(); gitBar('Write a commit message first.', 'error'); return; }
  if (!git.staged.length && !amend && git.operation !== 'merge') {
    if (!git.unstaged.length) { gitBar('There is nothing to commit.', 'error'); return; }
    const ok = await gitConfirm('Nothing staged', 'No files are staged. Stage all changes and commit them?', 'Stage all and commit', false);
    if (!ok) return;
    if (!(await gitOp('Stage', [['add', '-A']], { quiet: true }))) return;
  }
  const args = ['commit', '-F', '-'];
  if (amend) args.push('--amend');
  const ok = await gitOp(amend ? 'Amend commit' : 'Commit', [{ args, input: `${msg}\n` }]);
  if (!ok) return;
  $('#git-msg').value = '';
  $('#git-amend').checked = false;
  if ($('#git-push-after').checked) await push();
}

// ---------- toolbar ----------

async function push() {
  if (!git.branch) { gitBar('HEAD is detached: check out a branch to push.', 'error'); return; }
  const current = git.locals.find((b) => b.name === git.branch);
  if (current?.upstream && !current.gone) { await gitOp('Push', [['push']]); return; }
  if (!git.remotes.length) { gitBar('This repository has no remote to push to. Add one with git remote add.', 'error'); return; }
  const v = await gitDialog({
    title: 'Push',
    text: `${git.branch} has no upstream branch yet. Push it and track the remote branch?`,
    fields: [{ id: 'remote', label: 'Remote', type: 'select', options: git.remotes, value: git.remotes.includes('origin') ? 'origin' : git.remotes[0] }],
    ok: 'Push',
  });
  if (v) await gitOp('Push', [['push', '-u', v.remote, git.branch]]);
}

const selectedCommit = () => (isHash(git.selected) ? git.selected : null);

$('#gt-commit').addEventListener('click', () => { setView('status'); $('#git-msg').focus(); });
$('#gt-pull').addEventListener('click', () => {
  const current = git.locals.find((b) => b.name === git.branch);
  if (git.branch && !current?.upstream) { gitBar(`${git.branch} has no upstream branch to pull from. Push it first, or check out a remote branch.`, 'error'); return; }
  gitOp('Pull', [['pull', '--no-edit']]);
});
$('#gt-push').addEventListener('click', () => push());
$('#gt-fetch').addEventListener('click', () => {
  if (!git.remotes.length) { gitBar('This repository has no remotes to fetch from.', 'error'); return; }
  gitOp('Fetch', [['fetch', '--all', '--prune']]);
});
$('#gt-branch').addEventListener('click', () => newBranch(selectedCommit()));
$('#gt-merge').addEventListener('click', async () => {
  const options = [...git.locals.map((b) => b.name).filter((n) => n !== git.branch), ...git.remoteBranches.map((b) => b.name)];
  if (!options.length) { gitBar('There is no other branch to merge.', 'error'); return; }
  const current = git.locals.find((b) => b.name === git.branch);
  const v = await gitDialog({
    title: 'Merge',
    text: `Merge a branch into ${git.branch || 'the detached HEAD'}.`,
    fields: [
      { id: 'branch', label: 'Branch to merge', type: 'select', options, value: current?.upstream && options.includes(current.upstream) ? current.upstream : options[0] },
      { id: 'noff', label: 'Always create a merge commit (--no-ff)', type: 'check' },
    ],
    ok: 'Merge',
  });
  if (v) mergeBranch(v.branch, v.noff);
});
$('#gt-stash').addEventListener('click', async () => {
  if (!git.staged.length && !git.unstaged.length) { gitBar('There are no changes to stash.', 'error'); return; }
  const v = await gitDialog({
    title: 'Stash changes',
    text: 'Set the uncommitted changes aside and restore a clean working copy. Apply the stash later from the sidebar.',
    fields: [
      { id: 'msg', label: 'Message', placeholder: 'Optional' },
      { id: 'untracked', label: 'Include untracked files', type: 'check', value: true },
      { id: 'keep', label: 'Keep staged changes', type: 'check' },
    ],
    ok: 'Stash',
  });
  if (!v) return;
  const args = ['stash', 'push'];
  if (v.untracked) args.push('-u');
  if (v.keep) args.push('--keep-index');
  if (v.msg) args.push('-m', v.msg);
  gitOp('Stash', [args]);
});
$('#gt-discard').addEventListener('click', async () => {
  if (!git.staged.length && !git.unstaged.length) { gitBar('There are no changes to discard.', 'error'); return; }
  const untracked = git.unstaged.filter((f) => f.status === '?').length;
  const v = await gitDialog({
    title: 'Discard all changes',
    text: 'Reset every tracked file to the last commit, staged and unstaged changes alike. This can\'t be undone.',
    fields: untracked ? [{ id: 'clean', label: `Also delete ${untracked} untracked file${untracked === 1 ? '' : 's'}`, type: 'check' }] : [],
    ok: 'Discard all',
    danger: true,
  });
  if (!v) return;
  const steps = git.head ? [['reset', '--hard', '-q']] : [['rm', '-r', '-q', '--cached', '.']];
  if (v.clean) steps.push(['clean', '-fd', '-q']);
  gitOp('Discard all', steps);
});
$('#gt-tag').addEventListener('click', () => newTag(selectedCommit() || git.head));
$('#gt-folder').addEventListener('click', () => git.root && api.revealProject(git.root));
$('#gt-refresh').addEventListener('click', () => gitReload(true));
document.addEventListener('keydown', (e) => {
  if (git.open && e.key === 'F5') { e.preventDefault(); gitReload(true); }
});

// ---------- branches, tags, stashes, commits ----------

async function checkout(target, { detached = false } = {}) {
  if (target === git.branch) return;
  if (detached && !(await gitConfirm('Check out commit', `Check out ${short(target)}? HEAD will be detached: new commits won't belong to any branch until you create one.`, 'Check out', false))) return;
  gitOp(`Check out ${detached ? short(target) : target}`, [['checkout', ...(detached ? ['--detach'] : []), target]]);
}

async function checkoutRemote(b) {
  const local = b.name.slice(b.remote.length + 1);
  if (git.locals.some((x) => x.name === local)) {
    const existing = git.locals.find((x) => x.name === local);
    if (existing.upstream === b.name) { checkout(local); return; }
  }
  const v = await gitDialog({
    title: 'Check out remote branch',
    text: `Create a local branch that tracks ${b.name} and check it out.`,
    fields: [{ id: 'name', label: 'Local branch name', value: local, required: true }],
    ok: 'Check out',
  });
  if (v) gitOp(`Check out ${v.name}`, [['checkout', '-b', v.name, '--track', b.name]]);
}

async function newBranch(at) {
  const v = await gitDialog({
    title: 'New branch',
    text: at && at !== git.head ? `Start the branch at commit ${short(at)}.` : `Start the branch at the current ${git.branch ? `branch (${git.branch})` : 'commit'}.`,
    fields: [
      { id: 'name', label: 'Branch name', required: true },
      { id: 'checkout', label: 'Check out the new branch', type: 'check', value: true },
    ],
    ok: 'Create branch',
  });
  if (!v) return;
  const start = at && at !== git.head ? [at] : [];
  gitOp(`Create branch ${v.name}`, [v.checkout ? ['checkout', '-b', v.name, ...start] : ['branch', v.name, ...start]]);
}

async function newTag(at) {
  if (!at) { gitBar('Make a commit before adding a tag.', 'error'); return; }
  const v = await gitDialog({
    title: 'New tag',
    text: `Tag commit ${short(at)}.`,
    fields: [
      { id: 'name', label: 'Tag name', required: true, placeholder: 'e.g. v1.0.0' },
      { id: 'msg', label: 'Message', type: 'textarea', placeholder: 'Optional. With a message the tag is annotated.' },
      ...(git.remotes.length ? [{ id: 'push', label: `Push the tag to ${git.remotes.includes('origin') ? 'origin' : git.remotes[0]}`, type: 'check' }] : []),
    ],
    ok: 'Create tag',
  });
  if (!v) return;
  const steps = [v.msg ? ['tag', '-a', v.name, '-m', v.msg, at] : ['tag', v.name, at]];
  if (v.push) steps.push(['push', git.remotes.includes('origin') ? 'origin' : git.remotes[0], `refs/tags/${v.name}`]);
  gitOp(`Create tag ${v.name}`, steps);
}

async function mergeBranch(name, noff = false) {
  gitOp(`Merge ${name}`, [['merge', '--no-edit', ...(noff ? ['--no-ff'] : []), name]]);
}

async function rebaseOnto(name) {
  if (!git.branch) { gitBar('Check out a branch before rebasing.', 'error'); return; }
  if (await gitConfirm('Rebase', `Rebase ${git.branch} onto ${name}? Its commits are replayed on top of ${name}, rewriting them.`, 'Rebase', false)) gitOp(`Rebase onto ${name}`, [['rebase', name]]);
}

async function abortOperation() {
  const op = git.operation;
  if (op && (await gitConfirm(`Abort ${op}`, `Stop the ${op} and go back to how things were before it started?`, 'Abort'))) gitOp(`Abort ${op}`, [[op, '--abort']]);
}

async function continueOperation() {
  const op = git.operation;
  if (op) gitOp(`Continue ${op}`, [[op, '--continue']]);
}

function branchMenu(e, b) {
  const current = b.name === git.branch;
  const items = [];
  if (!current) {
    items.push({ label: `Check out ${b.name}`, action: () => checkout(b.name) });
    items.push({ label: `Merge ${b.name} into ${git.branch || 'HEAD'}`, action: () => mergeBranch(b.name) });
    items.push({ label: `Rebase ${git.branch || 'HEAD'} onto ${b.name}`, action: () => rebaseOnto(b.name) });
  }
  if (git.remotes.length) items.push({ label: b.upstream ? `Push to ${b.upstream}` : 'Push to remote…', separator: !current, action: () => (current ? push() : pushBranch(b)) });
  items.push({ label: 'New branch from here…', action: () => newBranch(b.hash) });
  items.push({ label: 'Rename…', action: () => renameBranch(b) });
  items.push({ label: 'Copy name', action: () => navigator.clipboard.writeText(b.name) });
  if (!current) items.push({ label: 'Delete…', danger: true, action: () => deleteBranch(b) });
  showMenu(e, items);
}

async function pushBranch(b) {
  if (b.upstream && !b.gone) { gitOp(`Push ${b.name}`, [['push', b.upstream.split('/')[0], `${b.name}:${b.upstream.slice(b.upstream.indexOf('/') + 1)}`]]); return; }
  const v = await gitDialog({
    title: `Push ${b.name}`,
    fields: [{ id: 'remote', label: 'Remote', type: 'select', options: git.remotes, value: git.remotes.includes('origin') ? 'origin' : git.remotes[0] }],
    ok: 'Push',
  });
  if (v) gitOp(`Push ${b.name}`, [['push', '-u', v.remote, b.name]]);
}

async function renameBranch(b) {
  const v = await gitDialog({ title: 'Rename branch', fields: [{ id: 'name', label: 'New name', value: b.name, required: true }], ok: 'Rename' });
  if (v && v.name !== b.name) gitOp('Rename branch', [['branch', '-m', b.name, v.name]]);
}

async function deleteBranch(b) {
  const v = await gitDialog({
    title: 'Delete branch',
    text: `Delete the local branch ${b.name}?`,
    fields: [{ id: 'force', label: 'Delete even if it has commits that aren\'t merged anywhere', type: 'check' }],
    ok: 'Delete',
    danger: true,
  });
  if (v) gitOp(`Delete ${b.name}`, [['branch', v.force ? '-D' : '-d', b.name]]);
}

function remoteBranchMenu(e, b) {
  showMenu(e, [
    { label: 'Check out as local branch…', action: () => checkoutRemote(b) },
    { label: `Merge ${b.name} into ${git.branch || 'HEAD'}`, action: () => mergeBranch(b.name) },
    { label: `Rebase ${git.branch || 'HEAD'} onto ${b.name}`, action: () => rebaseOnto(b.name) },
    { label: 'Copy name', action: () => navigator.clipboard.writeText(b.name) },
    { label: `Delete ${b.name} on the remote…`, danger: true, action: async () => {
      if (await gitConfirm('Delete remote branch', `Delete ${b.name.slice(b.remote.length + 1)} from ${b.remote}? This affects everyone using the remote.`, 'Delete')) {
        gitOp(`Delete ${b.name}`, [['push', b.remote, '--delete', b.name.slice(b.remote.length + 1)]]);
      }
    } },
  ]);
}

function tagMenu(e, t) {
  const items = [
    { label: `Check out ${t.name}`, action: () => checkout(t.name, { detached: true }) },
    { label: 'New branch from here…', action: () => newBranch(t.hash) },
  ];
  for (const r of git.remotes) items.push({ label: `Push to ${r}`, action: () => gitOp(`Push ${t.name}`, [['push', r, `refs/tags/${t.name}`]]) });
  items.push({ label: 'Copy name', action: () => navigator.clipboard.writeText(t.name) });
  items.push({ label: 'Delete…', danger: true, action: async () => {
    if (await gitConfirm('Delete tag', `Delete the local tag ${t.name}?`, 'Delete')) gitOp(`Delete tag ${t.name}`, [['tag', '-d', t.name]]);
  } });
  showMenu(e, items);
}

function stashMenu(e, s) {
  showMenu(e, [
    { label: 'Apply', action: () => gitOp(`Apply ${s.ref}`, [['stash', 'apply', '--index', s.ref]]) },
    { label: 'Pop (apply and delete)', action: () => gitOp(`Pop ${s.ref}`, [['stash', 'pop', '--index', s.ref]]) },
    { label: 'Delete…', danger: true, action: async () => {
      if (await gitConfirm('Delete stash', `Delete ${s.ref} (${s.message})? This can't be undone.`, 'Delete')) gitOp(`Delete ${s.ref}`, [['stash', 'drop', s.ref]]);
    } },
  ]);
}

function commitMenu(e, c) {
  const on = git.branch || 'HEAD';
  const items = [
    { label: 'Check out this commit…', action: () => checkout(c.hash, { detached: true }) },
    { label: 'New branch here…', action: () => newBranch(c.hash) },
    { label: 'New tag here…', action: () => newTag(c.hash) },
  ];
  if (c.hash !== git.head) {
    items.push({ label: `Merge into ${on}`, separator: true, action: () => mergeBranch(c.hash) });
    items.push({ label: `Rebase ${on} onto this commit`, action: () => rebaseOnto(c.hash) });
    items.push({ label: `Cherry-pick onto ${on}`, action: async () => {
      if (await gitConfirm('Cherry-pick', `Apply the changes of ${short(c.hash)} "${c.subject}" as a new commit on ${on}?`, 'Cherry-pick', false)) gitOp('Cherry-pick', [['cherry-pick', ...(c.parents.length > 1 ? ['-m', '1'] : []), c.hash]]);
    } });
  }
  items.push({ label: 'Revert this commit…', separator: c.hash === git.head, action: async () => {
    if (await gitConfirm('Revert', `Create a new commit that undoes ${short(c.hash)} "${c.subject}"?`, 'Revert', false)) gitOp('Revert', [['revert', '--no-edit', ...(c.parents.length > 1 ? ['-m', '1'] : []), c.hash]]);
  } });
  if (git.branch && c.hash !== git.head) {
    items.push({ label: `Reset ${on} to this commit…`, action: () => resetTo(c) });
  }
  items.push({ label: 'Copy commit hash', separator: true, action: () => navigator.clipboard.writeText(c.hash) });
  items.push({ label: 'Copy message', action: () => navigator.clipboard.writeText(c.subject) });
  showMenu(e, items);
}

async function resetTo(c) {
  const v = await gitDialog({
    title: `Reset ${git.branch}`,
    text: `Move ${git.branch} to ${short(c.hash)} "${c.subject}". Commits after it leave the branch.`,
    fields: [{ id: 'mode', label: 'Mode', type: 'select', options: ['Soft: keep all changes staged', 'Mixed: keep changes in the working copy', 'Hard: discard all changes'], value: 'Mixed: keep changes in the working copy' }],
    ok: 'Reset',
    danger: true,
  });
  if (!v) return;
  const mode = v.mode.split(':')[0].toLowerCase();
  if (mode === 'hard' && !(await gitConfirm('Hard reset', 'A hard reset deletes every uncommitted change. Continue?', 'Reset'))) return;
  gitOp(`Reset (${mode})`, [['reset', `--${mode}`, '-q', c.hash]]);
}

// ---------- resizable splits ----------

function dragSplit(handle, target, axis, key, min, maxOf) {
  const saved = parseInt(localStorage.getItem(key) || '', 10);
  if (saved) target.style[axis === 'y' ? 'height' : 'width'] = `${saved}px`;
  handle.addEventListener('mousedown', (e) => {
    e.preventDefault();
    const start = axis === 'y' ? e.clientY : e.clientX;
    const size = axis === 'y' ? target.offsetHeight : target.offsetWidth;
    document.body.classList.add('resizing', axis === 'y' ? 'resizing-y' : 'resizing-x');
    const move = (ev) => {
      const d = (axis === 'y' ? ev.clientY : ev.clientX) - start;
      target.style[axis === 'y' ? 'height' : 'width'] = `${Math.max(min, Math.min(maxOf(), size + d))}px`;
    };
    const up = () => {
      document.body.classList.remove('resizing', 'resizing-y', 'resizing-x');
      localStorage.setItem(key, parseInt(target.style[axis === 'y' ? 'height' : 'width'], 10));
      removeEventListener('mousemove', move);
      removeEventListener('mouseup', up);
    };
    addEventListener('mousemove', move);
    addEventListener('mouseup', up);
  });
}
dragSplit($('#git-split'), $('#git-log'), 'y', 'gitLogHeight', 80, () => $('#git-content').offsetHeight - 120);

// The panel's left edge: dragging it trades width between the terminal and Git.
(() => {
  const resizer = $('#git-resizer');
  const saved = parseInt(localStorage.getItem('gitWidth') || '', 10);
  if (saved) gitEl.style.width = `${saved}px`;
  resizer.addEventListener('mousedown', (e) => {
    e.preventDefault();
    resizer.classList.add('dragging');
    document.body.classList.add('resizing', 'resizing-x');
    const right = gitEl.getBoundingClientRect().right;
    const move = (ev) => {
      const max = right - $('#sidebar').getBoundingClientRect().right - 360; // keep the terminal usable
      gitEl.style.width = `${Math.max(460, Math.min(max, right - ev.clientX))}px`;
    };
    const up = () => {
      resizer.classList.remove('dragging');
      document.body.classList.remove('resizing', 'resizing-x');
      localStorage.setItem('gitWidth', parseInt(gitEl.style.width, 10));
      removeEventListener('mousemove', move);
      removeEventListener('mouseup', up);
      fitTerminals();
    };
    addEventListener('mousemove', move);
    addEventListener('mouseup', up);
  });
})();
dragSplit($('#git-files-split'), $('#git-files-col'), 'x', 'gitFilesWidth', 220, () => $('#git-detail').offsetWidth - 200);
