// Git worktrees, the way Zed's work (behaviour only; written for Crew). A repository can have more
// than one checkout on disk, so sessions can work side by side without touching each other's files.
//   - New worktrees start on a detached HEAD, so no branch ends up shared; a branch is picked after.
//   - They go in the worktree directory setting (default ../worktrees, then the project's name when
//     that's outside the project), under a typed name or a generated adjective-noun.
//   - Crew records the ones it created. Only those can be archived, and only while the folder is
//     still the one it created (git's metadata folder has the recorded creation time): when in
//     doubt, nothing is deleted.
//   - Archiving saves the staged and unstaged state as two detached commits kept by a ref, then
//     removes the folder. Restoring puts the folder, the branch and that state back.
//   - After creating one, the setup command from Settings runs in it, with CREW_WORKTREE_ROOT and
//     CREW_MAIN_GIT_WORKTREE set (for copying .env files, installing dependencies...).

const { ipcMain } = require('electron');
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');
const { spawn } = require('child_process');

const DEFAULT_DIR = '../worktrees';
const ARCHIVE_REF = 'refs/crew-archived-worktrees';
const WIP_ENV = { GIT_AUTHOR_NAME: 'Crew', GIT_AUTHOR_EMAIL: 'crew@localhost', GIT_COMMITTER_NAME: 'Crew', GIT_COMMITTER_EMAIL: 'crew@localhost' };

const ADJECTIVES = ['amber', 'ashen', 'azure', 'bold', 'brisk', 'calm', 'cedar', 'clear', 'cobalt', 'coral', 'crisp', 'dawn', 'deep', 'dusky', 'eager', 'early', 'fern', 'fleet', 'fresh', 'gentle', 'golden', 'green', 'hazel', 'honest', 'indigo', 'ivory', 'jade', 'keen', 'kind', 'lively', 'lucid', 'maple', 'mellow', 'misty', 'mossy', 'noble', 'olive', 'pale', 'pearl', 'plain', 'quick', 'quiet', 'rapid', 'rosy', 'rustic', 'sandy', 'silver', 'snowy', 'solid', 'spruce', 'steady', 'still', 'sunny', 'swift', 'tidy', 'topaz', 'vivid', 'warm', 'wild', 'windy'];
const NOUNS = ['anchor', 'aspen', 'badger', 'basin', 'beacon', 'birch', 'bison', 'brook', 'canyon', 'cedar', 'cliff', 'cloud', 'comet', 'crane', 'creek', 'delta', 'dune', 'eagle', 'ember', 'falcon', 'fjord', 'forest', 'fox', 'garden', 'glade', 'grove', 'harbor', 'hawk', 'heron', 'hill', 'island', 'lagoon', 'lake', 'lark', 'lynx', 'meadow', 'mesa', 'moon', 'oak', 'orchard', 'otter', 'owl', 'peak', 'pine', 'pond', 'prairie', 'quartz', 'raven', 'reef', 'ridge', 'river', 'robin', 'shore', 'sparrow', 'spring', 'stone', 'stream', 'summit', 'valley', 'willow'];

let deps = {}; // { runGit(cwd, args, input, env), getConfig, dataDir, send(channel, payload) }

// ---------- what Crew created and archived ----------

const registryFile = () => path.join(deps.dataDir(), 'worktrees.json');
let registry = null;
function reg() {
  if (registry) return registry;
  try { registry = JSON.parse(fs.readFileSync(registryFile(), 'utf8')); } catch { registry = {}; }
  registry.created ||= {}; // normalized path -> { path, main, name, gitDirBorn }
  registry.archived ||= []; // { id, path, main, name, branch, head, staged, unstaged, at }
  return registry;
}
function saveRegistry() {
  try { fs.writeFileSync(registryFile(), JSON.stringify(reg(), null, 1)); } catch { /* next change tries again */ }
}

// Paths compare in their long form: Windows may hand out C:\PROGRA~1 where git says C:\Program Files.
function normPath(p) {
  let full = path.resolve(p);
  try { full = fs.realpathSync.native(full); } catch { /* gone: as it was recorded */ }
  return full.replace(/[\\/]+$/, '').toLowerCase();
}

// ---------- reading a folder's state without starting git ----------

function dotGit(start) {
  let dir = path.resolve(start);
  for (;;) {
    const candidate = path.join(dir, '.git');
    let st = null;
    try { st = fs.statSync(candidate); } catch { /* keep looking */ }
    if (st) return { root: dir, dotGit: candidate, isFile: st.isFile() };
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

// The git folder of a linked worktree, from its ".git" file; null for anything else.
function linkedGitDir(root) {
  try {
    const m = /^gitdir:\s*(.+?)\s*$/m.exec(fs.readFileSync(path.join(root, '.git'), 'utf8'));
    return m ? path.resolve(root, m[1]) : null;
  } catch { return null; }
}

// The main checkout a linked worktree belongs to: <main>/.git/worktrees/<id> -> <main>.
function mainFromGitDir(gitDir) {
  const worktrees = path.dirname(gitDir);
  if (path.basename(worktrees) !== 'worktrees') return null;
  const common = path.dirname(worktrees);
  return path.basename(common) === '.git' ? path.dirname(common) : null; // a bare repository has no main folder
}

// For a session's folder: which repository checkout it's in, on which branch. Reads files only,
// so it can run often. The folder's main checkout also answers for a worktree that's archived.
function headOf(cwd) {
  const found = cwd && fs.existsSync(cwd) ? dotGit(cwd) : null;
  if (!found) return null;
  const linked = found.isFile ? linkedGitDir(found.root) : null;
  if (found.isFile && !linked) return null;
  const gitDir = linked || found.dotGit;
  let head = '';
  try { head = fs.readFileSync(path.join(gitDir, 'HEAD'), 'utf8').trim(); } catch { return null; }
  const m = /^ref: refs\/heads\/(.+)$/.exec(head);
  const main = linked ? mainFromGitDir(linked) : found.root;
  return {
    root: found.root,
    main: main || found.root,
    linked: !!linked,
    name: path.basename(found.root),
    branch: m ? m[1] : null,
    sha: m ? null : head.slice(0, 7),
  };
}

// The main checkout of a folder that is (or was, before it was archived) a linked worktree.
function mainOf(folder) {
  const rec = reg().created[normPath(folder)];
  if (rec) return rec.main;
  const archived = reg().archived.find((a) => normPath(a.path) === normPath(folder));
  if (archived) return archived.main;
  const gitDir = linkedGitDir(folder);
  return gitDir ? mainFromGitDir(gitDir) : null;
}

// For the sidebar: what to show on a session running in a linked worktree.
function sessionWorktree(folder) {
  const main = mainOf(folder);
  if (!main) return null;
  const archived = reg().archived.find((a) => normPath(a.path) === normPath(folder));
  return { main, path: folder, name: path.basename(folder), ...(archived ? { archivedId: archived.id } : {}) };
}

// ---------- listing ----------

function parseList(out) {
  const items = [];
  for (const block of out.replace(/\r\n/g, '\n').split('\n\n')) {
    const w = {};
    for (const line of block.split('\n')) {
      const i = line.indexOf(' ');
      const k = i < 0 ? line : line.slice(0, i);
      const v = i < 0 ? '' : line.slice(i + 1);
      if (k === 'worktree') w.path = path.normalize(v);
      else if (k === 'HEAD') w.sha = v;
      else if (k === 'branch') w.branch = v.replace(/^refs\/heads\//, '');
      else if (k === 'bare' || k === 'detached' || k === 'locked' || k === 'prunable') w[k] = true;
    }
    if (w.path) items.push(w);
  }
  items.forEach((w, i) => { w.isMain = i === 0; w.name = path.basename(w.path); });
  return items;
}

async function list(cwd) {
  const r = await deps.runGit(cwd, ['worktree', 'list', '--porcelain']);
  if (r.code !== 0) return { error: (r.stderr || r.stdout).trim(), worktrees: [] };
  const worktrees = parseList(r.stdout);
  const main = worktrees[0]?.path;
  for (const w of worktrees) w.crew = !!reg().created[normPath(w.path)];
  // origin/HEAD, when it's known, is what a new worktree is best based on.
  const dflt = await deps.runGit(main || cwd, ['symbolic-ref', '-q', '--short', 'refs/remotes/origin/HEAD']);
  const defaultBranch = dflt.code === 0 ? dflt.stdout.trim() : null;
  const archived = reg().archived.filter((a) => main && normPath(a.main) === normPath(main));
  return { worktrees, main, defaultBranch, archived };
}

// ---------- creating ----------

// Where new worktrees of this repository go: the setting, relative to the main checkout. When
// that's outside the project, the project's name is added so sibling repositories don't collide.
function baseDir(main) {
  const setting = String(deps.getConfig().worktreeDirectory || DEFAULT_DIR).trim().replace(/[\\/]+$/, '') || DEFAULT_DIR;
  const dir = path.resolve(main, setting);
  const rel = path.relative(main, dir);
  const inside = rel === '' || (!rel.startsWith('..') && !path.isAbsolute(rel));
  return inside ? dir : path.join(dir, path.basename(main));
}

function generateName(taken) {
  for (let i = 0; i < 12; i++) {
    const name = `${ADJECTIVES[crypto.randomInt(ADJECTIVES.length)]}-${NOUNS[crypto.randomInt(NOUNS.length)]}`;
    if (!taken.has(name)) return name;
  }
  return null;
}

const nameProblem = (name) => (/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name) && !name.includes('..') && !name.endsWith('.lock') && !name.endsWith('.')
  ? null
  : 'Use letters, numbers, dots, dashes and underscores in the name.');

// When the metadata folder git made for this worktree was created. It identifies this particular
// worktree: one removed and made again outside Crew has a different time.
function gitDirBorn(worktreePath) {
  const gitDir = linkedGitDir(worktreePath);
  if (!gitDir) return null;
  try { return fs.statSync(gitDir).birthtimeMs; } catch { return null; }
}

function remember(worktreePath, main) {
  reg().created[normPath(worktreePath)] = { path: worktreePath, main, name: path.basename(worktreePath), gitDirBorn: gitDirBorn(worktreePath) };
  saveRegistry();
}

// The setup command runs in the background; a failure doesn't undo the worktree.
function runSetup(worktreePath, main) {
  const command = String(deps.getConfig().worktreeSetup || '').trim();
  if (!command) return;
  const env = { ...process.env, CREW_WORKTREE_ROOT: worktreePath, CREW_MAIN_GIT_WORKTREE: main };
  const name = path.basename(worktreePath);
  deps.send('worktrees:setup', { name, state: 'running' });
  let child;
  try { child = spawn(command, { cwd: worktreePath, env, shell: true, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }); } catch (e) {
    deps.send('worktrees:setup', { name, state: 'failed', output: e.message });
    return;
  }
  let tail = '';
  const keep = (d) => { tail = (tail + d).slice(-2000); };
  child.stdout.on('data', keep);
  child.stderr.on('data', keep);
  child.on('error', (e) => deps.send('worktrees:setup', { name, state: 'failed', output: e.message }));
  child.on('close', (code) => deps.send('worktrees:setup', code === 0 ? { name, state: 'done' } : { name, state: 'failed', output: tail.trim() }));
}

// base: { remote, branch } for a remote branch (fetched first; the copy already here is used, with
// a warning, when that fails), or nothing for the current HEAD.
async function create(cwd, { name, base } = {}) {
  const { worktrees, main, error } = await list(cwd);
  if (error || !main) return { error: error || 'Not a Git repository.' };
  const dir = baseDir(main);
  const taken = new Set(worktrees.map((w) => w.name));
  try { for (const f of fs.readdirSync(dir)) taken.add(f); } catch { /* not made yet */ }
  const wanted = String(name || '').trim().replace(/\s+/g, '-');
  if (wanted) {
    const problem = nameProblem(wanted);
    if (problem) return { error: problem };
    if (taken.has(wanted)) return { error: `A worktree named "${wanted}" already exists.` };
  }
  const finalName = wanted || generateName(taken);
  if (!finalName) return { error: 'Couldn\'t find an unused name. Type one instead.' };
  const target = path.join(dir, finalName);

  let ref = 'HEAD';
  let warning = null;
  if (base?.remote && base?.branch) {
    ref = `${base.remote}/${base.branch}`;
    const f = await deps.runGit(main, ['fetch', base.remote, base.branch]);
    if (f.code !== 0) warning = `Couldn't fetch ${ref}, so this uses the copy already here. ${(f.stderr || '').trim()}`;
  }
  // HEAD of the folder the request came from, which may itself be a linked worktree.
  const from = ref === 'HEAD' ? cwd : main;
  try { fs.mkdirSync(path.dirname(target), { recursive: true }); } catch (e) { return { error: e.message }; }
  const r = await deps.runGit(from, ['worktree', 'add', '--detach', '--', target, ref]);
  if (r.code !== 0) return { error: (r.stderr || r.stdout).trim() };
  remember(target, main);
  runSetup(target, main);
  return { path: target, name: finalName, main, warning };
}

// ---------- removing ----------

// Removes the worktree's now empty parent folders, up to the worktree directory's own parent.
function tidyParents(worktreePath, main) {
  const stop = path.dirname(baseDir(main));
  for (let d = path.dirname(worktreePath); d !== stop && d.length > stop.length; d = path.dirname(d)) {
    try { fs.rmdirSync(d); } catch { return; } // not empty, or gone
  }
}

// { dirty: true } when git refuses because of changes in the folder; the caller asks, then forces.
async function remove(cwd, target, force) {
  const { worktrees, main, error } = await list(cwd);
  if (error) return { error };
  const w = worktrees.find((x) => normPath(x.path) === normPath(target));
  if (!w) return { error: 'That worktree no longer exists.' };
  if (w.isMain) return { error: 'The main checkout can\'t be removed.' };
  const r = await deps.runGit(main, ['worktree', 'remove', ...(force ? ['--force'] : []), '--', w.path]);
  if (r.code !== 0) {
    const msg = (r.stderr || r.stdout).trim();
    const low = msg.toLowerCase();
    if (low.includes('contains modified or untracked files') && low.includes('--force')) return { dirty: true, error: msg };
    return { error: msg };
  }
  delete reg().created[normPath(w.path)];
  saveRegistry();
  tidyParents(w.path, main);
  return { ok: true };
}

// ---------- archiving ----------

// Is this still the worktree Crew made? Anything uncertain means no.
function verifyCreated(worktreePath) {
  const rec = reg().created[normPath(worktreePath)];
  if (!rec) return 'Crew didn\'t create this worktree, so it won\'t archive it. You can still delete it.';
  const now = gitDirBorn(worktreePath);
  if (now == null || rec.gitDirBorn == null) return 'Couldn\'t confirm this is the worktree Crew created.';
  if (Math.abs(now - rec.gitDirBorn) > 1000) {
    delete reg().created[normPath(worktreePath)];
    saveRegistry();
    return 'This folder was recreated outside Crew, so it won\'t be archived.';
  }
  return null;
}

// Two commits that no branch points at: the index, then everything in the folder on top of it
// (through a temporary index, so the real one isn't touched).
async function snapshot(worktreePath, head) {
  const git = (args, input, env) => deps.runGit(worktreePath, args, input, env);
  const tree1 = await git(['write-tree']);
  if (tree1.code !== 0) return { error: (tree1.stderr || 'Couldn\'t read the staged changes. Finish any merge first.').trim() };
  const staged = await git(['commit-tree', tree1.stdout.trim(), '-p', head, '-m', 'WIP staged (Crew archive)'], undefined, WIP_ENV);
  if (staged.code !== 0) return { error: staged.stderr.trim() };
  const indexPath = path.join(os.tmpdir(), `crew-archive-${crypto.randomUUID()}.index`);
  const realIndex = (await git(['rev-parse', '--git-path', 'index'])).stdout.trim();
  try {
    fs.copyFileSync(path.resolve(worktreePath, realIndex), indexPath);
    const env = { GIT_INDEX_FILE: indexPath };
    const add = await git(['add', '-A'], undefined, env);
    if (add.code !== 0) return { error: add.stderr.trim() };
    const tree2 = await git(['write-tree'], undefined, env);
    if (tree2.code !== 0) return { error: tree2.stderr.trim() };
    const unstaged = await git(['commit-tree', tree2.stdout.trim(), '-p', staged.stdout.trim(), '-m', 'WIP unstaged (Crew archive)'], undefined, WIP_ENV);
    if (unstaged.code !== 0) return { error: unstaged.stderr.trim() };
    return { staged: staged.stdout.trim(), unstaged: unstaged.stdout.trim() };
  } catch (e) {
    return { error: e.message };
  } finally {
    try { fs.unlinkSync(indexPath); } catch { /* never made */ }
  }
}

async function archive(cwd, target) {
  const { worktrees, main, error } = await list(cwd);
  if (error) return { error };
  const w = worktrees.find((x) => normPath(x.path) === normPath(target));
  if (!w) return { error: 'That worktree no longer exists.' };
  if (w.isMain) return { error: 'The main checkout can\'t be archived.' };
  if (w.locked) return { error: 'This worktree is locked.' };
  const problem = verifyCreated(w.path);
  if (problem) return { error: problem };
  const saved = await snapshot(w.path, w.sha);
  if (saved.error) return saved;
  // Without this ref, git's cleanup would eventually collect the commits and restoring would fail.
  const id = crypto.randomUUID();
  const ref = await deps.runGit(main, ['update-ref', `${ARCHIVE_REF}/${id}`, saved.unstaged]);
  if (ref.code !== 0) return { error: ref.stderr.trim() };
  const r = await deps.runGit(main, ['worktree', 'remove', '--force', '--', w.path]);
  if (r.code !== 0) {
    await deps.runGit(main, ['update-ref', '-d', `${ARCHIVE_REF}/${id}`]);
    return { error: (r.stderr || r.stdout).trim() };
  }
  delete reg().created[normPath(w.path)];
  reg().archived.push({ id, path: w.path, main, name: w.name, branch: w.branch || null, head: w.sha, staged: saved.staged, unstaged: saved.unstaged, at: Date.now() });
  saveRegistry();
  tidyParents(w.path, main);
  return { ok: true, id };
}

async function restore(id) {
  const a = reg().archived.find((x) => x.id === id);
  if (!a) return { error: 'That archived worktree is gone.' };
  if (fs.existsSync(a.path)) return { error: `${a.path} already exists. Move it away to restore this worktree.` };
  await deps.runGit(a.main, ['worktree', 'prune']);
  try { fs.mkdirSync(path.dirname(a.path), { recursive: true }); } catch (e) { return { error: e.message }; }
  const add = await deps.runGit(a.main, ['worktree', 'add', '--detach', '--', a.path, a.head]);
  if (add.code !== 0) return { error: (add.stderr || add.stdout).trim() };

  // Back on its branch, unless that moved on since: the saved state belongs on the old commit.
  let note = null;
  if (a.branch) {
    const at = await deps.runGit(a.main, ['rev-parse', '-q', '--verify', `refs/heads/${a.branch}`]);
    if (at.code !== 0) {
      const b = await deps.runGit(a.path, ['switch', '-c', a.branch]);
      if (b.code !== 0) note = `Couldn't recreate the branch ${a.branch}, so this stays on a detached HEAD.`;
    } else if (at.stdout.trim() === a.head) {
      const b = await deps.runGit(a.path, ['switch', a.branch]);
      if (b.code !== 0) note = `${a.branch} is checked out somewhere else, so this stays on a detached HEAD.`;
    } else note = `${a.branch} has moved since this was archived, so this stays on a detached HEAD at the saved commit.`;
  }
  // The folder's contents (deletions too), then the index, leaving the folder alone.
  const files = await deps.runGit(a.path, ['read-tree', '--reset', '-u', a.unstaged]);
  const index = files.code === 0 ? await deps.runGit(a.path, ['read-tree', a.staged]) : files;
  if (index.code !== 0) return { error: `The worktree is back, but its saved changes couldn't be restored: ${index.stderr.trim()}`, path: a.path };

  await deps.runGit(a.main, ['update-ref', '-d', `${ARCHIVE_REF}/${id}`]);
  reg().archived = reg().archived.filter((x) => x.id !== id);
  remember(a.path, a.main);
  return { ok: true, path: a.path, name: a.name, main: a.main, note };
}

// Removes an archived worktree for good, saved changes included.
async function forget(id) {
  const a = reg().archived.find((x) => x.id === id);
  if (!a) return { ok: true };
  await deps.runGit(a.main, ['update-ref', '-d', `${ARCHIVE_REF}/${id}`]);
  reg().archived = reg().archived.filter((x) => x.id !== id);
  saveRegistry();
  return { ok: true };
}

function init(d) {
  deps = d;
  const safe = (fn) => async (_e, ...a) => { try { return await fn(...a); } catch (e) { return { error: e.message }; } };
  ipcMain.handle('worktrees:head', (_e, cwd) => { try { return headOf(cwd); } catch { return null; } });
  ipcMain.handle('worktrees:list', safe(list));
  ipcMain.handle('worktrees:create', safe(create));
  ipcMain.handle('worktrees:remove', safe(remove));
  ipcMain.handle('worktrees:archive', safe(archive));
  ipcMain.handle('worktrees:restore', safe(restore));
  ipcMain.handle('worktrees:forget', safe(forget));
}

module.exports = { init, sessionWorktree, mainOf, list, create, remove, archive, restore, forget, headOf, baseDir };
