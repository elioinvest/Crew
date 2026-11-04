/* global api, $, el, state, norm, gitDialog, git, gitReload, gitOp, gitBar, forgetRepoRoot, closeGit, openGit, activate, newSession, openSession, findProject, findSession, inputEl, branchMenu, remoteBranchMenu, tagMenu, jumpTo, toast, activeEntry, isHostWorkspace, providerOf, fmtDate, renderSidebar, refresh */
// The worktree picker on top of the Git panel and the branch pickers, there and next to the project
// chip under the prompt box. Pick or type to create a worktree: it starts on a detached
// HEAD, and the branch picker beside it then creates or checks out the branch to use there. A
// session belongs to one folder, so "switching" shows that worktree's session or starts one.
// worktrees.js in the main process does the work.

// ---------- a searchable list ----------

const pickerEl = el('div', { id: 'picker', class: 'hidden' });
document.body.append(pickerEl);
let pickerClose = null;

// rows(query): { section } | { note } | { label, sub, right, on, disabled, hint, action, buttons, menu }.
// A row's action closes the list; its buttons do unless `keep`. Async rows() is allowed.
function openListPicker({ anchor, placeholder, rows, above = false, footer }) {
  if (pickerClose) pickerClose();
  const input = el('input', { type: 'text', placeholder, spellcheck: 'false' });
  const list = el('div', { class: 'picker-list' });
  pickerEl.replaceChildren(input, list, ...(footer ? [el('div', { class: 'picker-foot' }, footer)] : []));
  let items = [];
  let index = 0;
  let seq = 0;

  const close = () => {
    pickerEl.classList.add('hidden');
    document.removeEventListener('mousedown', outside, true);
    pickerClose = null;
  };
  const outside = (e) => { if (!pickerEl.contains(e.target) && !document.querySelector('#menu')?.contains(e.target)) close(); };

  const mark = () => {
    list.querySelectorAll('.picker-row').forEach((r) => r.classList.toggle('active', +r.dataset.i === index));
    list.querySelector('.picker-row.active')?.scrollIntoView({ block: 'nearest' });
  };
  const choose = (i) => {
    const it = items[i];
    if (!it || it.disabled || !it.action) return;
    close();
    it.action();
  };
  const render = (built) => {
    items = built;
    const out = [];
    const selectable = items.map((it, i) => (it.action && !it.disabled ? i : -1)).filter((i) => i >= 0);
    if (!selectable.includes(index)) index = selectable[0] ?? -1;
    items.forEach((it, i) => {
      if (it.section) { out.push(el('div', { class: 'picker-section' }, it.section)); return; }
      if (it.note) { out.push(el('div', { class: 'picker-note' }, it.note)); return; }
      const row = el('div', {
        class: `picker-row${it.disabled ? ' disabled' : ''}${it.on ? ' on' : ''}${it.pending ? ' pending' : ''}${it.ours ? ' ours' : ''}`, // ours: Crew's words, translated (i18n.js)
        title: it.hint || it.title || '',
        onmousemove: () => { if (index !== i && !it.disabled) { index = i; mark(); } },
        onclick: () => choose(i),
      },
      el('div', { class: 'picker-main' },
        el('div', { class: 'picker-label' }, it.label),
        it.sub ? el('div', { class: 'picker-sub' }, it.sub) : null),
      it.right ? el('span', { class: 'picker-right' }, it.right) : null,
      ...(it.buttons || []).map((b) => el('button', {
        class: `picker-btn${b.danger ? ' danger' : ''}`,
        title: b.title || '',
        onclick: (e) => {
          e.stopPropagation();
          if (!b.keep) close();
          b.action(e);
        },
      }, b.label)));
      row.dataset.i = i;
      if (it.menu) row.addEventListener('contextmenu', (e) => { e.preventDefault(); it.menu(e, close); });
      out.push(row);
    });
    list.replaceChildren(...out);
    mark();
  };
  const update = async () => {
    const mine = ++seq;
    const built = await rows(input.value.trim().toLowerCase(), input.value.trim());
    if (mine === seq && pickerClose === close) render(built);
  };
  input.addEventListener('input', update);
  input.addEventListener('keydown', (e) => {
    const step = (d) => {
      const selectable = items.map((it, i) => (it.action && !it.disabled ? i : -1)).filter((i) => i >= 0);
      if (!selectable.length) return;
      index = selectable[(selectable.indexOf(index) + d + selectable.length) % selectable.length];
      mark();
    };
    if (e.key === 'ArrowDown') { e.preventDefault(); step(1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); step(-1); }
    else if (e.key === 'Enter') { e.preventDefault(); choose(index); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); }
  });

  pickerClose = close;
  pickerEl.classList.remove('hidden');
  const r = anchor.getBoundingClientRect();
  const width = Math.min(480, innerWidth - 16);
  pickerEl.style.width = `${width}px`;
  pickerEl.style.left = `${Math.max(8, Math.min(r.left, innerWidth - width - 8))}px`;
  // Over the chips under the prompt box, under the buttons elsewhere.
  if (above) { pickerEl.style.top = ''; pickerEl.style.bottom = `${innerHeight - r.top + 6}px`; } else { pickerEl.style.bottom = ''; pickerEl.style.top = `${r.bottom + 6}px`; }
  list.style.maxHeight = `${Math.min(420, (above ? r.top : innerHeight - r.bottom) - 90)}px`;
  document.addEventListener('mousedown', outside, true);
  input.focus();
  update();
  return { refresh: update, close };
}

// ---------- worktrees ----------

const wtSub = (w) => [w.branch || 'detached', w.sha ? w.sha.slice(0, 7) : null, w.path].filter(Boolean).join(' • ');
const sessionsIn = (p) => [...state.terms.entries()].filter(([, e]) => !e.exited && e.cwd && norm(e.cwd) === norm(p));

// Shows the worktree: its running session, its latest session, or a new one with the same agent.
async function switchToWorktree(w, main, active) {
  const running = sessionsIn(w.path)[0];
  if (running) { activate(running[0]); return; }
  const projectPath = active?.projectPath || main;
  const project = findProject(projectPath);
  const past = project?.sessions.find((s) => s.cwd && norm(s.cwd) === norm(w.path));
  if (past) {
    openSession(project.path, past.id, true, { agent: past.agent || 'claude', cwd: past.cwd });
    return;
  }
  const agent = active && (active.agent === 'claude' || active.agent === 'codex') ? active.agent : undefined;
  newSession(projectPath, agent, active?.provider || null, w.path);
}

async function createWorktree(cwd, { name, base } = {}, active) {
  toast(name ? `Creating worktree ${name}…` : 'Creating worktree…');
  const r = await api.worktreesCreate(cwd, { name, base });
  if (r.error) { worktreeError(r.error); return; }
  if (r.warning) worktreeError(r.warning);
  else toast(`Worktree ${r.name} created`);
  forgetRepoRoot(r.path);
  await switchToWorktree({ path: r.path }, r.main, active);
  renderChipsSoon();
}

function worktreeError(text) {
  if (git.open) gitBar(text, 'error');
  else toast(text, 'error');
}

// Deleting: refused while a session runs there; git refuses a folder with changes, and then asks.
async function deleteWorktree(cwd, w, picker, force = false) {
  const busy = sessionsIn(w.path);
  if (busy.length) { worktreeError(`Close the session running in ${w.name} first.`); return; }
  w.deleting = true;
  picker?.refresh();
  const r = await api.worktreesRemove(cwd, w.path, force);
  w.deleting = false;
  if (r.dirty) {
    const ok = await gitDialog({ title: 'Delete worktree', text: `Worktree "${w.name}" contains modified or untracked files. Force delete it?`, ok: 'Force delete', danger: true });
    if (ok) await deleteWorktree(cwd, w, picker, true);
    else picker?.refresh();
    return;
  }
  if (r.error) { worktreeError(r.error); picker?.refresh(); return; }
  forgetRepoRoot(w.path);
  toast(`Worktree ${w.name} deleted`);
  afterWorktreeGone(w.path, r.main);
  picker?.refresh();
}

async function archiveWorktree(cwd, w) {
  const busy = sessionsIn(w.path);
  if (busy.length) { worktreeError(`Close the session running in ${w.name} first.`); return; }
  const ok = await gitDialog({
    title: 'Archive worktree',
    text: `Save "${w.name}" and remove its folder? Staged, unstaged and untracked changes are kept and come back when you restore it. Files Git ignores (such as node_modules or .env) are deleted with the folder.`,
    ok: 'Archive',
  });
  if (!ok) return;
  const r = await api.worktreesArchive(cwd, w.path);
  if (r.error) { worktreeError(r.error); return; }
  forgetRepoRoot(w.path);
  toast(`Worktree ${w.name} archived`);
  afterWorktreeGone(w.path);
  refresh();
}

// The Git panel moves off a folder that's gone.
function afterWorktreeGone(p) {
  if (git.open && norm(git.root) === norm(p)) {
    const active = activeEntry();
    if (active && norm(active.cwd) !== norm(p)) openGit(active.cwd);
    else closeGit();
  }
  renderChipsSoon();
}

// Restores an archived worktree, returning its path, or null with the problem shown.
async function restoreWorktree(id) {
  toast('Restoring worktree…');
  const r = await api.worktreesRestore(id);
  if (r.error) { worktreeError(r.error); return null; }
  if (r.note) worktreeError(r.note);
  else toast(`Worktree ${r.name} restored`);
  forgetRepoRoot(r.path);
  await refresh();
  return r.path;
}

async function openWorktreePicker(anchor, cwd, { above = false } = {}) {
  const info = await api.worktreesList(cwd);
  if (info.error) { worktreeError(info.error); return; }
  const active = activeEntry();
  const head = await api.worktreeHead(cwd);
  const current = head?.branch || 'HEAD';
  const defaultBranch = info.defaultBranch; // e.g. origin/main
  const slash = defaultBranch?.indexOf('/') ?? -1;
  const remoteBase = defaultBranch && slash > 0 ? { remote: defaultBranch.slice(0, slash), branch: defaultBranch.slice(slash + 1) } : null;
  const offerRemote = !!remoteBase;
  const offerCurrent = !remoteBase || head?.branch !== remoteBase.branch;
  const baseLabel = offerRemote ? defaultBranch : current;
  const base = offerRemote ? remoteBase : null;

  const row = (w) => ({
    label: w.name,
    sub: wtSub(w),
    on: norm(w.path) === norm(cwd),
    pending: w.deleting,
    disabled: w.deleting || w.prunable,
    hint: w.prunable ? 'This folder is gone. Run "git worktree prune" to clear it.' : w.path,
    right: w.deleting ? 'Deleting…' : w.isMain ? 'Main checkout' : undefined,
    action: () => switchToWorktree(w, info.main, active),
    buttons: w.deleting ? [] : [
      { label: 'Folder', title: 'Open the folder', action: () => api.revealProject(w.path) },
      ...(w.isMain ? [] : [
        ...(w.crew ? [{ label: 'Archive', title: 'Save its changes and remove the folder; restore it later', action: () => archiveWorktree(cwd, w) }] : []),
        { label: 'Delete', danger: true, keep: true, title: 'Delete this worktree. Hold Alt to force delete.', action: (e) => deleteWorktree(cwd, w, picker, e.altKey) },
      ]),
    ],
  });

  const picker = openListPicker({
    anchor,
    above,
    placeholder: 'Select or type to create a worktree…',
    rows: (q, raw) => {
      const out = [];
      const matches = (s) => !q || s.toLowerCase().includes(q);
      const named = raw && !info.worktrees.some((w) => w.name.toLowerCase() === q);
      if (named) {
        out.push({ ours: true, label: `Create "${raw}" based on ${baseLabel}`, action: () => createWorktree(cwd, { name: raw, base }, active) });
        if (offerRemote && offerCurrent) out.push({ ours: true, label: `Create "${raw}" based on ${current}`, action: () => createWorktree(cwd, { name: raw }, active) });
      } else if (!raw) {
        if (offerRemote) out.push({ ours: true, label: `Create new worktree based on ${defaultBranch}`, action: () => createWorktree(cwd, { base: remoteBase }, active) });
        if (offerCurrent) out.push({ ours: true, label: `Create new worktree based on ${current}`, action: () => createWorktree(cwd, {}, active) });
      }
      const visible = info.worktrees.filter((w) => matches(w.name) || matches(w.branch || '') || matches(w.path));
      const open = visible.filter((w) => sessionsIn(w.path).length || norm(w.path) === norm(cwd));
      const others = visible.filter((w) => !open.includes(w));
      if (open.length) out.push({ section: 'Open in this window' }, ...open.map(row));
      if (others.length) out.push({ section: 'Worktrees' }, ...others.map(row));
      const archived = (info.archived || []).filter((a) => matches(a.name) || matches(a.branch || ''));
      if (archived.length) {
        out.push({ section: 'Archived' }, ...archived.map((a) => ({
          label: a.name,
          sub: [a.branch || 'detached', fmtDate(a.at / 1000)].join(' • '),
          hint: a.path,
          action: async () => {
            const p = await restoreWorktree(a.id);
            if (p) await switchToWorktree({ path: p }, a.main, active);
          },
          buttons: [{
            label: 'Delete',
            danger: true,
            title: 'Delete this archive and its saved changes for good',
            action: async () => {
              const ok = await gitDialog({ title: 'Delete archive', text: `Delete the archive of "${a.name}"? Its saved changes are lost.`, ok: 'Delete', danger: true });
              if (ok) { await api.worktreesForget(a.id); refresh(); }
            },
          }],
        })));
      }
      if (!out.length) out.push({ note: 'No matches' });
      return out;
    },
  });
}

// ---------- branches ----------

async function openBranchPicker(anchor, cwd, { above = false } = {}) {
  const inPanel = git.open && norm(git.root) === norm(cwd);
  const runHere = (args) => api.git(cwd, args);
  const [refs, info, head] = await Promise.all([
    runHere(['for-each-ref', '--sort=-committerdate', '--format=%(refname)%00%(refname:short)%00%(objectname)%00%(upstream:short)%00%(upstream:track)%00%(committerdate:relative)', 'refs/heads', 'refs/remotes', 'refs/tags']),
    api.worktreesList(cwd),
    api.worktreeHead(cwd),
  ]);
  const locals = [];
  const remotes = [];
  const tags = [];
  for (const line of refs.stdout.split('\n')) {
    if (!line) continue;
    const [full, name, hash, upstream, track, when] = line.split('\0');
    if (full.startsWith('refs/heads/')) locals.push({ name, hash, upstream, track, when });
    else if (full.startsWith('refs/remotes/')) { if (!full.endsWith('/HEAD')) remotes.push({ name, hash, when, remote: name.split('/')[0] }); } else tags.push({ name, hash, when });
  }
  // A branch can only be checked out in one worktree at a time.
  const usedIn = new Map((info.worktrees || []).filter((w) => w.branch && norm(w.path) !== norm(cwd)).map((w) => [w.branch, w]));

  const after = async () => {
    if (git.open && norm(git.root) === norm(cwd)) await gitReload(true);
    renderChipsSoon();
  };
  const act = async (label, args) => {
    if (inPanel) { await gitOp(label, [args]); renderChipsSoon(); return; }
    const r = await runHere(args);
    if (r.code !== 0) toast(`${label} failed. ${(r.stderr || r.stdout).trim()}`, 'error');
    await after();
  };
  const deleteBranch = async (b, close) => {
    close?.();
    const v = await gitDialog({ title: 'Delete branch', text: `Delete the local branch ${b.name}?`, ok: 'Delete', danger: true });
    if (!v) return;
    const r = await runHere(['branch', '-d', b.name]);
    if (r.code === 0) { await after(); return; }
    const force = await gitDialog({ title: 'Delete branch', text: `${b.name} has commits that aren't merged anywhere. Delete it anyway?`, ok: 'Delete anyway', danger: true });
    if (force) await act(`Delete ${b.name}`, ['branch', '-D', b.name]);
  };

  openListPicker({
    anchor,
    above,
    placeholder: 'Select a branch or type to create one…',
    rows: (q, raw) => {
      const out = [];
      const matches = (s) => !q || s.toLowerCase().includes(q);
      if (raw && !locals.some((b) => b.name.toLowerCase() === q)) {
        const name = raw.replace(/\s+/g, '-');
        out.push({ ours: true, label: `Create branch "${name}"`, sub: `from ${head?.branch || head?.sha || 'HEAD'}`, action: () => act(`Create branch ${name}`, ['switch', '-c', name]) });
      }
      const local = locals.filter((b) => matches(b.name));
      if (local.length) {
        out.push({ section: 'Branches' }, ...local.map((b) => {
          const current = b.name === head?.branch;
          const other = usedIn.get(b.name);
          return {
            label: b.name,
            sub: [b.upstream && `→ ${b.upstream}${b.track ? ` ${b.track}` : ''}`, b.when, other && `checked out in ${other.name}`].filter(Boolean).join(' • '),
            on: current,
            disabled: !!other,
            hint: other ? `${b.name} is already checked out in ${other.path}.` : '',
            action: current ? undefined : () => act(`Check out ${b.name}`, ['switch', b.name]),
            buttons: current || other ? [] : [{ label: 'Delete', danger: true, action: () => deleteBranch(b) }],
            menu: inPanel ? (e) => { const g = git.locals.find((x) => x.name === b.name); if (g) branchMenu(e, g); } : undefined,
          };
        }));
      }
      const remote = remotes.filter((b) => matches(b.name));
      if (remote.length) {
        out.push({ section: 'Remote branches' }, ...remote.slice(0, 60).map((b) => {
          const local2 = b.name.slice(b.remote.length + 1);
          return {
            label: b.name,
            sub: b.when,
            action: () => (locals.some((l) => l.name === local2) ? act(`Check out ${local2}`, ['switch', local2]) : act(`Check out ${local2}`, ['switch', '-c', local2, '--track', b.name])),
            menu: inPanel ? (e) => { const g = git.remoteBranches.find((x) => x.name === b.name); if (g) remoteBranchMenu(e, g); } : undefined,
          };
        }));
      }
      const tagged = q ? tags.filter((t) => matches(t.name)) : [];
      if (inPanel && tagged.length) {
        out.push({ section: 'Tags' }, ...tagged.slice(0, 30).map((t) => ({
          label: t.name,
          sub: t.when,
          action: () => jumpTo(t.hash),
          menu: (e) => { const g = git.tags.find((x) => x.name === t.name); if (g) tagMenu(e, g); },
        })));
      }
      if (!out.length) out.push({ note: 'No matches' });
      return out;
    },
  });
}

// ---------- the chips under the prompt box ----------

const GIT_ICONS = {
  branch: '<svg viewBox="0 0 24 24"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 4-6 3-12 7"/></svg>',
};
const chipBranch = $('#chip-branch');
let chipSeq = 0;

async function refreshGitChips() {
  const entry = activeEntry();
  const eligible = entry && entry.agent !== 'file' && entry.agent !== 'ssh' && !isHostWorkspace(entry.projectPath);
  const mine = ++chipSeq;
  const head = eligible ? await api.worktreeHead(entry.cwd) : null;
  if (mine !== chipSeq) return;
  chipBranch.classList.toggle('hidden', !head);
  if (!head) return;
  chipBranch.replaceChildren(el('span', { class: 'chip-icon', html: GIT_ICONS.branch }), head.branch || `detached ${head.sha}`);
  chipBranch.title = head.branch ? 'Switch or create a branch' : 'HEAD is detached: pick a branch to work on';
  chipBranch.classList.toggle('detached', !head.branch);
}

let chipTimer = null;
function renderChipsSoon() {
  clearTimeout(chipTimer);
  chipTimer = setTimeout(refreshGitChips, 50);
}

chipBranch.addEventListener('click', () => { const e = activeEntry(); if (e) openBranchPicker(chipBranch, e.cwd, { above: true }); });
// An agent can switch branches on its own. Reading the file is cheap.
setInterval(() => { if (!document.hidden) refreshGitChips(); }, 5000);

api.onWorktreeSetup(({ name, state: s, output }) => {
  if (s === 'running') toast(`Running the setup command in ${name}…`);
  else if (s === 'done') toast(`Setup finished in ${name}`);
  else worktreeError(`The setup command failed in ${name}.${output ? ` ${output}` : ''}`);
});
