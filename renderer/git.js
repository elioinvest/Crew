/* global api, $, el, state, norm, showMenu, safeFit, files, closeFiles, openWorktreePicker, openBranchPicker, openListPicker, refreshGitChips */
// The Git panel, as Zed draws it: Changes and History tabs, one column of files in Staged and
// Unstaged sections with checkboxes, and at the bottom the branch picker, one Fetch / Pull / Push
// button, the commit editor and the last commit, which can be undone. View Diff (or clicking a
// file) opens the Project Diff beside the list: hunks stage, unstage and restore, conflicts resolve
// with a click. Also a stash picker, file history and the commit graph. It follows
// the active session's folder, so a session in a linked worktree shows that worktree. Every action
// runs the real git CLI (main.js). The worktree and branch pickers are in worktrees.js.

const gitEl = $('#git');
const LOG_PAGE = 1000;
const ROW_H = 24;
const LANE_W = 14;
const DIFF_LINES_SHOWN = 4000;
const MAX_UNTRACKED_DIFFS = 25;
const LANE_COLORS = ['#4a8fd8', '#d97757', '#3fa45b', '#a46fd6', '#d6a23a', '#3aa8a8', '#d65a8a', '#7f8f3a'];

const git = {
  open: false,
  root: null,
  gitDir: null,
  view: localStorage.getItem('gitView') === 'history' ? 'history' : 'changes',
  groupBy: localStorage.getItem('gitGroupBy') === 'tracked' ? 'tracked' : 'staged',
  diffOpen: localStorage.getItem('gitDiffOpen') === '1', // the panel is just the list until a diff is asked for
  tree: localStorage.getItem('gitTree') === '1',
  diffStyle: localStorage.getItem('gitDiffStyle') === 'split' ? 'split' : 'unified',
  head: null, // HEAD commit hash, or null before the first commit
  last: null, // the latest commit: { subject, at, hasParent }
  numstat: { staged: new Map(), unstaged: new Map() }, // path -> [lines added, removed]
  foldedSections: {}, // section title -> folded
  foldedDirs: new Set(), // folders folded in the tree view
  foldedFiles: new Set(), // files folded in the diff
  branch: '', // current branch; empty when HEAD is detached
  locals: [], // { name, hash, upstream, ahead, behind, gone }
  remoteBranches: [], // { name, remote, hash }
  remotes: [],
  tags: [], // { name, hash }
  stashes: [], // { ref, message, at }
  operation: null, // 'merge' | 'rebase' | 'cherry-pick' | 'revert' in progress
  commits: [],
  logStale: true, // the history is loaded when it's shown
  limit: LOG_PAGE,
  staged: [],
  unstaged: [],
  diffs: new Map(), // path -> { staged: parsed diff, unstaged: parsed diff }
  commitOpts: { amend: false, signoff: false, noVerify: false, push: false },
  selected: null, // in History: a commit hash or a stash ref
  fileFilter: null, // in History: only the commits touching this file
  picked: new Set(), // selected file rows, by path
  anchor: null, // last plainly clicked row, for Shift-click ranges
  fullDiff: false,
  detail: null, // { rev, parent, meta, files, diffs } for the selected commit or stash
  detailCache: new Map(),
  refSig: null,
  statusSig: null,
  busy: false,
  reloading: false,
  diffSeq: 0,
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
  const doubled = { Commit: 'Committing', Pop: 'Popping', Reset: 'Resetting', Stop: 'Stopping', Uncommit: 'Uncommitting' }[word];
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
  refreshGitChips(); // worktrees.js: the branch may have changed
  return ok;
}

// ---------- loading ----------

// git status in its detailed form, which also says the branch, commit and upstream, so one process
// answers most of what a refresh needs.
function parseStatus(out) {
  const parts = out.split('\0');
  const staged = [];
  const unstaged = [];
  const branch = {};
  const after = (s, n) => { let i = 0; for (let k = 0; k < n; k++) i = s.indexOf(' ', i) + 1; return s.slice(i); };
  for (let i = 0; i < parts.length; i++) {
    const s = parts[i];
    if (s.startsWith('# branch.')) {
      const sp = s.indexOf(' ', 2);
      branch[s.slice(9, sp)] = s.slice(sp + 1);
    } else if (s[0] === '?') unstaged.push({ path: s.slice(2), status: '?' });
    else if (s[0] === 'u') unstaged.push({ path: after(s, 10), status: 'U' });
    else if (s[0] === '1' || s[0] === '2') {
      const x = s[2];
      const y = s[3];
      const p = after(s, s[0] === '1' ? 8 : 9);
      const orig = s[0] === '2' ? parts[++i] : undefined;
      if (x !== '.') staged.push({ path: p, orig, status: x });
      if (y !== '.') unstaged.push({ path: p, status: y });
    }
  }
  const byPath = (a, b) => a.path.localeCompare(b.path);
  git.staged = staged.sort(byPath);
  git.unstaged = unstaged.sort(byPath);
  git.head = branch.oid && branch.oid !== '(initial)' ? branch.oid : null;
  git.branch = branch.head && branch.head !== '(detached)' ? branch.head : '';
  return `${branch.oid}|${branch.head}|${branch.upstream || ''}|${branch.ab || ''}`;
}

// One row per changed file, with what's staged and what isn't.
function entries() {
  const map = new Map();
  for (const f of git.unstaged) map.set(f.path, { path: f.path, status: f.status, unstaged: f, staged: null, orig: f.orig });
  for (const f of git.staged) {
    const e = map.get(f.path);
    if (e) { e.staged = f; e.orig ||= f.orig; } else map.set(f.path, { path: f.path, status: f.status, staged: f, unstaged: null, orig: f.orig });
  }
  return [...map.values()].sort((a, b) => a.path.localeCompare(b.path));
}

function parseTrack(track) {
  return {
    ahead: +(/ahead (\d+)/.exec(track)?.[1] || 0),
    behind: +(/behind (\d+)/.exec(track)?.[1] || 0),
    gone: track.includes('gone'),
  };
}

async function loadRefs() {
  const [refs, stash, remotes, last] = await Promise.all([
    run(['for-each-ref', '--format=%(refname)%00%(refname:short)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track)', 'refs/heads', 'refs/remotes', 'refs/tags']),
    run(['stash', 'list', '--format=%gd%x00%s%x00%at']),
    run(['remote']),
    run(['log', '-1', '--format=%s%x00%at%x00%P']),
  ]);
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
    const [ref, message, at] = l.split('\0');
    return { ref, message, at: +at };
  });
  git.remotes = remotes.stdout.split('\n').map((s) => s.trim()).filter(Boolean);
  if (last.code === 0 && last.stdout.trim()) {
    const [subject, at, parents] = last.stdout.trim().split('\0');
    git.last = { subject, at: +at, hasParent: !!parents };
  } else git.last = null;

  const ops = [['MERGE_HEAD', 'merge'], ['rebase-merge', 'rebase'], ['rebase-apply', 'rebase'], ['CHERRY_PICK_HEAD', 'cherry-pick'], ['REVERT_HEAD', 'revert']];
  const found = git.gitDir ? await Promise.all(ops.map(([file]) => api.fileExists(joinPath(git.gitDir, file)))) : [];
  git.operation = ops[found.indexOf(true)]?.[1] || null;
}

async function loadLog() {
  git.logStale = false;
  const format = '--format=%x1e%H%x00%P%x00%D%x00%an%x00%ae%x00%at%x00%s';
  let args;
  if (git.fileFilter) args = ['log', '--follow', `-n${git.limit}`, format, '--', git.fileFilter];
  else {
    args = ['log', '--date-order', `-n${git.limit}`, format, '--branches', '--remotes', '--tags'];
    if (git.head) args.push('HEAD');
  }
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

// A diff of several files, split per file and parsed: path -> { header, hunks }.
function diffsByFile(text) {
  const out = new Map();
  for (const block of text.split(/^(?=diff --git )/m)) {
    if (!block.startsWith('diff --git ')) continue;
    const parsed = parseDiff(block);
    const p = pathOfDiff(parsed.header);
    if (p) out.set(p, parsed);
  }
  return out;
}

function pathOfDiff(header) {
  const clean = (s) => s.replace(/^"|"$/g, '').replace(/\t.*$/, '');
  for (const l of header) if (l.startsWith('rename to ') || l.startsWith('copy to ')) return clean(l.replace(/^(rename|copy) to /, ''));
  for (const l of header) if (l.startsWith('+++ ') && !l.startsWith('+++ /dev/null')) return clean(l.slice(4)).replace(/^"?b\//, '');
  for (const l of header) if (l.startsWith('--- ') && !l.startsWith('--- /dev/null')) return clean(l.slice(4)).replace(/^"?a\//, '');
  const m = /^diff --git "?a\/(.*?)"? "?b\/(.*?)"?$/.exec(header[0] || '');
  return m ? m[2] : null;
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

// Everything for the Project Diff: staged and unstaged changes, and untracked files as new ones.
async function loadProjectDiff() {
  const [un, st] = await Promise.all([run(['diff']), run(['diff', '--cached', '-M'])]);
  const unstaged = diffsByFile(un.stdout || '');
  const staged = diffsByFile(st.stdout || '');
  const untracked = git.unstaged.filter((f) => f.status === '?');
  const shown = untracked.slice(0, MAX_UNTRACKED_DIFFS);
  const results = await Promise.all(shown.map((f) => run(['diff', '--no-index', '--', '/dev/null', f.path])));
  const diffs = new Map();
  const at = (p) => { if (!diffs.has(p)) diffs.set(p, {}); return diffs.get(p); };
  for (const [p, d] of unstaged) at(p).unstaged = d;
  for (const [p, d] of staged) at(p).staged = d;
  shown.forEach((f, i) => { if ([0, 1].includes(results[i].code)) at(f.path).unstaged = parseDiff(results[i].stdout); });
  git.diffs = diffs;
  git.hiddenUntracked = untracked.length - shown.length;
}

// Reloads what changed since last time. A refresh is one status and one diff summary; the full list
// of refs is checked less often (something else may have fetched or made a branch), and at once
// when forced or when the window comes back.
const REFS_EVERY = 5;
async function gitReload(force = false) {
  if (!git.open || !git.root) return;
  if (git.reloading) { if (force) git.forceAgain = true; return; } // run again once this one is done
  git.reloading = true;
  const root = git.root;
  try {
    git.polls = (git.polls || 0) + 1;
    const checkRefs = force || git.polls % REFS_EVERY === 0;
    const [st, changes, refs] = await Promise.all([
      run(['status', '--porcelain=v2', '--branch', '-z', '-uall']),
      run(['diff', '--numstat', '-z']), // an edit to an already changed file doesn't alter the status
      checkRefs ? run(['for-each-ref', '--format=%(objectname) %(refname)']) : null,
    ]);
    if (root !== git.root) return;
    const branchSig = parseStatus(st.stdout);
    if (refs) git.refList = refs.stdout;
    const refSig = branchSig + (git.refList || '');
    const statusSig = st.stdout + changes.stdout;
    const refsChanged = force || refSig !== git.refSig;
    const statusChanged = force || statusSig !== git.statusSig;
    git.refSig = refSig;
    git.statusSig = statusSig;
    if (!refsChanged && !statusChanged) return;
    git.logStale ||= refsChanged;
    await Promise.all([
      statusChanged ? loadNumstat() : null,
      statusChanged && git.view === 'changes' && git.diffOpen ? loadProjectDiff() : null,
      refsChanged ? loadRefs() : null,
    ]);
    if (git.logStale && git.view === 'history') await loadLog();
    if (root !== git.root) return;
    if (git.selected && !git.commits.some((c) => c.hash === git.selected) && !git.stashes.some((s) => s.ref === git.selected)) git.selected = null;
    renderGit();
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
    Object.assign(git, { selected: null, fileFilter: null, picked: new Set(), detail: null, refSig: null, statusSig: null, limit: LOG_PAGE, commits: [], staged: [], unstaged: [], diffs: new Map(), logStale: true });
    git.detailCache.clear();
    $('#git-log-rows').replaceChildren();
    $('#git-files').replaceChildren();
    showDiffPlaceholder();
    const gd = await api.git(root, ['rev-parse', '--absolute-git-dir']);
    git.gitDir = gd.code === 0 ? gd.stdout.trim() : null;
  }
  if (!git.open) {
    git.open = true;
    gitEl.classList.remove('hidden');
    fitPanel();
    $('#git-resizer').classList.remove('hidden');
    document.body.classList.add('git-open');
    $('#btn-git').classList.remove('hidden');
    fitTerminals();
  }
  renderHeader();
  if (switching) showLoading('Loading repository…', 200);
  try { await gitReload(true); } finally { hideLoading(); }
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

// A folder that's no longer a repository (a worktree just removed) is looked up again next time.
function forgetRepoRoot(folder) {
  for (const k of repoRoots.keys()) if (norm(k) === norm(folder) || norm(repoRoots.get(k)) === norm(folder)) repoRoots.delete(k);
}

// The terminals get narrower or wider as the panel opens, closes or is resized.
function fitTerminals() {
  requestAnimationFrame(() => { for (const t of state.terms.values()) safeFit(t); });
}

// Called by activate(): show the Git button when the active session's folder is a repository,
// and follow the active session to its repository or worktree while the Git view is open.
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

// Refreshes only while Crew is in front; coming back catches up.
setInterval(() => { if (git.open && document.hasFocus() && !git.busy) gitReload(); }, 3000);
window.addEventListener('focus', () => { if (git.open && !git.busy) { git.polls = REFS_EVERY - 1; gitReload(); } });

// ---------- rendering ----------

function renderGit() {
  renderHeader();
  renderToolbar();
  const history = git.view === 'history';
  gitEl.dataset.view = git.view;
  fitPanel();
  $('#git-log').classList.toggle('hidden', !history);
  $('#git-split').classList.toggle('hidden', !history);
  $('#git-commit-box').classList.toggle('hidden', history);
  renderCommitBox();
  if (history) {
    renderLog();
    renderHistoryDetail();
  } else {
    renderGitFiles();
    if (git.diffOpen) renderProjectDiff();
    else if (diffBox.firstChild) { unmountCards(); diffBox.replaceChildren(); }
  }
}

// The list takes a narrow panel beside the terminal. The diff and the history need more room, and
// lie over the terminal instead of squeezing it: resizing one makes its whole history reflow and
// its agent redraw the screen, which would lag on every switch. Each width is remembered on its own.
function setGitWidth(width) {
  const narrow = gitEl.classList.contains('narrow');
  const list = parseInt(localStorage.getItem('gitListWidth') || '', 10) || 380;
  const room = innerWidth - $('#sidebar').getBoundingClientRect().right;
  const w = narrow ? width : Math.max(list, Math.min(width, room - 60));
  const over = narrow ? 0 : w - list;
  gitEl.style.width = `${w}px`;
  gitEl.style.marginLeft = over ? `${-over}px` : '';
  gitEl.classList.toggle('over', over > 0);
  $('#git-resizer').style.transform = over ? `translateX(${-over}px)` : '';
}

function fitPanel() {
  const wide = git.view === 'history' || git.diffOpen;
  gitEl.classList.toggle('narrow', !wide);
  const saved = parseInt(localStorage.getItem(wide ? 'gitWidth' : 'gitListWidth') || '', 10);
  setGitWidth(saved || (wide ? Math.max(660, Math.round(innerWidth * 0.5)) : 380));
}

async function toggleDiff(open = !git.diffOpen) {
  if (open === git.diffOpen) return;
  git.diffOpen = open;
  localStorage.setItem('gitDiffOpen', open ? '1' : '0');
  if (open) await loadProjectDiff();
  renderGit();
}

async function setView(view) {
  git.view = view;
  localStorage.setItem('gitView', view);
  if (view === 'history' && git.logStale) { showLoading('Loading history…', 200); await loadLog(); hideLoading(); }
  if (view === 'changes' && git.diffOpen) await loadProjectDiff();
  renderGit();
}

function renderHeader() {
  if (!git.open) return;
  const repo = git.root.split(/[\\/]/).filter(Boolean).pop();
  const detached = git.head ? `detached at ${short(git.head)}` : 'no commits yet';
  $('#git-wt-label').textContent = repo;
  $('#git-wt-btn').title = `Worktree: ${git.root}`;
  $('#git-br-label').textContent = git.branch || detached;
  $('#git-br-btn').title = git.branch ? 'Switch or create a branch' : 'HEAD is detached: pick a branch to work on';
  $('#git-br-btn').classList.toggle('detached', !git.branch);
  // The lines added and removed by the uncommitted changes, staged and not.
  let add = 0;
  let del = 0;
  for (const m of [git.numstat.staged, git.numstat.unstaged]) for (const [a, d] of m.values()) { add += a; del += d; }
  $('#git-stats').replaceChildren(...(add || del ? [el('span', { class: 'git-add' }, `+${add.toLocaleString('en-US')}`), el('span', { class: 'git-del' }, `−${del.toLocaleString('en-US')}`)] : []));
}

const SYNC_ICONS = {
  fetch: '<svg viewBox="0 0 24 24"><path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/></svg>',
  pull: '<svg viewBox="0 0 24 24"><path d="M12 4v16M6 14l6 6 6-6"/></svg>',
  push: '<svg viewBox="0 0 24 24"><path d="M12 20V4M6 10l6-6 6 6"/></svg>',
};

// One button that does what's next: push what's ahead, pull what's behind, publish a new branch,
// or fetch when there's nothing else to do.
function syncAction() {
  const current = git.locals.find((b) => b.name === git.branch);
  if (!git.remotes.length) return null;
  if (git.branch && (!current?.upstream || current.gone)) return { icon: 'push', label: 'Publish', title: `Push ${git.branch} and track it on the remote`, run: push };
  if (current?.behind) return { icon: 'pull', label: `Pull ↓${current.behind}${current.ahead ? ` ↑${current.ahead}` : ''}`, title: 'Pull from the upstream branch', run: pull };
  if (current?.ahead) return { icon: 'push', label: `Push ↑${current.ahead}`, title: 'Push to the upstream branch', run: push };
  return { icon: 'fetch', label: 'Fetch', title: 'Fetch from all remotes', run: fetchAll };
}

function renderToolbar() {
  const changes = entries().length;
  $('#git-tab-changes').classList.toggle('on', git.view === 'changes');
  $('#git-tab-history').classList.toggle('on', git.view === 'history');
  $('#git-tab-count').textContent = changes ? `(${changes})` : '';
  const history = git.view === 'history';
  $('#git-view-diff').classList.toggle('hidden', history);
  $('#git-view-diff-label').textContent = git.diffOpen ? 'Hide Diff' : 'View Diff';
  $('#git-view-diff').classList.toggle('on', git.diffOpen);
  $('#git-view-opts').classList.toggle('hidden', history);
  $('#git-stage-split').classList.toggle('hidden', history);
  $('#git-more').classList.toggle('hidden', !history);
  const allStaged = changes > 0 && !git.unstaged.length;
  $('#git-stage-all').textContent = allStaged ? 'Unstage All' : 'Stage All';
  $('#git-stage-all').disabled = !changes;
  const a = syncAction();
  $('#git-sync').classList.toggle('hidden', !a);
  if (a) {
    const main = $('#git-sync-go');
    main.replaceChildren(el('span', { class: 'git-sync-icon', html: SYNC_ICONS[a.icon] }), a.label);
    main.title = a.title;
  }
  $('#git-diff-style').classList.toggle('hidden', history ? !git.selected : !git.diffOpen);
  $('#git-diff-style').title = git.diffStyle === 'split' ? 'Show changes inline' : 'Show changes side by side';
  $('#git-diff-style').classList.toggle('on', git.diffStyle === 'split');
}

$('#git-tab-changes').addEventListener('click', () => setView('changes'));
$('#git-tab-history').addEventListener('click', () => setView('history'));
$('#git-wt-btn').addEventListener('click', (e) => openWorktreePicker(e.currentTarget, git.root));
$('#git-br-btn').addEventListener('click', (e) => openBranchPicker(e.currentTarget, git.root));
$('#git-sync-go').addEventListener('click', () => syncAction()?.run());
$('#git-sync-more').addEventListener('click', (e) => syncMenu(e));
$('#git-more').addEventListener('click', (e) => moreMenu(e));
$('#git-stage-more').addEventListener('click', (e) => moreMenu(e));
$('#git-view-diff').addEventListener('click', () => toggleDiff());
$('#git-view-opts').addEventListener('click', (e) => viewMenu(e));
$('#git-stage-all').addEventListener('click', () => (git.unstaged.length ? stage(entries(), true) : unstage(git.staged, true)));
$('#git-diff-style').addEventListener('click', () => {
  git.diffStyle = git.diffStyle === 'split' ? 'unified' : 'split';
  localStorage.setItem('gitDiffStyle', git.diffStyle);
  renderToolbar();
  if (git.view === 'changes') renderProjectDiff(); else renderCommitDiff();
});

function syncMenu(e) {
  const multi = git.remotes.length > 1;
  const current = git.locals.find((b) => b.name === git.branch);
  const items = [
    { label: 'Fetch', action: fetchAll },
    { label: 'Pull', action: pull },
    { label: 'Pull (rebase)', action: () => pull(['--rebase']) },
    { label: current?.upstream ? 'Push' : 'Publish', action: push },
    { label: 'Force push', danger: true, action: forcePush },
  ];
  if (multi) {
    git.remotes.forEach((r, i) => {
      items.push({ label: `Fetch from ${r}`, divider: i === 0, action: () => gitOp(`Fetch ${r}`, [['fetch', '--prune', r]]) });
      if (git.branch) items.push({ label: `Push to ${r}/${git.branch}`, action: () => gitOp(`Push to ${r}`, [['push', '-u', r, git.branch]]) });
    });
  }
  const r = e.currentTarget.getBoundingClientRect();
  showMenu({ clientX: r.left, clientY: r.bottom + 4 }, items);
}

function moreMenu(e) {
  const all = entries();
  const tracked = all.filter((x) => x.status !== '?');
  const untracked = all.filter((x) => x.status === '?');
  const r = e.currentTarget.getBoundingClientRect();
  const items = [
    { label: 'Stage all', action: () => stage(all, true) },
    { label: 'Unstage all', action: () => unstage(git.staged, true) },
    { label: 'Stash all', divider: true, action: () => stashChanges('all') },
    { label: git.groupBy === 'tracked' ? 'Stash tracked' : 'Stash staged', action: () => stashChanges(git.groupBy === 'tracked' ? 'tracked' : 'staged') },
    { label: 'View stashes…', action: () => openStashPicker($('#git-more')) },
    { label: 'Apply latest stash', action: () => gitOp('Apply stash', [['stash', 'apply', '--index']]) },
    { label: 'Pop latest stash', action: () => gitOp('Pop stash', [['stash', 'pop', '--index']]) },
    { label: 'Merge a branch…', divider: true, action: mergeDialog },
    { label: 'Create tag at HEAD…', action: () => newTag(git.head) },
    { label: 'Open repository folder', divider: true, action: () => git.root && api.revealProject(git.root) },
    { label: 'Refresh', action: () => gitReload(true) },
    { label: `Restore tracked files (${tracked.length})`, divider: true, danger: true, action: restoreTracked },
    { label: `Move untracked files to trash (${untracked.length})`, danger: true, action: trashUntracked },
  ];
  showMenu({ clientX: r.left, clientY: r.bottom + 4 }, items);
}

function viewMenu(e) {
  const r = e.currentTarget.getBoundingClientRect();
  showMenu({ clientX: r.left, clientY: r.bottom + 4 }, [
    { label: 'Group by staged and unstaged', checked: git.groupBy === 'staged', action: () => setGroupBy('staged') },
    { label: 'Group by tracked and untracked', checked: git.groupBy === 'tracked', action: () => setGroupBy('tracked') },
    { label: 'Show as a list', divider: true, checked: !git.tree, action: () => setTree(false) },
    { label: 'Show as a tree', checked: git.tree, action: () => setTree(true) },
  ]);
}

function setTree(tree) {
  git.tree = tree;
  localStorage.setItem('gitTree', tree ? '1' : '0');
  renderGitFiles();
}

function setGroupBy(by) {
  git.groupBy = by;
  localStorage.setItem('gitGroupBy', by);
  renderGitFiles();
}

// ---------- the file list ----------

const STATUS_NAMES = { M: 'Modified', A: 'Added', D: 'Deleted', R: 'Renamed', C: 'Copied', T: 'Type changed', U: 'Conflicted', '?': 'Untracked' };
// A status as a small square, Zed's way: a dot for modified, plus for new, minus for deleted.
const ST_MARK = {
  M: '<circle cx="12" cy="12" r="2.2" fill="currentColor" stroke="none"/>',
  T: '<circle cx="12" cy="12" r="2.2" fill="currentColor" stroke="none"/>',
  A: '<path d="M12 8v8M8 12h8"/>',
  '?': '<path d="M12 8v8M8 12h8"/>',
  D: '<path d="M8 12h8"/>',
  R: '<path d="M8 12h8M13 9l3 3-3 3"/>',
  C: '<path d="M8 12h8M13 9l3 3-3 3"/>',
  U: '<path d="M12 7.5v5.5M12 16v.5"/>',
};
const statusIcon = (s) => el('span', {
  class: `git-sticon st-${s === '?' ? 'q' : s}`,
  title: STATUS_NAMES[s] || s,
  html: `<svg viewBox="0 0 24 24"><rect x="4" y="4" width="16" height="16" rx="3"/>${ST_MARK[s] || ST_MARK.M}</svg>`,
});
const CHEV = '<svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>';

function checkbox(state, onchange, title) {
  const c = el('input', { type: 'checkbox', class: 'git-check', title });
  c.checked = state === 'all';
  c.indeterminate = state === 'some';
  c.addEventListener('click', (e) => { e.stopPropagation(); e.preventDefault(); onchange(); });
  return c;
}

const stageState = (e) => (e.staged && !e.unstaged ? 'all' : e.staged ? 'some' : 'none');
const groupState = (list) => (list.every((e) => stageState(e) === 'all') ? 'all' : list.some((e) => stageState(e) !== 'none') ? 'some' : 'none');
const toggleStaged = (list) => (groupState(list) === 'all' ? unstage(list.map((e) => e.staged)) : stage(list.filter((e) => e.unstaged).map((e) => e.unstaged)));

function fileRow(e, list, depth) {
  const [dir, base] = splitPath(e.path);
  const counts = (() => {
    const a = git.numstat.staged.get(e.path);
    const b = git.numstat.unstaged.get(e.path);
    return a || b ? [(a?.[0] || 0) + (b?.[0] || 0), (a?.[1] || 0) + (b?.[1] || 0)] : null;
  })();
  const row = el('div', {
    class: `git-file${git.picked.has(e.path) ? ' picked' : ''}`,
    title: `${STATUS_NAMES[e.status] || e.status}: ${e.orig ? `${e.orig} → ` : ''}${e.path}`,
    onclick: (ev) => pickFile(ev, e, list),
    oncontextmenu: (ev) => { ev.preventDefault(); if (!git.picked.has(e.path)) pickFile({}, e, list, false); fileMenu(ev, e, list); },
  },
  statusIcon(e.status),
  el('span', { class: 'git-fname' }, base),
  git.tree ? null : el('span', { class: 'git-fdir' }, e.orig ? `${e.orig} →` : dir),
  el('span', { class: 'spacer' }),
  el('span', { class: 'git-fcount' }, ...(counts ? [el('span', { class: 'git-add' }, `+${counts[0].toLocaleString('en-US')}`), el('span', { class: 'git-del' }, `−${counts[1].toLocaleString('en-US')}`)] : [])),
  checkbox(stageState(e), () => toggleStaged([e]), e.status === 'U' ? 'Mark resolved' : stageState(e) === 'all' ? 'Unstage' : 'Stage'));
  if (depth) row.style.paddingLeft = `${10 + depth * 14}px`;
  return row;
}

// The tree view: folders with one child folder chain collapse into one row.
function treeRows(list) {
  const root = { dirs: new Map(), files: [] };
  for (const e of list) {
    const parts = e.path.split('/');
    let node = root;
    for (const part of parts.slice(0, -1)) {
      if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [], name: part });
      node = node.dirs.get(part);
    }
    node.files.push(e);
  }
  const under = (n) => [...n.files, ...[...n.dirs.values()].flatMap(under)];
  const walk = (node, depth, prefix) => {
    const out = [];
    for (const d of [...node.dirs.values()].sort((a, b) => a.name.localeCompare(b.name))) {
      let name = d.name;
      let cur = d;
      while (!cur.files.length && cur.dirs.size === 1) { cur = [...cur.dirs.values()][0]; name += `/${cur.name}`; }
      const key = `${prefix}${name}`;
      const folded = git.foldedDirs.has(key);
      const kids = under(cur);
      const row = el('div', {
        class: `git-dir${folded ? ' folded' : ''}`,
        onclick: () => { if (folded) git.foldedDirs.delete(key); else git.foldedDirs.add(key); renderGitFiles(); },
      },
      el('span', { class: 'git-section-chev', html: CHEV }),
      el('span', { class: 'git-fname' }, name),
      el('span', { class: 'spacer' }),
      checkbox(groupState(kids), () => toggleStaged(kids), 'Stage this folder'));
      row.style.paddingLeft = `${10 + depth * 14}px`;
      out.push(row);
      if (!folded) out.push(...walk(cur, depth + 1, `${key}/`));
    }
    out.push(...node.files.map((e) => fileRow(e, list, depth)));
    return out;
  };
  return walk(root, 0, '');
}

function section(title, list, emptyText, sectionEntries = list) {
  const folded = !!git.foldedSections[title];
  const head = el('div', { class: `git-section-head${folded ? ' folded' : ''}` },
    el('span', { class: 'git-section-chev', html: CHEV }),
    el('span', { class: 'git-section-title' }, title),
    el('span', { class: 'spacer' }),
    list.length ? el('span', { class: 'git-section-count' }, String(list.length)) : null,
    sectionEntries.length ? checkbox(groupState(sectionEntries), () => toggleStaged(sectionEntries), `Stage everything in ${title}`) : null);
  head.addEventListener('click', () => {
    git.foldedSections[title] = !folded;
    renderGitFiles();
  });
  const rows = git.tree ? treeRows(list) : list.map((e) => fileRow(e, list, 0));
  return el('div', { class: 'git-section' }, head,
    folded ? null : el('div', { class: 'git-file-list' }, ...(rows.length ? rows : [el('div', { class: 'git-empty' }, emptyText)])));
}

const smallBtn = (label, onclick, disabled) => {
  const b = el('button', { class: 'git-small', onclick }, label);
  b.disabled = !!disabled;
  return b;
};

function renderGitFiles() {
  const box = $('#git-files');
  const scroll = box.scrollTop;
  const all = entries();
  const live = new Set(all.map((e) => e.path));
  git.picked = new Set([...git.picked].filter((p) => live.has(p)));
  const parts = [];
  if (git.operation) {
    const actions = [smallBtn(`Abort ${git.operation}`, () => abortOperation())];
    if (git.operation === 'rebase' || git.operation === 'cherry-pick' || git.operation === 'revert') actions.unshift(smallBtn('Continue', () => continueOperation()));
    parts.push(el('div', { class: 'git-banner' }, el('span', {}, `A ${git.operation} is in progress. Resolve any conflicts and stage the files, then ${git.operation === 'merge' ? 'commit' : 'continue'}.`), ...actions));
  }
  const conflicts = all.filter((e) => e.status === 'U');
  const rest = all.filter((e) => e.status !== 'U');
  if (conflicts.length) parts.push(section('Conflicts', conflicts, ''));
  if (git.groupBy === 'staged') {
    const staged = rest.filter((e) => e.staged);
    const unstaged = rest.filter((e) => e.unstaged);
    parts.push(section('Staged', staged, 'No staged changes yet'), section('Unstaged', unstaged, 'No changes'));
  } else {
    parts.push(section('Tracked', rest.filter((e) => e.status !== '?'), 'No changes'));
    const untracked = rest.filter((e) => e.status === '?');
    if (untracked.length) parts.push(section('Untracked', untracked, ''));
  }
  box.replaceChildren(...parts);
  box.scrollTop = scroll;
}

function pickFile(ev, e, list, scrollTo = true) {
  if (ev.shiftKey && git.anchor) {
    const a = list.findIndex((x) => x.path === git.anchor);
    const b = list.findIndex((x) => x.path === e.path);
    git.picked = new Set(list.slice(Math.min(a, b), Math.max(a, b) + 1).map((x) => x.path));
  } else if (ev.ctrlKey) {
    if (git.picked.has(e.path)) git.picked.delete(e.path); else git.picked.add(e.path);
    git.anchor = e.path;
  } else {
    git.picked = new Set([e.path]);
    git.anchor = e.path;
  }
  renderGitFiles();
  if (scrollTo) toggleDiff(true).then(() => scrollDiffTo(e.path));
}

function fileMenu(ev, e, list) {
  const picked = list.filter((x) => git.picked.has(x.path));
  const many = picked.length > 1;
  const abs = joinPath(git.root, e.path);
  const unstaged = picked.filter((x) => x.unstaged).map((x) => x.unstaged);
  const staged = picked.filter((x) => x.staged).map((x) => x.staged);
  const items = [];
  if (unstaged.length) items.push({ label: many ? `Stage ${unstaged.length} files` : e.status === 'U' ? 'Mark resolved' : 'Stage', action: () => stage(unstaged) });
  if (staged.length) items.push({ label: many ? `Unstage ${staged.length} files` : 'Unstage', action: () => unstage(staged) });
  if (unstaged.some((f) => f.status !== 'U')) items.push({ label: many ? `Restore ${unstaged.length} files` : 'Restore', danger: true, action: () => discard(unstaged.filter((f) => f.status !== 'U')) });
  if (!many) {
    items.push({ label: 'View file', divider: true, action: () => api.openFile(abs) });
    items.push({ label: 'View file history', action: () => openFileHistory(e.path) });
    if (e.status === '?') {
      items.push({ label: 'Ignore this file', action: () => ignorePath(e.path) });
      const dir = splitPath(e.path)[0];
      if (dir) items.push({ label: `Ignore everything in ${dir}/`, action: () => ignorePath(`${dir}/`) });
    } else {
      items.push({ label: 'Open file permalink', action: () => filePermalink(e.path, true) });
      items.push({ label: 'Copy file permalink', action: () => filePermalink(e.path, false) });
    }
    items.push({ label: 'Show in folder', action: () => api.showFile(abs) });
    items.push({ label: 'Copy path', action: () => navigator.clipboard.writeText(e.path) });
  }
  showMenu(ev, items);
}

// A permanent link to the file at this commit, on the host of origin (or the first remote).
async function filePermalink(p, open) {
  const remote = git.remotes.includes('origin') ? 'origin' : git.remotes[0];
  if (!remote || !git.head) { gitBar('There is no remote to link to.', 'error'); return; }
  const r = await run(['remote', 'get-url', remote]);
  const m = /^(?:https?:\/\/(?:[^@/]+@)?|ssh:\/\/(?:[^@/]+@)?|[^@/]+@)([^/:]+)[:/](.+?)(?:\.git)?\/?$/.exec(r.stdout.trim());
  if (!m) { gitBar('Couldn\'t make a link from this remote\'s address.', 'error'); return; }
  const [, host, repo] = m;
  const middle = /bitbucket/i.test(host) ? 'src' : /gitlab/i.test(host) ? '-/blob' : 'blob';
  const url = `https://${host}/${repo}/${middle}/${git.head}/${p.split('/').map(encodeURIComponent).join('/')}`;
  if (open) window.open(url);
  else { navigator.clipboard.writeText(url); gitBar('Copied the link.', 'ok'); }
}

// ---------- the Project Diff ----------

function showDiffPlaceholder(text = 'No changes') {
  git.diffSeq++;
  unmountCards();
  diffBox.replaceChildren(el('div', { class: 'git-placeholder' }, text));
}

function scrollDiffTo(p) {
  const card = [...document.querySelectorAll('#git-diff .git-diff-card')].find((c) => c.dataset.path === p);
  if (card) card.scrollIntoView({ block: 'start' });
}

// What changed inside a replaced line: the words between the common start and end.
function wordSpans(oldText, newText) {
  const split = (s) => s.match(/\s+|\w+|[^\s\w]/g) || [];
  const a = split(oldText);
  const b = split(newText);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let end = 0;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  const part = (toks) => ({ pre: toks.slice(0, start).join(''), mid: toks.slice(start, toks.length - end).join(''), post: toks.slice(toks.length - end).join('') });
  // Replaced outright: highlighting the whole line says nothing.
  if (!start && !end) return null;
  return { old: part(a), new: part(b) };
}

function codeSpan(text, words) {
  if (!words) return el('span', { class: 'code' }, text);
  return el('span', { class: 'code' }, words.pre, el('span', { class: 'git-word' }, words.mid), words.post);
}

// A hunk's lines with their numbers, and the words changed in replaced lines.
function hunkRows(hunk) {
  const combined = hunk.head.startsWith('@@@');
  // "@@ -a,b +c,d @@", or "@@@ -a,b -a2,b2 +c,d @@@" for a conflicted file
  let oldN = +(/^@@+ -(\d+)/.exec(hunk.head)?.[1] || 0);
  let newN = +(/ \+(\d+)(?:,\d+)? @@/.exec(hunk.head)?.[1] || 0);
  const rows = [];
  for (const l of hunk.lines) {
    if (l[0] === '\\') { rows.push({ cls: 'meta', text: l }); continue; }
    const c = combined ? (l.slice(0, 2).includes('+') ? '+' : l.slice(0, 2).includes('-') ? '-' : ' ') : l[0];
    const text = (combined ? l.slice(2) : l.slice(1)).replace(/\r$/, '');
    if (c === '+') rows.push({ cls: 'add', sign: '+', b: newN++, text });
    else if (c === '-') rows.push({ cls: 'del', sign: '-', a: oldN++, text });
    else rows.push({ cls: 'ctx', sign: ' ', a: oldN++, b: newN++, text });
  }
  // Same-sized runs of removed then added lines are the same lines, edited.
  for (let i = 0; i < rows.length; i++) {
    if (rows[i].cls !== 'del') continue;
    let d = i;
    while (rows[d + 1]?.cls === 'del') d++;
    let e = d;
    while (rows[e + 1]?.cls === 'add') e++;
    if (e - d === d - i + 1) {
      for (let k = 0; k <= d - i; k++) {
        const w = wordSpans(rows[i + k].text, rows[d + 1 + k].text);
        if (w) { rows[i + k].words = w.old; rows[d + 1 + k].words = w.new; }
      }
    }
    i = e;
  }
  return rows;
}

function unifiedHunk(rows) {
  const body = el('div', { class: 'git-hunk' });
  for (const r of rows) {
    body.append(el('div', { class: `git-line ${r.cls}` },
      el('span', { class: 'ln' }, r.a == null ? '' : String(r.a)), el('span', { class: 'ln' }, r.b == null ? '' : String(r.b)),
      el('span', { class: 'sign' }, r.cls === 'meta' ? '' : r.sign), codeSpan(r.text, r.words)));
  }
  return body;
}

// Side by side: runs of removed and added lines face each other.
function splitHunk(rows) {
  const body = el('div', { class: 'git-hunk split' });
  const side = (r, n) => (r
    ? el('div', { class: `git-line side ${r.cls}` }, el('span', { class: 'ln' }, r[n] == null ? '' : String(r[n])), codeSpan(r.text, r.words))
    : el('div', { class: 'git-line side blank' }, el('span', { class: 'ln' }), el('span', { class: 'code' })));
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    if (r.cls === 'del' || r.cls === 'add') {
      const dels = [];
      const adds = [];
      while (rows[i]?.cls === 'del') dels.push(rows[i++]);
      while (rows[i]?.cls === 'add') adds.push(rows[i++]);
      i--;
      for (let k = 0; k < Math.max(dels.length, adds.length); k++) body.append(side(dels[k], 'a'), side(adds[k], 'b'));
    } else if (r.cls === 'meta') body.append(side(r, 'a'), side(null, 'b'));
    else body.append(side(r, 'a'), side(r, 'b'));
  }
  return body;
}

function hunkBox(hunk, actions, tag) {
  const head = el('div', { class: 'git-hunk-head' }, el('span', { class: 'mono' }, hunk.head.replace(/^@@+ [^@]*@@+ ?/, '') || hunk.head));
  if (tag) head.prepend(el('span', { class: 'git-hunk-tag' }, tag));
  head.append(el('span', { class: 'spacer' }), ...actions);
  const rows = hunkRows(hunk);
  return { box: el('div', { class: 'git-hunk-box' }, head, git.diffStyle === 'split' ? splitHunk(rows) : unifiedHunk(rows)), lines: rows.length };
}

function noHunksNote(d) {
  const header = d?.header || [];
  const binary = header.some((l) => l.startsWith('Binary files') || l.startsWith('GIT binary patch'));
  const mode = header.filter((l) => / mode /.test(l));
  return el('div', { class: 'git-empty' }, binary ? 'Binary file: no text diff.' : mode.length ? `File mode changed (${mode.join(', ')}).` : 'No content changes.');
}

// Cards build their lines when they come near the view: a large diff is tens of thousands of nodes.
const diffBox = $('#git-diff');
const cardBodies = new Map(); // card -> build its lines
const cardObserver = new IntersectionObserver((seen) => {
  for (const s of seen) if (s.isIntersecting) mountCard(s.target);
}, { root: diffBox, rootMargin: '1000px 0px' });

function mountCard(card) {
  const build = cardBodies.get(card);
  if (!build) return;
  cardBodies.delete(card);
  cardObserver.unobserve(card);
  card.style.minHeight = '';
  build(card);
}

function unmountCards() {
  cardObserver.disconnect();
  cardBodies.clear();
}

// Shows the cards, building at once those on screen so nothing flickers, the rest when reached.
function showCards(cards) {
  const scroll = diffBox.scrollTop;
  const top = scroll - 300;
  const bottom = scroll + diffBox.clientHeight + 300;
  unmountCards();
  let y = 0;
  for (const { card, build, height } of cards) {
    const h = build ? height : 34;
    if (build) {
      if (y + h >= top && y <= bottom) build(card);
      else { cardBodies.set(card, build); card.style.minHeight = `${h}px`; cardObserver.observe(card); }
    }
    y += h;
  }
  diffBox.replaceChildren(...cards.map((c) => c.card));
  diffBox.scrollTop = scroll;
}

const estimateHeight = (parts) => 34 + parts.reduce((sum, p) => sum + (p?.hunks || []).reduce((n, h) => n + 38 + h.lines.length * 19, 0), 0);

function diffCard(key, head, bodyFn, parts) {
  const folded = git.foldedFiles.has(key);
  const fold = el('button', {
    class: 'git-card-fold',
    title: folded ? 'Unfold' : 'Fold',
    html: CHEV,
    onclick: () => { if (git.foldedFiles.has(key)) git.foldedFiles.delete(key); else git.foldedFiles.add(key); if (git.view === 'changes') renderProjectDiff(); else renderCommitDiff(); },
  });
  const card = el('div', { class: `git-diff-card${folded ? ' folded' : ''}` }, el('div', { class: 'git-diff-title' }, fold, ...head));
  card.dataset.path = key;
  return { card, build: folded ? null : bodyFn, height: estimateHeight(parts) };
}

// A file's hunks, up to a limit unless asked for all of them.
function appendHunks(card, list) {
  let left = git.fullDiff ? Infinity : DIFF_LINES_SHOWN;
  for (const { hunk, actions, tag } of list) {
    if (left <= 0) {
      card.append(el('div', { class: 'git-more', onclick: () => { git.fullDiff = true; if (git.view === 'changes') renderProjectDiff(); else renderCommitDiff(); } }, 'Large diff: the rest is hidden. Show everything'));
      return;
    }
    const { box, lines } = hunkBox(hunk, actions(hunk), tag);
    left -= lines;
    card.append(box);
  }
}

function renderProjectDiff() {
  git.diffSeq++;
  const all = entries();
  if (!all.length) {
    unmountCards();
    diffBox.replaceChildren(el('div', { class: 'git-placeholder' }, git.head ? 'No uncommitted changes' : 'No commits yet'));
    return;
  }
  showCards(all.map((e) => {
    const [dir, base] = splitPath(e.path);
    const head = [
      checkbox(stageState(e), () => toggleStaged([e]), stageState(e) === 'all' ? 'Unstage file' : 'Stage file'),
      statusIcon(e.status),
      el('span', { class: 'git-card-name sel' }, base),
      el('span', { class: 'git-fdir sel' }, e.orig ? `${e.orig} →` : dir),
      el('span', { class: 'spacer' }),
    ];
    if (e.unstaged && e.status !== 'U') head.push(smallBtn('Restore', () => discard([e.unstaged])));
    head.push(smallBtn('View file', () => api.openFile(joinPath(git.root, e.path))));
    const d = git.diffs.get(e.path) || {};
    return diffCard(e.path, head, (card) => {
      if (e.status === 'U') { conflictBody(e, card); return; }
      const list = [
        ...(d.staged?.hunks || []).map((hunk) => ({ hunk, tag: e.unstaged ? 'Staged' : '', actions: (h) => [smallBtn('Unstage', () => applyHunk(d.staged.header, h, 'unstage'))] })),
        ...(d.unstaged?.hunks || []).map((hunk) => ({ hunk, tag: '', actions: (h) => (e.status === '?' ? [] : [smallBtn('Stage', () => applyHunk(d.unstaged.header, h, 'stage')), smallBtn('Restore', () => applyHunk(d.unstaged.header, h, 'discard'))]) })),
      ];
      if (list.length) appendHunks(card, list);
      else card.append(d.staged || d.unstaged ? noHunksNote(d.staged || d.unstaged) : el('div', { class: 'git-empty' }, e.status === '?' ? 'Too many new files to show them all. Stage or open this one.' : 'No content changes.'));
    }, [d.staged, d.unstaged]);
  }));
}

// ---------- merge conflicts ----------

// A file with conflict markers, as plain parts and { ours, theirs, base, oursLabel, theirsLabel }.
function parseConflicts(text) {
  const parts = [];
  let plain = '';
  let c = null;
  let at = null; // which side of the conflict the lines belong to
  for (const line of text.split(/(?<=\n)/)) {
    const bare = line.replace(/\r?\n$/, '');
    if (!c && bare.startsWith('<<<<<<<')) {
      if (plain) { parts.push({ plain }); plain = ''; }
      c = { oursLabel: bare.slice(7).trim(), ours: '', base: '', theirs: '', theirsLabel: '', raw: line };
      at = 'ours';
    } else if (c && at === 'ours' && bare.startsWith('|||||||')) { at = 'base'; c.raw += line; } else if (c && (at === 'ours' || at === 'base') && bare.startsWith('=======')) { at = 'theirs'; c.raw += line; } else if (c && at === 'theirs' && bare.startsWith('>>>>>>>')) {
      c.theirsLabel = bare.slice(7).trim();
      c.raw += line;
      parts.push(c);
      c = null;
    } else if (c) { c[at] += line; c.raw += line; } else plain += line;
  }
  if (c) parts.push({ plain: c.raw }); // an unfinished marker is just text
  if (plain) parts.push({ plain });
  return parts;
}

const conflictSideLabel = (label) => (/^(HEAD|Updated upstream)$/i.test(label) && git.branch ? git.branch : label.replace(/^[0-9a-f]{7,40} \(|\)$/g, '') || 'this side');

async function conflictBody(e, card) {
  const seq = git.diffSeq;
  const r = await api.readFile(git.root, e.path);
  if (seq !== git.diffSeq) return;
  if (r.text == null) { card.append(el('div', { class: 'git-empty' }, r.error || 'This file can\'t be shown.')); return; }
  const parts = parseConflicts(r.text);
  const conflicts = parts.filter((p) => p.ours != null);
  if (!conflicts.length) {
    card.append(el('div', { class: 'git-conflict-done' }, 'No conflict markers left in this file.', smallBtn('Mark resolved', () => stage([e.unstaged]))));
    return;
  }
  // Around a conflict, a few lines of what's unchanged.
  const context = (text, atStart, atEnd) => {
    const ls = text.split(/(?<=\n)/);
    if (ls.length <= 6) return text;
    return `${atStart ? '' : ls.slice(0, 3).join('')}…\n${atEnd ? '' : ls.slice(-3).join('')}`;
  };
  parts.forEach((p, i) => {
    if (p.plain != null) {
      card.append(el('pre', { class: 'git-conflict-ctx' }, context(p.plain, i === 0, i === parts.length - 1).replace(/\s+$/, '')));
      return;
    }
    const ours = conflictSideLabel(p.oursLabel);
    const theirs = conflictSideLabel(p.theirsLabel);
    card.append(el('div', { class: 'git-conflict' },
      el('div', { class: 'git-conflict-bar' },
        smallBtn(`Use ${ours}`, () => resolveConflict(e, i, 'ours')),
        smallBtn(`Use ${theirs}`, () => resolveConflict(e, i, 'theirs')),
        smallBtn('Use both', () => resolveConflict(e, i, 'both'))),
      el('div', { class: 'git-conflict-label ours' }, ours),
      el('pre', { class: 'git-conflict-code ours' }, p.ours.replace(/\s+$/, '') || ' '),
      el('div', { class: 'git-conflict-label theirs' }, theirs),
      el('pre', { class: 'git-conflict-code theirs' }, p.theirs.replace(/\s+$/, '') || ' ')));
  });
}

// Reads the file again so nothing typed since is lost, and replaces one conflict with the choice.
async function resolveConflict(e, index, choice) {
  const r = await api.readFile(git.root, e.path);
  if (r.text == null) { gitBar(r.error || 'Couldn\'t read the file.', 'error'); return; }
  const parts = parseConflicts(r.text);
  const c = parts[index];
  if (c?.ours == null) { gitReload(true); return; }
  parts[index] = { plain: choice === 'ours' ? c.ours : choice === 'theirs' ? c.theirs : c.ours + c.theirs };
  const w = await api.writeFile(git.root, e.path, parts.map((p) => p.plain ?? p.raw).join(''));
  if (w.error) { gitBar(w.error, 'error'); return; }
  renderProjectDiff();
}

// ---------- working copy actions ----------

async function stage(list, all = false) {
  if (!list.length) return;
  const steps = all ? [['add', '-A']] : chunked(['add', '-A'], list.map((f) => f.path));
  await gitOp('Stage', steps, { quiet: true });
}

async function unstage(list, all = false) {
  list = list.filter(Boolean);
  if (!list.length) return;
  const paths = list.flatMap((f) => (f.orig ? [f.orig, f.path] : [f.path]));
  let steps;
  if (!git.head) steps = all ? [['rm', '-r', '-q', '--cached', '.']] : chunked(['rm', '-r', '-q', '--cached'], paths);
  else steps = all ? [['reset', '-q']] : chunked(['reset', '-q', 'HEAD'], paths);
  await gitOp('Unstage', steps, { quiet: true });
}

async function discard(list) {
  if (!list.length) return;
  const untracked = list.filter((f) => f.status === '?');
  const tracked = list.filter((f) => f.status !== '?');
  const what = list.length === 1 ? list[0].path : `${list.length} files`;
  const ok = await gitConfirm('Restore', `Throw away the unstaged changes to ${what}?${untracked.length ? ` ${untracked.length === list.length ? 'It is untracked and will be deleted.' : `${untracked.length} untracked file(s) will be deleted.`}` : ''} This can't be undone.`, 'Restore');
  if (!ok) return;
  await gitOp('Restore', [
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
  if (mode === 'discard' && !(await gitConfirm('Restore hunk', 'Throw away this hunk\'s changes? This can\'t be undone.', 'Restore'))) return;
  await gitOp({ stage: 'Stage hunk', unstage: 'Unstage hunk', discard: 'Restore hunk' }[mode], [{ args, input: patch }], { quiet: true });
}

async function restoreTracked() {
  if (!entries().some((e) => e.status !== '?')) { gitBar('There are no changes to restore.', 'error'); return; }
  if (!(await gitConfirm('Restore tracked files', 'Reset every tracked file to the last commit, staged and unstaged changes alike. This can\'t be undone.', 'Restore'))) return;
  gitOp('Restore tracked files', git.head ? [['reset', '--hard', '-q']] : [['rm', '-r', '-q', '--cached', '.']]);
}

async function trashUntracked() {
  const list = git.unstaged.filter((f) => f.status === '?');
  if (!list.length) { gitBar('There are no untracked files.', 'error'); return; }
  if (!(await gitConfirm('Move to trash', `Move ${list.length} untracked file${list.length === 1 ? '' : 's'} to the Recycle Bin?`, 'Move to trash'))) return;
  const r = await api.gitTrash(git.root, list.map((f) => f.path));
  if (r.failed) gitBar(`${r.failed} file(s) couldn't be moved to the Recycle Bin.`, 'error');
  gitReload(true);
}

async function ignorePath(pattern) {
  if (!(await api.gitIgnore(git.root, `/${pattern}`))) { gitBar('Couldn\'t update .gitignore.', 'error'); return; }
  gitBar(`Added /${pattern} to .gitignore.`, 'ok');
  gitReload(true);
}

// "Stash all" includes untracked files, "tracked" leaves them, "staged" takes only the index.
async function stashChanges(kind) {
  if (!entries().length) { gitBar('There are no changes to stash.', 'error'); return; }
  const v = await gitDialog({
    title: { all: 'Stash all', tracked: 'Stash tracked', staged: 'Stash staged' }[kind],
    fields: [{ id: 'msg', label: 'Name', placeholder: 'Optional' }],
    ok: 'Stash',
  });
  if (!v) return;
  const args = ['stash', 'push', ...{ all: ['--include-untracked'], tracked: [], staged: ['--staged'] }[kind]];
  if (v.msg) args.push('-m', v.msg);
  gitOp('Stash', [args]);
}

// ---------- commit ----------

const commitLabel = () => {
  if (git.commitOpts.amend) return 'Amend';
  if (git.staged.length || git.operation === 'merge') return 'Commit';
  return git.unstaged.some((f) => f.status !== '?') ? 'Commit Tracked' : 'Commit';
};

function renderCommitBox() {
  $('#git-commit-label').textContent = commitLabel();
  const o = git.commitOpts;
  const flags = [o.amend && 'Amend', o.signoff && 'Signoff', o.noVerify && 'Skip hooks', o.push && 'Then push'].filter(Boolean);
  $('#git-commit-flags').textContent = flags.join(' · ');
  const last = $('#git-last-commit');
  last.classList.toggle('hidden', !git.last);
  if (git.last) {
    const undo = el('button', { id: 'git-uncommit', class: 'icon-btn small', title: 'Uncommit', html: '<svg viewBox="0 0 24 24"><path d="M9 14 4 9l5-5"/><path d="M4 9h10a6 6 0 0 1 0 12h-3"/></svg>', onclick: uncommit });
    undo.disabled = !git.last.hasParent;
    last.replaceChildren(
      el('span', { class: 'git-last-subject', title: `${git.last.subject}\n${fmtDate(git.last.at)}` }, git.last.subject),
      undo,
      el('button', { class: 'icon-btn small', title: 'Show in history', html: '<svg viewBox="0 0 24 24"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 4-6 3-12 7"/></svg>', onclick: () => git.head && jumpTo(git.head) }));
  }
}

$('#git-commit-go').addEventListener('click', () => doCommit());
$('#git-msg-expand').addEventListener('click', () => $('#git-commit-box').classList.toggle('tall'));
$('#git-commit-fold').addEventListener('click', () => {
  const folded = $('#git-commit-box').classList.toggle('folded');
  localStorage.setItem('gitCommitFolded', folded ? '1' : '0');
});
$('#git-commit-box').classList.toggle('folded', localStorage.getItem('gitCommitFolded') === '1');
$('#git-commit-more').addEventListener('click', (e) => {
  const o = git.commitOpts;
  const flip = (k) => async () => {
    o[k] = !o[k];
    if (k === 'amend' && o.amend && !$('#git-msg').value.trim() && git.head) {
      const r = await run(['log', '-1', '--format=%B']);
      if (r.code === 0) $('#git-msg').value = r.stdout.trim();
    }
    renderCommitBox();
  };
  showMenu(e, [
    { label: 'Amend the last commit', checked: o.amend, action: flip('amend') },
    { label: 'Signoff', checked: o.signoff, action: flip('signoff') },
    { label: 'Skip hooks', checked: o.noVerify, action: flip('noVerify') },
    { label: 'Push after committing', checked: o.push, action: flip('push') },
  ], e.currentTarget.getBoundingClientRect());
});
$('#git-msg').addEventListener('keydown', (e) => { if (e.key === 'Enter' && e.ctrlKey) { e.preventDefault(); doCommit(); } });

async function doCommit() {
  const msg = $('#git-msg').value.trim();
  const o = git.commitOpts;
  if (!msg) { $('#git-msg').focus(); gitBar('Write a commit message first.', 'error'); return; }
  const steps = [];
  if (!git.staged.length && !o.amend && git.operation !== 'merge') {
    // Nothing staged: commit what's tracked. Untracked files are only added on request.
    if (!git.unstaged.some((f) => f.status !== '?')) { gitBar(git.unstaged.length ? 'Only untracked files changed: tick the ones to commit.' : 'There is nothing to commit.', 'error'); return; }
    steps.push(['add', '-u']);
  }
  const args = ['commit', '-F', '-'];
  if (o.amend) args.push('--amend');
  if (o.signoff) args.push('--signoff');
  if (o.noVerify) args.push('--no-verify');
  steps.push({ args, input: `${msg}\n` });
  const ok = await gitOp(o.amend ? 'Amend commit' : 'Commit', steps);
  if (!ok) return;
  $('#git-msg').value = '';
  const pushAfter = o.push;
  Object.assign(o, { amend: false, signoff: false, noVerify: false, push: false });
  renderCommitBox();
  if (pushAfter) await push();
}

// The last commit's changes go back to being staged.
function uncommit() {
  if (git.last?.hasParent) gitOp('Uncommit', [['reset', '--soft', 'HEAD^']]);
}

// ---------- fetch, pull, push ----------

function fetchAll() {
  if (!git.remotes.length) { gitBar('This repository has no remotes to fetch from.', 'error'); return; }
  gitOp('Fetch', [['fetch', '--all', '--prune']]);
}

function pull(extra = []) {
  const current = git.locals.find((b) => b.name === git.branch);
  if (git.branch && !current?.upstream) { gitBar(`${git.branch} has no upstream branch to pull from. Publish it first, or check out a remote branch.`, 'error'); return; }
  gitOp('Pull', [['pull', '--no-edit', ...extra]]);
}

async function push() {
  if (!git.branch) { gitBar('HEAD is detached: pick a branch to push.', 'error'); return; }
  const current = git.locals.find((b) => b.name === git.branch);
  if (current?.upstream && !current.gone) { await gitOp('Push', [['push']]); return; }
  if (!git.remotes.length) { gitBar('This repository has no remote to push to. Add one with git remote add.', 'error'); return; }
  // Git's own settings decide first: pushRemote for the branch, then remote.pushDefault.
  const conf = await run(['config', '--get-regexp', `^(branch\\.${git.branch.replace(/[.^$*+?()[\]{}|\\]/g, '\\$&')}\\.pushRemote|remote\\.pushDefault)$`]);
  const configured = conf.stdout.split('\n').map((l) => l.split(' ')).find((p) => p[1] && git.remotes.includes(p[1]))?.[1];
  const preferred = configured || (git.remotes.includes('origin') ? 'origin' : git.remotes[0]);
  if (git.remotes.length === 1 || configured) { await gitOp('Publish', [['push', '-u', preferred, git.branch]]); return; }
  const v = await gitDialog({
    title: 'Publish',
    text: `${git.branch} has no upstream branch yet. Push it and track the remote branch?`,
    fields: [{ id: 'remote', label: 'Remote', type: 'select', options: git.remotes, value: preferred }],
    ok: 'Push',
  });
  if (v) await gitOp('Publish', [['push', '-u', v.remote, git.branch]]);
}

async function forcePush() {
  if (!git.branch) { gitBar('HEAD is detached: pick a branch to push.', 'error'); return; }
  if (await gitConfirm('Force push', `Overwrite the remote ${git.branch} with your version? Commits only on the remote are lost. The push stops if someone else pushed since your last fetch.`, 'Force push')) {
    gitOp('Force push', [['push', '--force-with-lease']]);
  }
}

async function mergeDialog() {
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
}

document.addEventListener('keydown', (e) => {
  if (git.open && e.key === 'F5') { e.preventDefault(); gitReload(true); }
});

// ---------- stashes ----------

function openStashPicker(anchor) {
  const drop = async (s) => {
    if (await gitConfirm('Drop stash', `Delete ${s.ref} (${s.message})? This can't be undone.`, 'Drop')) gitOp(`Drop ${s.ref}`, [['stash', 'drop', s.ref]]);
  };
  openListPicker({
    anchor,
    placeholder: 'Find a stash…',
    rows: (q) => {
      const list = git.stashes.filter((s) => !q || s.message.toLowerCase().includes(q));
      if (!list.length) return [{ note: git.stashes.length ? 'No matches' : 'No stashes yet' }];
      return list.map((s) => ({
        label: s.message.replace(/^(WIP on|On) [^:]+: /, '') || s.ref,
        sub: `${s.ref} · ${s.at ? fmtDate(s.at) : ''}`,
        action: () => showStash(s.ref),
        buttons: [
          { label: 'Apply', action: () => gitOp(`Apply ${s.ref}`, [['stash', 'apply', '--index', s.ref]]) },
          { label: 'Pop', action: () => gitOp(`Pop ${s.ref}`, [['stash', 'pop', '--index', s.ref]]) },
          { label: 'Drop', danger: true, action: () => drop(s) },
        ],
      }));
    },
  });
}

async function showStash(ref) {
  await setView('history');
  selectRow(ref);
}

// ---------- file history ----------

async function openFileHistory(p) {
  git.fileFilter = p;
  git.selected = null;
  git.logStale = true;
  await setView('history');
}

$('#git-log-filter-clear').addEventListener('click', async () => {
  git.fileFilter = null;
  git.selected = null;
  git.logStale = true;
  await setView('history');
});

// ---------- history: the commit graph ----------

function laneX(i) { return 10 + i * LANE_W; }

function graphSvg(c, width) {
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
  for (const pc of parentCols) s += curve(laneX(col), mid, laneX(pc), ROW_H, color(pc));
  const head = c.hash === git.head;
  s += `<circle cx="${laneX(col)}" cy="${mid}" r="${head ? 4.5 : 3.5}" fill="${head ? 'var(--main-bg)' : color(col)}" stroke="${color(col)}" stroke-width="${head ? 2.5 : 1}"/>`;
  return `<svg class="git-graph" width="${width}" height="${ROW_H}" viewBox="0 0 ${width} ${ROW_H}">${s}</svg>`;
}

function refBadge(r) {
  const cls = { head: 'local head', local: 'local', remote: 'remote', tag: 'tag', detached: 'head' }[r.kind];
  const badge = el('span', { class: `git-ref ${cls}`, title: r.kind === 'tag' ? `Tag ${r.name}` : r.name }, r.name);
  badge.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (r.kind === 'tag') { const t = git.tags.find((x) => x.name === r.name); if (t) tagMenu(e, t); return; }
    if (r.kind === 'remote') { const b = git.remoteBranches.find((x) => x.name === r.name); if (b) remoteBranchMenu(e, b); return; }
    const b = git.locals.find((x) => x.name === r.name);
    if (b) branchMenu(e, b);
  });
  return badge;
}

function logRow(c, width) {
  const row = el('div', {
    class: `git-row${git.selected === c.hash ? ' selected' : ''}${c.hash === git.head ? ' head' : ''}`,
    onclick: () => selectRow(c.hash),
  },
  el('span', { class: 'c-graph', html: graphSvg(c, width) }),
  el('span', { class: 'c-desc' }, ...c.refs.map(refBadge), el('span', { class: 'subject' }, c.subject)),
  el('span', { class: 'c-date' }, fmtDate(c.at)),
  el('span', { class: 'c-author', title: `${c.author} <${c.email}>` }, c.author),
  el('span', { class: 'c-hash' }, short(c.hash)));
  row.dataset.key = c.hash;
  row.addEventListener('contextmenu', (e) => { e.preventDefault(); selectRow(c.hash); commitMenu(e, c); });
  return row;
}

// The list of commits builds only the rows in view: each has its own graph.
let logWindow = null;

function renderLog() {
  const rows = $('#git-log-rows');
  const width = Math.min(git.graphLanes || 1, 16) * LANE_W + 12;
  gitEl.style.setProperty('--graph-w', `${Math.max(width, 64)}px`);
  $('#git-log-filter').classList.toggle('hidden', !git.fileFilter);
  $('#git-log-filter-path').textContent = git.fileFilter || '';
  const scroll = rows.scrollTop;
  logWindow = { width, from: -1, to: -1, inner: el('div', { class: 'git-log-inner' }) };
  logWindow.inner.style.height = `${git.commits.length * ROW_H}px`;
  const after = [];
  if (git.commits.length >= git.limit) {
    after.push(el('div', { class: 'git-more', onclick: async () => { git.limit += LOG_PAGE; await loadLog(); renderLog(); } }, `Showing the latest ${git.limit} commits. Load more`));
  }
  if (!git.commits.length) after.push(el('div', { class: 'git-placeholder' }, 'No commits yet'));
  rows.replaceChildren(logWindow.inner, ...after);
  rows.scrollTop = scroll;
  drawLogRows();
}

function drawLogRows() {
  const rows = $('#git-log-rows');
  if (!logWindow) return;
  const first = Math.max(0, Math.floor(rows.scrollTop / ROW_H) - 15);
  const last = Math.min(git.commits.length, Math.ceil((rows.scrollTop + (rows.clientHeight || 600)) / ROW_H) + 15);
  if (first === logWindow.from && last === logWindow.to) return;
  logWindow.from = first;
  logWindow.to = last;
  const out = [];
  for (let i = first; i < last; i++) {
    const row = logRow(git.commits[i], logWindow.width);
    row.style.top = `${i * ROW_H}px`;
    out.push(row);
  }
  logWindow.inner.replaceChildren(...out);
}

let logScrollFrame = 0;
$('#git-log-rows').addEventListener('scroll', () => {
  if (logScrollFrame) return;
  logScrollFrame = requestAnimationFrame(() => { logScrollFrame = 0; drawLogRows(); });
});

function scrollLogTo(i, block = 'nearest') {
  const rows = $('#git-log-rows');
  const y = i * ROW_H;
  if (block === 'center') rows.scrollTop = y - rows.clientHeight / 2;
  else if (y < rows.scrollTop) rows.scrollTop = y;
  else if (y + ROW_H > rows.scrollTop + rows.clientHeight) rows.scrollTop = y + ROW_H - rows.clientHeight;
  drawLogRows();
}

function selectRow(key) {
  git.selected = key;
  git.fullDiff = false;
  for (const r of document.querySelectorAll('#git-log-rows .git-row')) r.classList.toggle('selected', r.dataset.key === key);
  renderToolbar();
  renderHistoryDetail();
}

function jumpTo(hash) {
  const go = () => {
    selectRow(hash);
    const i = git.commits.findIndex((c) => c.hash === hash);
    if (i >= 0) scrollLogTo(i, 'center');
    else gitBar('That commit is older than the commits loaded. Use "Load more" at the end of the history.', 'ok');
  };
  if (git.view !== 'history') setView('history').then(go); else go();
}

$('#git-log-rows').addEventListener('keydown', (e) => {
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
  e.preventDefault();
  const i = git.commits.findIndex((c) => c.hash === git.selected);
  const n = Math.max(0, Math.min(git.commits.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)));
  if (!git.commits[n]) return;
  selectRow(git.commits[n].hash);
  scrollLogTo(n);
});

// ---------- history: a commit or stash, all its changes ----------

async function renderHistoryDetail() {
  const box = $('#git-files');
  const rev = git.selected;
  if (!rev) {
    box.replaceChildren(el('div', { class: 'git-placeholder' }, 'Select a commit'));
    showDiffPlaceholder('Select a commit to view its changes');
    git.detail = null;
    return;
  }
  const cacheKey = `${rev}|${git.fileFilter || ''}`;
  if (git.detail?.cacheKey !== cacheKey) {
    box.replaceChildren(el('div', { class: 'git-placeholder' }, 'Loading…'));
    showDiffPlaceholder('Loading…');
    const detail = git.detailCache.get(cacheKey) || (await loadCommitDetail(rev));
    if (git.selected !== rev) return;
    git.detail = detail;
    if (detail) detail.cacheKey = cacheKey;
    if (isHash(rev) && detail) git.detailCache.set(cacheKey, detail);
  }
  renderCommitFiles();
  renderCommitDiff();
}

async function loadCommitDetail(rev) {
  const meta = await run(['show', '-s', '--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B', rev]);
  if (meta.code !== 0) return null;
  const [hash, parents, an, ae, at, cn, ce, ct, body] = meta.stdout.split('\0');
  const parent = parents ? parents.split(' ')[0] : null;
  const only = git.fileFilter ? ['--', git.fileFilter] : [];
  const [names, patch] = await Promise.all([
    parent ? run(['diff', '--name-status', '-z', '-M', parent, hash, ...only]) : run(['diff-tree', '--root', '-r', '--name-status', '-z', '-M', '--no-commit-id', hash, ...only]),
    parent ? run(['diff', '-M', parent, hash, ...only]) : run(['diff-tree', '-p', '--root', '-M', '--no-commit-id', hash, ...only]),
  ]);
  const tok = names.stdout.split('\0').filter((s) => s !== '');
  const changed = [];
  for (let i = 0; i < tok.length; i++) {
    const st = tok[i];
    if (st[0] === 'R' || st[0] === 'C') { changed.push({ status: st[0], orig: tok[i + 1], path: tok[i + 2] }); i += 2; } else { changed.push({ status: st[0], path: tok[i + 1] }); i += 1; }
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
    files: changed,
    diffs: diffsByFile(patch.stdout || ''),
  };
}

function commitFileMenu(ev, f) {
  const d = git.detail;
  const abs = joinPath(git.root, f.path);
  showMenu(ev, [
    { label: 'View current version', action: () => api.openFile(abs) },
    { label: 'View file history', action: () => openFileHistory(f.path) },
    { label: 'Show in folder', action: () => api.showFile(abs) },
    { label: 'Copy path', action: () => navigator.clipboard.writeText(f.path) },
    { label: 'Restore this version to the working copy', divider: true, action: async () => {
      if (await gitConfirm('Restore file', `Replace ${f.path} in the working copy with its version from ${short(d.hash)}?`, 'Restore')) gitOp('Restore file', [['checkout', d.hash, '--', f.path]]);
    } },
  ]);
}

function renderCommitFiles() {
  const box = $('#git-files');
  const d = git.detail;
  if (!d) { box.replaceChildren(el('div', { class: 'git-placeholder' }, 'Couldn\'t load this commit.')); return; }
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
    d.committer !== d.author ? metaRow('Committer', el('span', { class: 'sel' }, d.committer)) : null,
    stash ? el('div', { class: 'git-meta-actions' },
      smallBtn('Apply', () => gitOp(`Apply ${stash.ref}`, [['stash', 'apply', '--index', stash.ref]])),
      smallBtn('Pop', () => gitOp(`Pop ${stash.ref}`, [['stash', 'pop', '--index', stash.ref]]))) : null);
  const rows = d.files.map((f) => {
    const [dir, base] = splitPath(f.path);
    return el('div', {
      class: 'git-file',
      title: `${STATUS_NAMES[f.status] || f.status}: ${f.orig ? `${f.orig} → ` : ''}${f.path}`,
      onclick: () => scrollDiffTo(f.path),
      oncontextmenu: (ev) => { ev.preventDefault(); commitFileMenu(ev, f); },
    }, statusIcon(f.status), el('span', { class: 'git-fname' }, base), el('span', { class: 'git-fdir' }, f.orig ? `${f.orig} →` : dir));
  });
  const note = d.parents.length > 1 ? el('div', { class: 'git-empty' }, 'Changes compared with the first parent.') : null;
  box.replaceChildren(meta, el('div', { class: 'git-section' },
    el('div', { class: 'git-section-head static' }, el('span', { class: 'git-section-title' }, 'Files'), el('span', { class: 'git-section-count' }, String(d.files.length))),
    el('div', { class: 'git-file-list' }, note, ...rows)));
}

function renderCommitDiff() {
  const d = git.detail;
  if (!d) return;
  git.diffSeq++;
  if (!d.files.length) {
    unmountCards();
    diffBox.replaceChildren(el('div', { class: 'git-placeholder' }, 'No changes'));
    return;
  }
  diffBox.scrollTop = 0;
  showCards(d.files.map((f) => {
    const [dir, base] = splitPath(f.path);
    const part = d.diffs.get(f.path);
    return diffCard(f.path, [
      statusIcon(f.status),
      el('span', { class: 'git-card-name sel' }, base),
      el('span', { class: 'git-fdir sel' }, f.orig ? `${f.orig} →` : dir),
    ], (card) => {
      if (!part?.hunks.length) { card.append(noHunksNote(part)); return; }
      appendHunks(card, part.hunks.map((hunk) => ({ hunk, tag: '', actions: () => [] })));
    }, [part]);
  }));
}

// ---------- branches, tags, commits ----------

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
  if (git.remotes.length) items.push({ label: b.upstream ? `Push to ${b.upstream}` : 'Push to remote…', divider: !current, action: () => (current ? push() : pushBranch(b)) });
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

function commitMenu(e, c) {
  const on = git.branch || 'HEAD';
  const items = [
    { label: 'Check out this commit…', action: () => checkout(c.hash, { detached: true }) },
    { label: 'New branch here…', action: () => newBranch(c.hash) },
    { label: 'New tag here…', action: () => newTag(c.hash) },
  ];
  if (c.hash !== git.head) {
    items.push({ label: `Merge into ${on}`, divider: true, action: () => mergeBranch(c.hash) });
    items.push({ label: `Rebase ${on} onto this commit`, action: () => rebaseOnto(c.hash) });
    items.push({ label: `Cherry-pick onto ${on}`, action: async () => {
      if (await gitConfirm('Cherry-pick', `Apply the changes of ${short(c.hash)} "${c.subject}" as a new commit on ${on}?`, 'Cherry-pick', false)) gitOp('Cherry-pick', [['cherry-pick', ...(c.parents.length > 1 ? ['-m', '1'] : []), c.hash]]);
    } });
  }
  items.push({ label: 'Revert this commit…', divider: c.hash === git.head, action: async () => {
    if (await gitConfirm('Revert', `Create a new commit that undoes ${short(c.hash)} "${c.subject}"?`, 'Revert', false)) gitOp('Revert', [['revert', '--no-edit', ...(c.parents.length > 1 ? ['-m', '1'] : []), c.hash]]);
  } });
  if (git.branch && c.hash !== git.head) {
    items.push({ label: `Reset ${on} to this commit…`, action: () => resetTo(c) });
  }
  items.push({ label: 'Copy commit hash', divider: true, action: () => navigator.clipboard.writeText(c.hash) });
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
  resizer.addEventListener('mousedown', (e) => {
    e.preventDefault();
    resizer.classList.add('dragging');
    document.body.classList.add('resizing', 'resizing-x');
    const right = gitEl.getBoundingClientRect().right;
    const move = (ev) => {
      const narrow = gitEl.classList.contains('narrow');
      const max = right - $('#sidebar').getBoundingClientRect().right - (narrow ? 360 : 60); // keep the terminal usable
      setGitWidth(Math.max(narrow ? 300 : 460, Math.min(max, right - ev.clientX)));
    };
    const up = () => {
      resizer.classList.remove('dragging');
      document.body.classList.remove('resizing', 'resizing-x');
      localStorage.setItem(gitEl.classList.contains('narrow') ? 'gitListWidth' : 'gitWidth', parseInt(gitEl.style.width, 10));
      removeEventListener('mousemove', move);
      removeEventListener('mouseup', up);
      fitTerminals();
    };
    addEventListener('mousemove', move);
    addEventListener('mouseup', up);
  });
})();
dragSplit($('#git-files-split'), $('#git-files-col'), 'x', 'gitFilesWidth', 220, () => $('#git-detail').offsetWidth - 200);
