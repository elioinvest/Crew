//! The Git panel, as Zed draws it: Changes and History tabs, one column of files in Staged and Unstaged
//! sections with checkboxes, and at the bottom the branch picker, one Fetch / Pull / Push button, the
//! commit editor and the last commit, which can be undone. View Diff (or clicking a file) opens the
//! Project Diff beside the list: hunks stage, unstage and restore, conflicts resolve with a click. It
//! follows the active session's folder, so a session in a linked worktree shows that worktree. Every
//! action runs the real git CLI. This file holds the state and actions; git_view.rs draws it.

use crew_core::git::{self, Change, Commit, CommitDetail, ConflictPart, Entry, FileDiff, Hunk, LastCommit, LocalBranch, Output, RefKind, Refs, RemoteBranch, Status, Tag};
use gpui::{Point, ScrollStrategy, UniformListScrollHandle};

use super::forms::Spec;
use super::*;

const LOG_PAGE: usize = 1000;
const MAX_UNTRACKED_DIFFS: usize = 25;
const DIFF_LINES_SHOWN: usize = 4000;
const REFS_EVERY: u64 = 5;

pub(super) type Step = (Vec<String>, Option<String>);

pub(super) fn step(args: &[&str]) -> Step {
    (args.iter().map(|a| (*a).to_string()).collect(), None)
}

fn chunked(base: &[&str], paths: &[String]) -> Vec<Step> {
    paths
        .chunks(200)
        .map(|c| {
            let mut args: Vec<String> = base.iter().map(|a| (*a).to_string()).collect();
            args.push("--".into());
            args.extend(c.iter().cloned());
            (args, None)
        })
        .collect()
}

pub(super) fn short(h: &str) -> String {
    h.chars().take(7).collect()
}

pub(super) fn split_path(p: &str) -> (&str, &str) {
    p.rsplit_once('/').unwrap_or(("", p))
}

pub(super) fn fmt_date(sec: i64) -> String {
    chrono::TimeZone::timestamp_opt(&chrono::Local, sec, 0).single().map_or(String::new(), |d| d.format("%Y-%m-%d %H:%M").to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GitView {
    Changes,
    History,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BarKind {
    Busy,
    Ok,
    Error,
}

#[derive(Clone, Default)]
pub(super) struct FileDiffs {
    pub(super) staged: Option<FileDiff>,
    pub(super) unstaged: Option<FileDiff>,
}

/// One line of the diff list. Every row is the same height, so only those on screen are ever built,
/// however large the diff.
#[derive(Clone)]
pub(super) enum DiffRow {
    File { key: String, status: char, orig: Option<String>, folded: bool, entry: Option<Entry> },
    Hunk { title: String, tag: Option<&'static str>, actions: HunkActions },
    Line(git::DiffLine),
    Sides(Option<git::DiffLine>, Option<git::DiffLine>),
    Note(String),
    ShowAll,
    Conflict { path: String, index: usize, ours: String, theirs: String },
    ConflictSide { label: String, text: String, ours: bool },
    ConflictContext(String),
    ConflictDone { path: String },
    Placeholder(String),
}

#[derive(Clone)]
pub(super) enum HunkActions {
    None,
    Stage { header: Rc<Vec<String>>, hunk: Rc<Hunk> },
    Unstage { header: Rc<Vec<String>>, hunk: Rc<Hunk> },
}

pub(super) struct GitPanel {
    pub(super) root: PathBuf,
    pub(super) git_dir: Option<PathBuf>,
    pub(super) view: GitView,
    pub(super) by_tracked: bool,
    pub(super) diff_open: bool,
    pub(super) tree: bool,
    pub(super) split_diff: bool,
    pub(super) status: Status,
    pub(super) numstat_staged: HashMap<String, (u64, u64)>,
    pub(super) numstat_unstaged: HashMap<String, (u64, u64)>,
    pub(super) refs: Refs,
    pub(super) commits: Rc<Vec<Commit>>,
    pub(super) lanes: usize,
    log_stale: bool,
    limit: usize,
    pub(super) diffs: HashMap<String, FileDiffs>,
    conflicts: HashMap<String, String>,
    pub(super) diff_rows: Rc<Vec<DiffRow>>,
    pub(super) diff_scroll: UniformListScrollHandle,
    pub(super) log_scroll: UniformListScrollHandle,
    pub(super) folded_sections: HashSet<String>,
    pub(super) folded_dirs: HashSet<String>,
    folded_files: HashSet<String>,
    pub(super) amend: bool,
    pub(super) signoff: bool,
    pub(super) no_verify: bool,
    pub(super) push_after: bool,
    pub(super) commit_folded: bool,
    pub(super) message: Entity<TextareaState>,
    pub(super) selected: Option<String>,
    /// History shows the selected commit's own page instead of the list.
    pub(super) commit_open: bool,
    pub(super) file_filter: Option<String>,
    pub(super) picked: HashSet<String>,
    anchor: Option<String>,
    full_diff: bool,
    pub(super) detail: Option<CommitDetail>,
    detail_cache: HashMap<String, CommitDetail>,
    ref_list: String,
    ref_sig: Option<String>,
    status_sig: Option<String>,
    pub(super) busy: bool,
    reloading: bool,
    force_again: bool,
    polls: u64,
    pub(super) loading: Option<String>,
    pub(super) bar: Option<(String, BarKind, u64)>,
    pub(super) list_width: f32,
    pub(super) wide_width: f32,
    pub(super) files_col_width: f32,
    _subscription: gpui::Subscription,
}

struct Reloaded {
    root: PathBuf,
    status: Status,
    ref_list: Option<String>,
    ref_sig: String,
    status_sig: String,
    changed: bool,
    numstat: Option<(HashMap<String, (u64, u64)>, HashMap<String, (u64, u64)>)>,
    diffs: Option<(HashMap<String, FileDiffs>, HashMap<String, String>)>,
    refs: Option<Refs>,
    log: Option<(Vec<Commit>, usize)>,
}

fn load_diffs(root: &Path, status: &Status) -> (HashMap<String, FileDiffs>, HashMap<String, String>) {
    let unstaged = git::diffs_by_file(&git::run(root, &["diff"]).stdout);
    let staged = git::diffs_by_file(&git::run(root, &["diff", "--cached", "-M"]).stdout);
    let mut diffs: HashMap<String, FileDiffs> = HashMap::new();
    for (p, d) in unstaged {
        diffs.entry(p).or_default().unstaged = Some(d);
    }
    for (p, d) in staged {
        diffs.entry(p).or_default().staged = Some(d);
    }
    for f in status.unstaged.iter().filter(|f| f.status == '?').take(MAX_UNTRACKED_DIFFS) {
        let r = git::run(root, &["diff", "--no-index", "--", "/dev/null", &f.path]);
        if r.code == 0 || r.code == 1 {
            diffs.entry(f.path.clone()).or_default().unstaged = Some(git::parse_diff(&r.stdout));
        }
    }
    let conflicts = status.unstaged.iter().filter(|f| f.status == 'U').filter_map(|f| std::fs::read_to_string(root.join(&f.path)).ok().map(|t| (f.path.clone(), t))).collect();
    (diffs, conflicts)
}

fn load_log(root: &Path, head: bool, locals: &[LocalBranch], limit: usize, filter: Option<&str>) -> (Vec<Commit>, usize) {
    let n = format!("-n{limit}");
    let mut args: Vec<&str> = vec!["log"];
    match filter {
        Some(path) => args.extend(["--follow", &n, git::LOG_FORMAT, "--", path]),
        None => {
            args.extend(["--date-order", &n, git::LOG_FORMAT, "--branches", "--remotes", "--tags"]);
            if head {
                args.push("HEAD");
            }
        }
    }
    let r = git::run(root, &args);
    let names: HashSet<String> = locals.iter().map(|b| b.name.clone()).collect();
    let mut commits = if r.ok() { git::parse_log(&r.stdout, &names) } else { Vec::new() };
    let lanes = git::build_graph(&mut commits);
    (commits, lanes)
}

impl CrewApp {
    fn git_setting(&self, key: &str) -> bool {
        self.config.bool(key)
    }

    pub(super) fn git_entries(&self) -> Vec<Entry> {
        self.git.as_ref().map_or(Vec::new(), |g| git::entries(&g.status))
    }

    pub(super) fn toggle_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.is_some() {
            self.git = None;
            cx.notify();
            return;
        }
        if let Some(folder) = self.active().map(|o| o.cwd.clone()) {
            self.open_git(folder, window, cx);
        }
    }

    pub(super) fn open_git(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let lookup = folder.clone();
        let find = cx.background_spawn(async move { (git::root_of(&lookup), lookup) });
        cx.spawn_in(window, async move |this, cx| {
            let (root, folder) = find.await;
            let _ = this.update_in(cx, |app, window, cx| match root {
                Some(root) => app.show_git(root, window, cx),
                None => {
                    let text = if crate::i18n::zh() { format!("{} 还没有由 Git 管理。要在这里用 git init 创建仓库吗？", folder.display()) } else { format!("{} isn't managed by Git yet. Create a repository here with git init?", folder.display()) };
                    app.ask(
                        t("Not a Git repository"),
                        text,
                        t("Initialize"),
                        false,
                        move |app, window, cx| {
                            let r = git::run(&folder, &["init"]);
                            if r.ok() {
                                app.open_git(folder.clone(), window, cx);
                            } else {
                                app.toast(format!("git init failed. {}", r.message()), overlays::ToastKind::Error, cx);
                            }
                        },
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn show_git(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.as_ref().is_some_and(|g| folder_text(&g.root) == folder_text(&root)) {
            self.git_reload(true, cx);
            return;
        }
        self.files = None; // they share the right side
        let git_dir = {
            let gd = git::run(&root, &["rev-parse", "--absolute-git-dir"]);
            gd.ok().then(|| PathBuf::from(gd.stdout.trim().replace('/', "\\")))
        };
        let message = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8).placeholder(t("Enter commit message")));
        let subscription = cx.subscribe_in(&message, window, |app, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { secondary: true, .. }) {
                app.do_commit(cx);
            }
        });
        let width = |key: &str, default: f32, config: &Config| config.f32(key, default);
        let history = self.config.str("gitView") == "history";
        self.git = Some(GitPanel {
            root,
            git_dir,
            view: if history { GitView::History } else { GitView::Changes },
            by_tracked: self.config.str("gitGroupBy") == "tracked",
            diff_open: self.git_setting("gitDiffOpen"),
            tree: self.git_setting("gitTree"),
            split_diff: self.config.str("gitDiffStyle") == "split",
            status: Status::default(),
            numstat_staged: HashMap::new(),
            numstat_unstaged: HashMap::new(),
            refs: Refs::default(),
            commits: Rc::new(Vec::new()),
            lanes: 1,
            log_stale: true,
            limit: LOG_PAGE,
            diffs: HashMap::new(),
            conflicts: HashMap::new(),
            diff_rows: Rc::new(Vec::new()),
            diff_scroll: UniformListScrollHandle::new(),
            log_scroll: UniformListScrollHandle::new(),
            folded_sections: HashSet::new(),
            folded_dirs: HashSet::new(),
            folded_files: HashSet::new(),
            amend: false,
            signoff: false,
            no_verify: false,
            push_after: false,
            commit_folded: self.git_setting("gitCommitFolded"),
            message,
            selected: None,
            commit_open: false,
            file_filter: None,
            picked: HashSet::new(),
            anchor: None,
            full_diff: false,
            detail: None,
            detail_cache: HashMap::new(),
            ref_list: String::new(),
            ref_sig: None,
            status_sig: None,
            busy: false,
            reloading: false,
            force_again: false,
            polls: 0,
            loading: Some(t("Loading repository…").to_string()),
            bar: None,
            list_width: width("gitListWidth", 380., &self.config),
            wide_width: width("gitWidth", 760., &self.config),
            files_col_width: width("gitFilesWidth", 300., &self.config),
            _subscription: subscription,
        });
        self.git_reload(true, cx);
        cx.notify();
    }

    /// An open panel follows the active session to its repository or worktree.
    pub(super) fn git_follow_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.active().map(|o| o.cwd.clone()) else { return };
        let Some(current) = self.git.as_ref().map(|g| g.root.clone()) else { return };
        if folder_text(&folder).starts_with(&folder_text(&current)) {
            return;
        }
        let find = cx.background_spawn(async move { git::root_of(&folder) });
        cx.spawn_in(window, async move |this, cx| {
            if let Some(root) = find.await {
                let _ = this.update_in(cx, |app, window, cx| {
                    if app.git.is_some() {
                        app.git = None;
                        app.show_git(root, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// Reloads what changed since last time. A refresh is one status and one diff summary; the full
    /// list of refs is checked less often, and at once when forced.
    pub(super) fn git_reload(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        if g.reloading {
            g.force_again |= force;
            return;
        }
        g.reloading = true;
        g.polls += 1;
        let check_refs = force || g.polls % REFS_EVERY == 0;
        let (root, git_dir) = (g.root.clone(), g.git_dir.clone());
        let (prev_ref_list, prev_ref_sig, prev_status_sig) = (g.ref_list.clone(), g.ref_sig.clone(), g.status_sig.clone());
        let (history, diff_open, log_stale, limit, filter) = (g.view == GitView::History, g.diff_open, g.log_stale, g.limit, g.file_filter.clone());
        let prev_locals = g.refs.locals.clone();
        let work = cx.background_spawn(async move {
            let raw = git::run(&root, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]);
            // An edit to an already changed file doesn't alter the status.
            let changes = git::run(&root, &["diff", "--numstat", "-z"]);
            let status = git::parse_status(&raw.stdout);
            let ref_list = check_refs.then(|| git::run(&root, &["for-each-ref", "--format=%(objectname) %(refname)"]).stdout);
            let ref_sig = format!("{}{}", status.signature, ref_list.as_deref().unwrap_or(&prev_ref_list));
            let status_sig = format!("{}{}", raw.stdout, changes.stdout);
            let refs_changed = force || prev_ref_sig.as_deref() != Some(ref_sig.as_str());
            let status_changed = force || prev_status_sig.as_deref() != Some(status_sig.as_str());
            let numstat = status_changed.then(|| {
                let staged = git::run(&root, &["diff", "--cached", "--numstat", "-z", "--no-renames"]);
                let unstaged = git::run(&root, &["diff", "--numstat", "-z", "--no-renames"]);
                (git::parse_numstat(&staged.stdout), git::parse_numstat(&unstaged.stdout))
            });
            let diffs = (status_changed && !history && diff_open).then(|| load_diffs(&root, &status));
            let refs = refs_changed.then(|| git::load_refs(&root, git_dir.as_deref()));
            let stale = log_stale || refs_changed;
            let log = (stale && history).then(|| load_log(&root, status.head.is_some(), refs.as_ref().map_or(&prev_locals, |r| &r.locals), limit, filter.as_deref()));
            Reloaded { root, status, ref_list, ref_sig, status_sig, changed: refs_changed || status_changed, numstat, diffs, refs, log }
        });
        cx.spawn(async move |this, cx| {
            let done = work.await;
            let _ = this.update(cx, |app, cx| app.apply_reload(done, cx));
        })
        .detach();
    }

    fn apply_reload(&mut self, r: Reloaded, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.reloading = false;
        g.loading = if g.busy { g.loading.take() } else { None };
        if folder_text(&r.root) != folder_text(&g.root) {
            return;
        }
        let was_refs_stale = r.refs.is_some();
        g.status = r.status;
        if let Some(list) = r.ref_list {
            g.ref_list = list;
        }
        g.ref_sig = Some(r.ref_sig);
        g.status_sig = Some(r.status_sig);
        if r.changed {
            if was_refs_stale {
                g.log_stale = true;
            }
            if let Some((a, b)) = r.numstat {
                g.numstat_staged = a;
                g.numstat_unstaged = b;
            }
            if let Some((diffs, conflicts)) = r.diffs {
                g.diffs = diffs;
                g.conflicts = conflicts;
            }
            if let Some(refs) = r.refs {
                g.refs = refs;
            }
            if let Some((commits, lanes)) = r.log {
                g.commits = Rc::new(commits);
                g.lanes = lanes;
                g.log_stale = false;
            }
            if g.selected.as_ref().is_some_and(|s| !g.commits.iter().any(|c| &c.hash == s) && !g.refs.stashes.iter().any(|t| &t.reference == s)) {
                g.selected = None;
                g.commit_open = false;
                g.detail = None;
            }
            let live: HashSet<String> = git::entries(&g.status).into_iter().map(|e| e.path).collect();
            g.picked.retain(|p| live.contains(p));
            self.rebuild_diff_rows();
            self.refresh_branch(cx);
            cx.notify();
        }
        let again = self.git.as_mut().is_some_and(|g| std::mem::take(&mut g.force_again));
        if again {
            self.git_reload(true, cx);
        }
    }

    /// Reloads just the diff, when the view asks for it.
    fn load_project_diff(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let (root, status) = (g.root.clone(), g.status.clone());
        let work = cx.background_spawn(async move { load_diffs(&root, &status) });
        cx.spawn(async move |this, cx| {
            let (diffs, conflicts) = work.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(g) = app.git.as_mut() {
                    g.diffs = diffs;
                    g.conflicts = conflicts;
                }
                app.rebuild_diff_rows();
                cx.notify();
            });
        })
        .detach();
    }

    fn load_history(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let (root, head, locals, limit, filter) = (g.root.clone(), g.status.head.is_some(), g.refs.locals.clone(), g.limit, g.file_filter.clone());
        let work = cx.background_spawn(async move { load_log(&root, head, &locals, limit, filter.as_deref()) });
        cx.spawn(async move |this, cx| {
            let (commits, lanes) = work.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(g) = app.git.as_mut() {
                    g.commits = Rc::new(commits);
                    g.lanes = lanes;
                    g.log_stale = false;
                    g.loading = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn load_more_commits(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.limit += LOG_PAGE;
        }
        self.load_history(cx);
    }

    pub(super) fn set_git_view(&mut self, view: GitView, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        // Its tab always leads back to the list.
        g.commit_open = false;
        g.view = view;
        let (history, stale, diff_open) = (view == GitView::History, g.log_stale, g.diff_open);
        if history && stale {
            g.loading = Some(t("Loading history…").to_string());
        }
        self.save_setting("gitView", json!(if history { "history" } else { "changes" }));
        if history && stale {
            self.load_history(cx);
        }
        if !history && diff_open {
            self.load_project_diff(cx);
        }
        self.rebuild_diff_rows();
        cx.notify();
    }

    pub(super) fn toggle_diff(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        if g.diff_open == open {
            return;
        }
        g.diff_open = open;
        self.save_setting("gitDiffOpen", json!(open));
        if open {
            self.load_project_diff(cx);
        }
        self.rebuild_diff_rows();
        cx.notify();
    }

    pub(super) fn toggle_diff_style(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.split_diff = !g.split_diff;
        let style = if g.split_diff { "split" } else { "unified" };
        self.save_setting("gitDiffStyle", json!(style));
        self.rebuild_diff_rows();
        cx.notify();
    }

    pub(super) fn set_git_list_option(&mut self, tree: Option<bool>, by_tracked: Option<bool>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        if let Some(v) = tree {
            g.tree = v;
            self.save_setting("gitTree", json!(v));
        }
        if let Some(v) = by_tracked {
            if let Some(g) = self.git.as_mut() {
                g.by_tracked = v;
            }
            self.save_setting("gitGroupBy", json!(if v { "tracked" } else { "staged" }));
        }
        cx.notify();
    }

    // ---------- running git ----------

    pub(super) fn git_bar(&mut self, text: impl Into<String>, kind: BarKind, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else {
            self.toast(text.into(), if kind == BarKind::Error { overlays::ToastKind::Error } else { overlays::ToastKind::Plain }, cx);
            return;
        };
        let seq = g.bar.as_ref().map_or(1, |b| b.2 + 1);
        g.bar = Some((text.into(), kind, seq));
        if kind == BarKind::Ok {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let _ = this.update(cx, |app, cx| {
                    if let Some(g) = app.git.as_mut() {
                        if g.bar.as_ref().is_some_and(|b| b.2 == seq) {
                            g.bar = None;
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Runs git commands one after another, stopping at the first failure, then reloads the view.
    /// `quiet` only reports failures.
    pub(super) fn git_op(&mut self, label: impl Into<String>, steps: Vec<Step>, quiet: bool, then: Option<overlays::Action>, cx: &mut Context<Self>) {
        let label: String = label.into();
        let Some(g) = self.git.as_mut() else { return };
        if g.busy {
            return;
        }
        g.busy = true;
        if !quiet {
            g.loading = Some(format!("{}…", git::progressive(&label)));
        }
        let root = g.root.clone();
        if !quiet {
            self.git_bar(format!("{label}…"), BarKind::Busy, cx);
        }
        let work = cx.background_spawn(async move {
            let mut last = Output::default();
            for (args, input) in steps {
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                last = git::run_with(&root, &refs, input.as_deref(), &[]);
                if !last.ok() {
                    return (false, last);
                }
            }
            (true, last)
        });
        cx.spawn(async move |this, cx| {
            let (ok, out) = work.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(g) = app.git.as_mut() {
                    g.busy = false;
                    g.loading = None;
                }
                let said = out.message();
                if !ok {
                    let why = if said.is_empty() { "git exited with an error.".to_string() } else { said };
                    app.git_bar(format!("{label} failed. {why}"), BarKind::Error, cx);
                } else if !quiet {
                    let tail = said.lines().filter(|l| !l.trim().is_empty()).last().map_or(String::new(), |l| format!(" {l}"));
                    app.git_bar(format!("{label}: done.{tail}"), BarKind::Ok, cx);
                }
                app.git_reload(true, cx);
                if ok {
                    if let Some(then) = then {
                        app.deferred_action = Some(then);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn stage(&mut self, list: &[Change], all: bool, cx: &mut Context<Self>) {
        if list.is_empty() {
            return;
        }
        let steps = if all { vec![step(&["add", "-A"])] } else { chunked(&["add", "-A"], &list.iter().map(|f| f.path.clone()).collect::<Vec<_>>()) };
        self.git_op("Stage", steps, true, None, cx);
    }

    pub(super) fn unstage(&mut self, list: &[Change], all: bool, cx: &mut Context<Self>) {
        if list.is_empty() {
            return;
        }
        let has_head = self.git.as_ref().is_some_and(|g| g.status.head.is_some());
        let paths: Vec<String> = list.iter().flat_map(|f| f.orig.iter().cloned().chain([f.path.clone()])).collect();
        let steps = match (has_head, all) {
            (false, true) => vec![step(&["rm", "-r", "-q", "--cached", "."])],
            (false, false) => chunked(&["rm", "-r", "-q", "--cached"], &paths),
            (true, true) => vec![step(&["reset", "-q"])],
            (true, false) => chunked(&["reset", "-q", "HEAD"], &paths),
        };
        self.git_op("Unstage", steps, true, None, cx);
    }

    pub(super) fn toggle_staged(&mut self, list: &[Entry], cx: &mut Context<Self>) {
        let all_staged = !list.is_empty() && list.iter().all(|e| e.staged.is_some() && e.unstaged.is_none());
        if all_staged {
            let changes: Vec<Change> = list.iter().filter_map(|e| e.staged.clone()).collect();
            self.unstage(&changes, false, cx);
        } else {
            let changes: Vec<Change> = list.iter().filter_map(|e| e.unstaged.clone()).collect();
            self.stage(&changes, false, cx);
        }
    }

    pub(super) fn stage_all_toggle(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        if g.status.unstaged.is_empty() {
            let staged = g.status.staged.clone();
            self.unstage(&staged, true, cx);
        } else {
            let all = g.status.unstaged.clone();
            self.stage(&all, true, cx);
        }
    }

    pub(super) fn discard(&mut self, list: Vec<Change>, window: &mut Window, cx: &mut Context<Self>) {
        if list.is_empty() {
            return;
        }
        let untracked: Vec<String> = list.iter().filter(|f| f.status == '?').map(|f| f.path.clone()).collect();
        let tracked: Vec<String> = list.iter().filter(|f| f.status != '?').map(|f| f.path.clone()).collect();
        let what = if list.len() == 1 { list[0].path.clone() } else { format!("{} files", list.len()) };
        let warn = match untracked.len() {
            0 => String::new(),
            n if n == list.len() => " It is untracked and will be deleted.".to_string(),
            n => format!(" {n} untracked file(s) will be deleted."),
        };
        self.ask(
            t("Restore"),
            format!("Throw away the unstaged changes to {what}?{warn} This can't be undone."),
            t("Restore"),
            true,
            move |app, _, cx| {
                let mut steps = chunked(&["checkout"], &tracked);
                steps.extend(chunked(&["clean", "-f", "-q"], &untracked));
                app.git_op("Restore", steps, true, None, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn apply_hunk(&mut self, header: &[String], hunk: &Hunk, mode: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let patch = git::hunk_patch(header, hunk);
        let (label, args): (&str, Vec<&str>) = match mode {
            "stage" => ("Stage hunk", vec!["apply", "--cached", "--whitespace=nowarn", "-"]),
            "unstage" => ("Unstage hunk", vec!["apply", "--cached", "-R", "--whitespace=nowarn", "-"]),
            _ => ("Restore hunk", vec!["apply", "-R", "--whitespace=nowarn", "-"]),
        };
        let run_step: Step = (args.iter().map(|a| (*a).to_string()).collect(), Some(patch));
        if mode == "discard" {
            self.ask(
                t("Restore hunk"),
                t("Throw away this hunk's changes? This can't be undone."),
                t("Restore"),
                true,
                move |app, _, cx| app.git_op(label, vec![run_step.clone()], true, None, cx),
                window,
                cx,
            );
        } else {
            self.git_op(label, vec![run_step], true, None, cx);
        }
    }

    pub(super) fn resolve_conflict(&mut self, path: &str, index: usize, choice: &'static str, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let file = g.root.join(path);
        // Read again so nothing typed since is lost.
        let result = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| {
            let resolved = git::resolve_conflict(&text, index, choice).ok_or_else(|| "The conflict changed. Look again.".to_string())?;
            std::fs::write(&file, resolved).map_err(|e| e.to_string())
        });
        if let Err(why) = result {
            self.git_bar(why, BarKind::Error, cx);
        }
        self.git_reload(true, cx);
        self.load_project_diff(cx);
    }

    fn current_branch(&self) -> Option<&LocalBranch> {
        let g = self.git.as_ref()?;
        g.refs.locals.iter().find(|b| b.name == g.status.branch)
    }

    /// What the one sync button does next: push what's ahead, pull what's behind, publish a new branch,
    /// or fetch when there's nothing else to do. (icon, label)
    pub(super) fn sync_action(&self) -> Option<(&'static str, String)> {
        let g = self.git.as_ref()?;
        if g.refs.remotes.is_empty() {
            return None;
        }
        let current = self.current_branch();
        if !g.status.branch.is_empty() && current.is_none_or(|b| b.upstream.is_empty() || b.gone) {
            return Some(("push", t("Publish").to_string()));
        }
        if let Some(b) = current.filter(|b| b.behind > 0) {
            let ahead = if b.ahead > 0 { format!(" ↑{}", b.ahead) } else { String::new() };
            return Some(("pull", format!("{} ↓{}{ahead}", t("Pull"), b.behind)));
        }
        if let Some(b) = current.filter(|b| b.ahead > 0) {
            return Some(("push", format!("{} ↑{}", t("Push"), b.ahead)));
        }
        Some(("fetch", t("Fetch").to_string()))
    }

    pub(super) fn run_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.sync_action().map(|a| a.0) {
            Some("push") => self.push(window, cx),
            Some("pull") => self.pull(&[], cx),
            Some("fetch") => self.fetch_all(cx),
            _ => {}
        }
    }

    pub(super) fn fetch_all(&mut self, cx: &mut Context<Self>) {
        if self.git.as_ref().is_some_and(|g| g.refs.remotes.is_empty()) {
            self.git_bar("This repository has no remotes to fetch from.", BarKind::Error, cx);
            return;
        }
        self.git_op("Fetch", vec![step(&["fetch", "--all", "--prune"])], false, None, cx);
    }

    pub(super) fn pull(&mut self, extra: &[&str], cx: &mut Context<Self>) {
        let branch = self.git.as_ref().map_or(String::new(), |g| g.status.branch.clone());
        if !branch.is_empty() && self.current_branch().is_none_or(|b| b.upstream.is_empty()) {
            self.git_bar(format!("{branch} has no upstream branch to pull from. Publish it first, or check out a remote branch."), BarKind::Error, cx);
            return;
        }
        let mut args = vec!["pull", "--no-edit"];
        args.extend(extra);
        self.git_op("Pull", vec![step(&args)], false, None, cx);
    }

    pub(super) fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let branch = g.status.branch.clone();
        if branch.is_empty() {
            self.git_bar("HEAD is detached: pick a branch to push.", BarKind::Error, cx);
            return;
        }
        if self.current_branch().is_some_and(|b| !b.upstream.is_empty() && !b.gone) {
            self.git_op("Push", vec![step(&["push"])], false, None, cx);
            return;
        }
        let remotes = g.refs.remotes.clone();
        let root = g.root.clone();
        if remotes.is_empty() {
            self.git_bar("This repository has no remote to push to. Add one with git remote add.", BarKind::Error, cx);
            return;
        }
        // Git's own settings decide first: pushRemote for the branch, then remote.pushDefault.
        let push_remote = git::run(&root, &["config", &format!("branch.{branch}.pushRemote")]);
        let push_default = git::run(&root, &["config", "remote.pushDefault"]);
        let configured = [push_remote, push_default].into_iter().filter(Output::ok).map(|o| o.stdout.trim().to_string()).find(|r| remotes.contains(r));
        let preferred = configured.clone().unwrap_or_else(|| if remotes.iter().any(|r| r == "origin") { "origin".to_string() } else { remotes[0].clone() });
        if remotes.len() == 1 || configured.is_some() {
            self.git_op("Publish", vec![step(&["push", "-u", &preferred, &branch])], false, None, cx);
            return;
        }
        self.open_form(
            t("Publish"),
            Some(format!("{branch} has no upstream branch yet. Push it and track the remote branch?")),
            vec![Spec::Select { id: "remote", label: "Remote", options: remotes, value: preferred }],
            t("Push"),
            false,
            move |app, v, _, cx| {
                let remote = v.text("remote");
                app.git_op("Publish", vec![step(&["push", "-u", &remote, &branch])], false, None, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn force_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.git.as_ref().map_or(String::new(), |g| g.status.branch.clone());
        if branch.is_empty() {
            self.git_bar("HEAD is detached: pick a branch to push.", BarKind::Error, cx);
            return;
        }
        self.ask(
            t("Force push"),
            format!("Overwrite the remote {branch} with your version? Commits only on the remote are lost. The push stops if someone else pushed since your last fetch."),
            t("Force push"),
            true,
            |app, _, cx| app.git_op("Force push", vec![step(&["push", "--force-with-lease"])], false, None, cx),
            window,
            cx,
        );
    }

    pub(super) fn commit_label(&self) -> &'static str {
        let Some(g) = &self.git else { return t("Commit") };
        if g.amend {
            return t("Amend");
        }
        if !g.status.staged.is_empty() || g.refs.operation == Some("merge") {
            return t("Commit");
        }
        if g.status.unstaged.iter().any(|f| f.status != '?') { t("Commit Tracked") } else { t("Commit") }
    }

    pub(super) fn do_commit(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let message = g.message.read(cx).value().trim().to_string();
        if message.is_empty() {
            self.git_bar(t("Write a commit message first."), BarKind::Error, cx);
            return;
        }
        let mut steps = Vec::new();
        if g.status.staged.is_empty() && !g.amend && g.refs.operation != Some("merge") {
            // Nothing staged: commit what's tracked. Untracked files are only added on request.
            if !g.status.unstaged.iter().any(|f| f.status != '?') {
                let text = if g.status.unstaged.is_empty() { "There is nothing to commit." } else { "Only untracked files changed: tick the ones to commit." };
                self.git_bar(t(text), BarKind::Error, cx);
                return;
            }
            steps.push(step(&["add", "-u"]));
        }
        let mut args = vec!["commit", "-F", "-"];
        if g.amend {
            args.push("--amend");
        }
        if g.signoff {
            args.push("--signoff");
        }
        if g.no_verify {
            args.push("--no-verify");
        }
        steps.push((args.iter().map(|a| (*a).to_string()).collect(), Some(format!("{message}\n"))));
        let (label, push_after) = (if g.amend { "Amend commit" } else { "Commit" }, g.push_after);
        self.git_op(
            label,
            steps,
            false,
            Some(Rc::new(move |app: &mut CrewApp, window: &mut Window, cx: &mut Context<CrewApp>| {
                if let Some(g) = app.git.as_mut() {
                    let message = g.message.clone();
                    message.update(cx, |m, cx| m.set_value("", window, cx));
                    g.amend = false;
                    g.signoff = false;
                    g.no_verify = false;
                    g.push_after = false;
                }
                if push_after {
                    app.push(window, cx);
                }
            })),
            cx,
        );
    }

    pub(super) fn toggle_amend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.amend = !g.amend;
        if g.amend && g.status.head.is_some() && g.message.read(cx).value().trim().is_empty() {
            let last = git::run(&g.root, &["log", "-1", "--format=%B"]);
            if last.ok() {
                let (message, text) = (g.message.clone(), last.stdout.trim().to_string());
                message.update(cx, |m, cx| m.set_value(text, window, cx));
            }
        }
        cx.notify();
    }

    pub(super) fn uncommit(&mut self, cx: &mut Context<Self>) {
        if self.git.as_ref().and_then(|g| g.refs.last.as_ref()).is_some_and(|l: &LastCommit| l.has_parent) {
            self.git_op("Uncommit", vec![step(&["reset", "--soft", "HEAD^"])], false, None, cx);
        }
    }

    pub(super) fn stash_changes(&mut self, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_entries().is_empty() {
            self.git_bar("There are no changes to stash.", BarKind::Error, cx);
            return;
        }
        let title = match kind {
            "tracked" => t("Stash tracked"),
            "staged" => t("Stash staged"),
            _ => t("Stash all"),
        };
        self.open_form(
            title,
            None,
            vec![Spec::Text { id: "msg", label: "Name", value: String::new(), placeholder: "Optional", required: false }],
            t("Stash"),
            false,
            move |app, v, _, cx| {
                let mut args = vec!["stash", "push"];
                match kind {
                    "all" => args.push("--include-untracked"),
                    "staged" => args.push("--staged"),
                    _ => {}
                }
                let message = v.text("msg");
                if !message.is_empty() {
                    args.extend(["-m", &message]);
                }
                app.git_op("Stash", vec![step(&args)], false, None, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn restore_tracked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_entries().iter().any(|e| e.status != '?') {
            self.git_bar("There are no changes to restore.", BarKind::Error, cx);
            return;
        }
        let has_head = self.git.as_ref().is_some_and(|g| g.status.head.is_some());
        self.ask(
            t("Restore tracked files"),
            t("Reset every tracked file to the last commit, staged and unstaged changes alike. This can't be undone."),
            t("Restore"),
            true,
            move |app, _, cx| {
                let s = if has_head { step(&["reset", "--hard", "-q"]) } else { step(&["rm", "-r", "-q", "--cached", "."]) };
                app.git_op("Restore tracked files", vec![s], false, None, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn trash_untracked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let list: Vec<PathBuf> = g.status.unstaged.iter().filter(|f| f.status == '?').map(|f| g.root.join(&f.path)).collect();
        if list.is_empty() {
            self.git_bar("There are no untracked files.", BarKind::Error, cx);
            return;
        }
        let count = list.len();
        self.ask(
            t("Move to trash"),
            format!("Move {count} untracked file{} to the Recycle Bin?", if count == 1 { "" } else { "s" }),
            t("Move to trash"),
            true,
            move |app, _, cx| {
                let failed = list.iter().filter(|f| trash::delete(f).is_err()).count();
                if failed > 0 {
                    app.git_bar(format!("{failed} file(s) couldn't be moved to the Recycle Bin."), BarKind::Error, cx);
                }
                app.git_reload(true, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn ignore_path(&mut self, pattern: &str, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let line = format!("/{pattern}");
        match git::ignore(&g.root, &line) {
            Ok(()) => self.git_bar(format!("Added {line} to .gitignore."), BarKind::Ok, cx),
            Err(_) => self.git_bar("Couldn't update .gitignore.", BarKind::Error, cx),
        }
        self.git_reload(true, cx);
    }

    pub(super) fn file_permalink(&mut self, path: &str, open: bool, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let remote = if g.refs.remotes.iter().any(|r| r == "origin") { Some("origin".to_string()) } else { g.refs.remotes.first().cloned() };
        let (Some(remote), Some(head)) = (remote, g.status.head.clone()) else {
            self.git_bar("There is no remote to link to.", BarKind::Error, cx);
            return;
        };
        let url = git::run(&g.root, &["remote", "get-url", &remote]);
        match git::permalink(&url.stdout, &head, path) {
            None => self.git_bar("Couldn't make a link from this remote's address.", BarKind::Error, cx),
            Some(link) if open => cx.open_url(&link),
            Some(link) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(link));
                self.git_bar("Copied the link.", BarKind::Ok, cx);
            }
        }
    }

    pub(super) fn merge_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let branch = g.status.branch.clone();
        let mut options: Vec<String> = g.refs.locals.iter().map(|b| b.name.clone()).filter(|n| *n != branch).collect();
        options.extend(g.refs.remote_branches.iter().map(|b| b.name.clone()));
        if options.is_empty() {
            self.git_bar("There is no other branch to merge.", BarKind::Error, cx);
            return;
        }
        let upstream = self.current_branch().map(|b| b.upstream.clone()).filter(|u| options.contains(u));
        let value = upstream.unwrap_or_else(|| options[0].clone());
        self.open_form(
            t("Merge"),
            Some(format!("Merge a branch into {}.", if branch.is_empty() { "the detached HEAD" } else { &branch })),
            vec![
                Spec::Select { id: "branch", label: "Branch to merge", options, value },
                Spec::Check { id: "noff", label: t("Always create a merge commit (--no-ff)").to_string(), value: false },
            ],
            t("Merge"),
            false,
            |app, v, _, cx| app.merge_branch(&v.text("branch"), v.on("noff"), cx),
            window,
            cx,
        );
    }

    pub(super) fn merge_branch(&mut self, name: &str, no_ff: bool, cx: &mut Context<Self>) {
        let mut args = vec!["merge", "--no-edit"];
        if no_ff {
            args.push("--no-ff");
        }
        args.push(name);
        self.git_op(format!("Merge {name}"), vec![step(&args)], false, None, cx);
    }

    pub(super) fn rebase_onto(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.git.as_ref().map_or(String::new(), |g| g.status.branch.clone());
        if branch.is_empty() {
            self.git_bar("Check out a branch before rebasing.", BarKind::Error, cx);
            return;
        }
        self.ask(
            t("Rebase"),
            format!("Rebase {branch} onto {name}? Its commits are replayed on top of {name}, rewriting them."),
            t("Rebase"),
            false,
            move |app, _, cx| app.git_op(format!("Rebase onto {name}"), vec![step(&["rebase", &name])], false, None, cx),
            window,
            cx,
        );
    }

    pub(super) fn abort_operation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(op) = self.git.as_ref().and_then(|g| g.refs.operation) else { return };
        self.ask(
            format!("Abort {op}"),
            format!("Stop the {op} and go back to how things were before it started?"),
            t("Abort"),
            true,
            move |app, _, cx| app.git_op(format!("Abort {op}"), vec![step(&[op, "--abort"])], false, None, cx),
            window,
            cx,
        );
    }

    pub(super) fn continue_operation(&mut self, cx: &mut Context<Self>) {
        if let Some(op) = self.git.as_ref().and_then(|g| g.refs.operation) {
            self.git_op(format!("Continue {op}"), vec![step(&[op, "--continue"])], false, None, cx);
        }
    }

    pub(super) fn checkout_detached(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = short(&target);
        self.ask(
            t("Check out commit"),
            format!("Check out {name}? HEAD will be detached: new commits won't belong to any branch until you create one."),
            t("Check out"),
            false,
            move |app, _, cx| app.git_op(format!("Check out {name}"), vec![step(&["checkout", "--detach", &target])], false, None, cx),
            window,
            cx,
        );
    }

    pub(super) fn checkout_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.git.as_ref().is_some_and(|g| g.status.branch == name) {
            return;
        }
        self.git_op(format!("Check out {name}"), vec![step(&["checkout", name])], false, None, cx);
    }

    fn checkout_remote(&mut self, b: RemoteBranch, window: &mut Window, cx: &mut Context<Self>) {
        let local = b.name[b.remote.len() + 1..].to_string();
        let tracked = self.git.as_ref().is_some_and(|g| g.refs.locals.iter().any(|x| x.name == local && x.upstream == b.name));
        if tracked {
            self.checkout_branch(&local, cx);
            return;
        }
        self.open_form(
            t("Check out remote branch"),
            Some(format!("Create a local branch that tracks {} and check it out.", b.name)),
            vec![Spec::Text { id: "name", label: "Local branch name", value: local, placeholder: "", required: true }],
            t("Check out"),
            false,
            move |app, v, _, cx| {
                let name = v.text("name");
                app.git_op(format!("Check out {name}"), vec![step(&["checkout", "-b", &name, "--track", &b.name])], false, None, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn new_branch(&mut self, at: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let head = g.status.head.clone();
        let start = at.filter(|a| Some(a) != head.as_ref());
        let text = match &start {
            Some(a) => format!("Start the branch at commit {}.", short(a)),
            None if !g.status.branch.is_empty() => format!("Start the branch at the current branch ({}).", g.status.branch),
            None => "Start the branch at the current commit.".to_string(),
        };
        self.open_form(
            t("New branch"),
            Some(text),
            vec![
                Spec::Text { id: "name", label: "Branch name", value: String::new(), placeholder: "", required: true },
                Spec::Check { id: "checkout", label: t("Check out the new branch").to_string(), value: true },
            ],
            t("Create branch"),
            false,
            move |app, v, _, cx| {
                let name = v.text("name");
                let mut args: Vec<&str> = if v.on("checkout") { vec!["checkout", "-b", &name] } else { vec!["branch", &name] };
                if let Some(s) = &start {
                    args.push(s);
                }
                app.git_op(format!("Create branch {name}"), vec![step(&args)], false, None, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn new_tag(&mut self, at: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = at else {
            self.git_bar("Make a commit before adding a tag.", BarKind::Error, cx);
            return;
        };
        let Some(g) = &self.git else { return };
        let remote = if g.refs.remotes.iter().any(|r| r == "origin") { Some("origin".to_string()) } else { g.refs.remotes.first().cloned() };
        let mut specs = vec![
            Spec::Text { id: "name", label: "Tag name", value: String::new(), placeholder: "e.g. v1.0.0", required: true },
            Spec::Area { id: "msg", label: "Message", placeholder: "Optional. With a message the tag is annotated." },
        ];
        if let Some(r) = &remote {
            specs.push(Spec::Check { id: "push", label: format!("Push the tag to {r}"), value: false });
        }
        self.open_form(
            t("New tag"),
            Some(format!("Tag commit {}.", short(&at))),
            specs,
            t("Create tag"),
            false,
            move |app, v, _, cx| {
                let (name, message) = (v.text("name"), v.text("msg"));
                let mut steps = vec![if message.is_empty() { step(&["tag", &name, &at]) } else { step(&["tag", "-a", &name, "-m", &message, &at]) }];
                if let (true, Some(r)) = (v.on("push"), &remote) {
                    steps.push(step(&["push", r, &format!("refs/tags/{name}")]));
                }
                app.git_op(format!("Create tag {name}"), steps, false, None, cx);
            },
            window,
            cx,
        );
    }

    fn reset_to(&mut self, c: Commit, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.git.as_ref().map_or(String::new(), |g| g.status.branch.clone());
        let options = vec!["Soft: keep all changes staged".to_string(), "Mixed: keep changes in the working copy".to_string(), "Hard: discard all changes".to_string()];
        let value = options[1].clone();
        let hash = c.hash.clone();
        self.open_form(
            format!("Reset {branch}"),
            Some(format!("Move {branch} to {} \"{}\". Commits after it leave the branch.", short(&c.hash), c.subject)),
            vec![Spec::Select { id: "mode", label: "Mode", options, value }],
            t("Reset"),
            true,
            move |app, v, window, cx| {
                let mode = v.text("mode").split(':').next().unwrap_or("mixed").to_lowercase();
                let flag = format!("--{mode}");
                let reset = step(&["reset", &flag, "-q", &hash]);
                let label = format!("Reset ({mode})");
                if mode == "hard" {
                    app.ask(t("Hard reset"), t("A hard reset deletes every uncommitted change. Continue?"), t("Reset"), true, move |app, _, cx| app.git_op(label.clone(), vec![reset.clone()], false, None, cx), window, cx);
                } else {
                    app.git_op(label, vec![reset], false, None, cx);
                }
            },
            window,
            cx,
        );
    }

    // ---------- selecting ----------

    pub(super) fn pick_file(&mut self, e: &Entry, list: &[Entry], modifiers: gpui::Modifiers, open_diff: bool, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        if modifiers.shift && g.anchor.is_some() {
            let find = |p: &str| list.iter().position(|x| x.path == p);
            if let (Some(a), Some(b)) = (g.anchor.as_deref().and_then(find), find(&e.path)) {
                g.picked = list[a.min(b)..=a.max(b)].iter().map(|x| x.path.clone()).collect();
            }
        } else if modifiers.control {
            if !g.picked.remove(&e.path) {
                g.picked.insert(e.path.clone());
            }
            g.anchor = Some(e.path.clone());
        } else {
            g.picked = [e.path.clone()].into();
            g.anchor = Some(e.path.clone());
        }
        if open_diff {
            self.toggle_diff(true, cx);
            self.scroll_diff_to(&e.path);
        }
        cx.notify();
    }

    pub(super) fn scroll_diff_to(&mut self, path: &str) {
        let Some(g) = &self.git else { return };
        if let Some(i) = g.diff_rows.iter().position(|r| matches!(r, DiffRow::File { key, .. } if key == path)) {
            g.diff_scroll.scroll_to_item(i, ScrollStrategy::Top);
        }
    }

    pub(super) fn toggle_file_fold(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            if !g.folded_files.remove(key) {
                g.folded_files.insert(key.to_string());
            }
        }
        self.rebuild_diff_rows();
        cx.notify();
    }

    pub(super) fn show_full_diff(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.full_diff = true;
        }
        self.rebuild_diff_rows();
        cx.notify();
    }

    pub(super) fn select_commit(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.selected = Some(key.clone());
        g.full_diff = false;
        let cache_key = format!("{key}|{}", g.file_filter.as_deref().unwrap_or(""));
        if let Some(found) = g.detail_cache.get(&cache_key).cloned() {
            g.detail = Some(found);
            self.rebuild_diff_rows();
            cx.notify();
            return;
        }
        g.detail = None;
        let (root, filter) = (g.root.clone(), g.file_filter.clone());
        let is_hash = key.len() == 40 && key.chars().all(|c| c.is_ascii_hexdigit());
        let wanted = key.clone();
        let work = cx.background_spawn(async move { git::load_commit(&root, &key, filter.as_deref()) });
        cx.spawn(async move |this, cx| {
            let detail = work.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(g) = app.git.as_mut().filter(|g| g.selected.as_deref() == Some(wanted.as_str())) {
                    if let Some(d) = detail.as_ref().filter(|_| is_hash) {
                        g.detail_cache.insert(cache_key, d.clone());
                    }
                    g.detail = detail;
                }
                app.rebuild_diff_rows();
                cx.notify();
            });
        })
        .detach();
        self.rebuild_diff_rows();
        cx.notify();
    }

    /// A commit's page: its details, files and diff, in place of the list.
    pub(super) fn open_commit(&mut self, hash: String, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.commit_open = true;
        }
        self.select_commit(hash, cx);
    }

    /// Back to the list, where it was, with the commit still marked.
    pub(super) fn close_commit(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.commit_open = false;
        if let Some(i) = g.commits.iter().position(|c| Some(&c.hash) == g.selected.as_ref()) {
            g.log_scroll.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    pub(super) fn jump_to_commit(&mut self, hash: String, cx: &mut Context<Self>) {
        if self.git.as_ref().is_some_and(|g| g.view != GitView::History) {
            self.set_git_view(GitView::History, cx);
        }
        self.open_commit(hash.clone(), cx);
        if let Some(g) = &self.git {
            if let Some(i) = g.commits.iter().position(|c| c.hash == hash) {
                g.log_scroll.scroll_to_item(i, ScrollStrategy::Center);
            }
        }
    }

    pub(super) fn step_commit(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let now = g.commits.iter().position(|c| Some(&c.hash) == g.selected.as_ref());
        let next = now.map_or(0, |i| if down { (i + 1).min(g.commits.len().saturating_sub(1)) } else { i.saturating_sub(1) });
        if let Some(c) = g.commits.get(next) {
            let hash = c.hash.clone();
            g.log_scroll.scroll_to_item(next, ScrollStrategy::Nearest);
            self.select_commit(hash, cx);
        }
    }

    pub(super) fn open_file_history(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.file_filter = path;
            g.selected = None;
            g.commit_open = false;
            g.detail = None;
            g.log_stale = true;
        }
        self.set_git_view(GitView::History, cx);
    }

    /// Everything the diff list shows, flattened into rows of one height.
    pub(super) fn rebuild_diff_rows(&mut self) {
        let Some(g) = self.git.as_mut() else { return };
        let mut rows: Vec<DiffRow> = Vec::new();
        let limit = if g.full_diff { usize::MAX } else { DIFF_LINES_SHOWN };
        let split = g.split_diff;
        let push_hunk = |rows: &mut Vec<DiffRow>, hunk: &Hunk, tag: Option<&'static str>, actions: HunkActions, left: &mut usize| {
            let title = hunk.head.splitn(3, "@@").nth(2).map(str::trim).filter(|s| !s.is_empty()).map_or_else(|| hunk.head.clone(), str::to_string);
            rows.push(DiffRow::Hunk { title, tag, actions });
            let lines = git::hunk_rows(hunk);
            *left = left.saturating_sub(lines.len());
            if split {
                let mut i = 0;
                while i < lines.len() {
                    match lines[i].kind {
                        git::LineKind::Del | git::LineKind::Add => {
                            let (mut dels, mut adds) = (Vec::new(), Vec::new());
                            while lines.get(i).is_some_and(|l| l.kind == git::LineKind::Del) {
                                dels.push(lines[i].clone());
                                i += 1;
                            }
                            while lines.get(i).is_some_and(|l| l.kind == git::LineKind::Add) {
                                adds.push(lines[i].clone());
                                i += 1;
                            }
                            for k in 0..dels.len().max(adds.len()) {
                                rows.push(DiffRow::Sides(dels.get(k).cloned(), adds.get(k).cloned()));
                            }
                        }
                        git::LineKind::Meta => {
                            rows.push(DiffRow::Sides(Some(lines[i].clone()), None));
                            i += 1;
                        }
                        git::LineKind::Context => {
                            rows.push(DiffRow::Sides(Some(lines[i].clone()), Some(lines[i].clone())));
                            i += 1;
                        }
                    }
                }
            } else {
                rows.extend(lines.into_iter().map(DiffRow::Line));
            }
        };
        let no_hunks = |d: Option<&FileDiff>| -> String {
            let header = d.map_or(&[][..], |d| d.header.as_slice());
            if header.iter().any(|l| l.starts_with("Binary files") || l.starts_with("GIT binary patch")) {
                return t("Binary file: no text diff.").to_string();
            }
            let modes: Vec<&str> = header.iter().filter(|l| l.contains(" mode ")).map(String::as_str).collect();
            if modes.is_empty() { t("No content changes.").to_string() } else { format!("File mode changed ({}).", modes.join(", ")) }
        };
        if g.view == GitView::Changes {
            let all = git::entries(&g.status);
            if all.is_empty() {
                rows.push(DiffRow::Placeholder(if g.status.head.is_some() { t("No uncommitted changes") } else { t("No commits yet") }.to_string()));
            }
            for e in all {
                let folded = g.folded_files.contains(&e.path);
                rows.push(DiffRow::File { key: e.path.clone(), status: e.status, orig: e.orig.clone(), folded, entry: Some(e.clone()) });
                if folded {
                    continue;
                }
                if e.status == 'U' {
                    conflict_rows(&mut rows, &e.path, g.conflicts.get(&e.path), &g.status.branch);
                    continue;
                }
                let d = g.diffs.get(&e.path).cloned().unwrap_or_default();
                let mut left = limit;
                let mut shown = false;
                if let Some(staged) = &d.staged {
                    let header = Rc::new(staged.header.clone());
                    for h in &staged.hunks {
                        if left == 0 {
                            break;
                        }
                        push_hunk(&mut rows, h, e.unstaged.is_some().then_some("Staged"), HunkActions::Unstage { header: header.clone(), hunk: Rc::new(h.clone()) }, &mut left);
                        shown = true;
                    }
                }
                if let Some(unstaged) = &d.unstaged {
                    let header = Rc::new(unstaged.header.clone());
                    for h in &unstaged.hunks {
                        if left == 0 {
                            break;
                        }
                        let actions = if e.status == '?' { HunkActions::None } else { HunkActions::Stage { header: header.clone(), hunk: Rc::new(h.clone()) } };
                        push_hunk(&mut rows, h, None, actions, &mut left);
                        shown = true;
                    }
                }
                if left == 0 {
                    rows.push(DiffRow::ShowAll);
                } else if !shown {
                    let text = if d.staged.is_some() || d.unstaged.is_some() {
                        no_hunks(d.staged.as_ref().or(d.unstaged.as_ref()))
                    } else if e.status == '?' {
                        t("Too many new files to show them all. Stage or open this one.").to_string()
                    } else {
                        t("No content changes.").to_string()
                    };
                    rows.push(DiffRow::Note(text));
                }
            }
        } else {
            match (&g.selected, &g.detail) {
                (None, _) => rows.push(DiffRow::Placeholder(t("Select a commit to view its changes").to_string())),
                (Some(_), None) => rows.push(DiffRow::Placeholder(t("Loading…").to_string())),
                (Some(_), Some(d)) => {
                    if d.files.is_empty() {
                        rows.push(DiffRow::Placeholder(t("No changes").to_string()));
                    }
                    for f in &d.files {
                        let folded = g.folded_files.contains(&f.path);
                        rows.push(DiffRow::File { key: f.path.clone(), status: f.status, orig: f.orig.clone(), folded, entry: None });
                        if folded {
                            continue;
                        }
                        let part = d.diffs.get(&f.path);
                        match part.filter(|p| !p.hunks.is_empty()) {
                            None => rows.push(DiffRow::Note(no_hunks(part))),
                            Some(p) => {
                                let mut left = limit;
                                for h in &p.hunks {
                                    if left == 0 {
                                        rows.push(DiffRow::ShowAll);
                                        break;
                                    }
                                    push_hunk(&mut rows, h, None, HunkActions::None, &mut left);
                                }
                            }
                        }
                    }
                }
            }
        }
        g.diff_rows = Rc::new(rows);
    }

    // ---------- menus ----------

    pub(super) fn sync_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let has_upstream = self.current_branch().is_some_and(|b| !b.upstream.is_empty());
        let mut items = vec![
            entry(t("Fetch"), |app, _, cx| app.fetch_all(cx)),
            entry(t("Pull"), |app, _, cx| app.pull(&[], cx)),
            entry(t("Pull (rebase)"), |app, _, cx| app.pull(&["--rebase"], cx)),
            entry(if has_upstream { t("Push") } else { t("Publish") }, |app, window, cx| app.push(window, cx)),
            entry(t("Force push"), |app, window, cx| app.force_push(window, cx)).danger(),
        ];
        if g.refs.remotes.len() > 1 {
            let branch = g.status.branch.clone();
            for (i, r) in g.refs.remotes.iter().enumerate() {
                let (fetch_r, push_r, b) = (r.clone(), r.clone(), branch.clone());
                let fetch = entry(format!("Fetch from {r}"), move |app, _, cx| app.git_op(format!("Fetch {fetch_r}"), vec![step(&["fetch", "--prune", &fetch_r])], false, None, cx));
                items.push(if i == 0 { fetch.separated() } else { fetch });
                if !branch.is_empty() {
                    items.push(entry(format!("Push to {r}/{branch}"), move |app, _, cx| app.git_op(format!("Push to {push_r}"), vec![step(&["push", "-u", &push_r, &b])], false, None, cx)));
                }
            }
        }
        self.open_menu(at, items, cx);
    }

    pub(super) fn more_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let all = git::entries(&g.status);
        let tracked = all.iter().filter(|e| e.status != '?').count();
        let untracked = all.len() - tracked;
        let (by_tracked, head, root) = (g.by_tracked, g.status.head.clone(), g.root.clone());
        let items = vec![
            entry(t("Stage all"), |app, _, cx| {
                let list = app.git.as_ref().map_or(Vec::new(), |g| g.status.unstaged.clone());
                app.stage(&list, true, cx);
            }),
            entry(t("Unstage all"), |app, _, cx| {
                let list = app.git.as_ref().map_or(Vec::new(), |g| g.status.staged.clone());
                app.unstage(&list, true, cx);
            }),
            entry(t("Stash all"), |app, window, cx| app.stash_changes("all", window, cx)).separated(),
            entry(if by_tracked { t("Stash tracked") } else { t("Stash staged") }, move |app, window, cx| app.stash_changes(if by_tracked { "tracked" } else { "staged" }, window, cx)),
            entry(t("View stashes…"), move |app, window, cx| app.open_stash_picker(at, window, cx)),
            entry(t("Apply latest stash"), |app, _, cx| app.git_op("Apply stash", vec![step(&["stash", "apply", "--index"])], false, None, cx)),
            entry(t("Pop latest stash"), |app, _, cx| app.git_op("Pop stash", vec![step(&["stash", "pop", "--index"])], false, None, cx)),
            entry(t("Merge a branch…"), |app, window, cx| app.merge_dialog(window, cx)).separated(),
            entry(t("Create tag at HEAD…"), move |app, window, cx| app.new_tag(head.clone(), window, cx)),
            entry(t("Open repository folder"), move |_, _, cx| cx.open_with_system(&root)).separated(),
            entry(t("Refresh"), |app, _, cx| app.git_reload(true, cx)),
            entry(format!("{} ({tracked})", t("Restore tracked files")), |app, window, cx| app.restore_tracked(window, cx)).separated().danger(),
            entry(format!("{} ({untracked})", t("Move untracked files to trash")), |app, window, cx| app.trash_untracked(window, cx)).danger(),
        ];
        self.open_menu(at, items, cx);
    }

    pub(super) fn view_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let mark = |on: bool, text: &'static str| if on { format!("✓ {}", t(text)) } else { t(text).to_string() };
        let items = vec![
            entry(mark(!g.by_tracked, "Group by staged and unstaged"), |app, _, cx| app.set_git_list_option(None, Some(false), cx)),
            entry(mark(g.by_tracked, "Group by tracked and untracked"), |app, _, cx| app.set_git_list_option(None, Some(true), cx)),
            entry(mark(!g.tree, "Show as a list"), |app, _, cx| app.set_git_list_option(Some(false), None, cx)).separated(),
            entry(mark(g.tree, "Show as a tree"), |app, _, cx| app.set_git_list_option(Some(true), None, cx)),
        ];
        self.open_menu(at, items, cx);
    }

    pub(super) fn commit_options_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let mark = |on: bool, text: &'static str| if on { format!("✓ {}", t(text)) } else { t(text).to_string() };
        let items = vec![
            entry(mark(g.amend, "Amend the last commit"), |app, window, cx| app.toggle_amend(window, cx)),
            entry(mark(g.signoff, "Signoff"), |app, _, _| {
                if let Some(g) = app.git.as_mut() {
                    g.signoff = !g.signoff;
                }
            }),
            entry(mark(g.no_verify, "Skip hooks"), |app, _, _| {
                if let Some(g) = app.git.as_mut() {
                    g.no_verify = !g.no_verify;
                }
            }),
            entry(mark(g.push_after, "Push after committing"), |app, _, _| {
                if let Some(g) = app.git.as_mut() {
                    g.push_after = !g.push_after;
                }
            }),
        ];
        self.open_menu(at, items, cx);
    }

    pub(super) fn file_menu(&mut self, e: Entry, list: Vec<Entry>, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let picked: Vec<&Entry> = list.iter().filter(|x| g.picked.contains(&x.path)).collect();
        let many = picked.len() > 1;
        let unstaged: Vec<Change> = picked.iter().filter_map(|x| x.unstaged.clone()).collect();
        let staged: Vec<Change> = picked.iter().filter_map(|x| x.staged.clone()).collect();
        let abs = g.root.join(e.path.replace('/', "\\"));
        let mut items = Vec::new();
        if !unstaged.is_empty() {
            let label = if many { format!("Stage {} files", unstaged.len()) } else if e.status == 'U' { t("Mark resolved").to_string() } else { t("Stage").to_string() };
            let list = unstaged.clone();
            items.push(entry(label, move |app, _, cx| app.stage(&list, false, cx)));
        }
        if !staged.is_empty() {
            let label = if many { format!("Unstage {} files", staged.len()) } else { t("Unstage").to_string() };
            let list = staged.clone();
            items.push(entry(label, move |app, _, cx| app.unstage(&list, false, cx)));
        }
        let can_restore: Vec<Change> = unstaged.iter().filter(|f| f.status != 'U').cloned().collect();
        if !can_restore.is_empty() {
            let label = if many { format!("Restore {} files", unstaged.len()) } else { t("Restore").to_string() };
            items.push(entry(label, move |app, window, cx| app.discard(can_restore.clone(), window, cx)).danger());
        }
        if !many {
            let (view, reveal) = (abs.clone(), abs);
            items.push(entry(t("View file"), move |_, _, cx| cx.open_with_system(&view)).separated());
            let path = e.path.clone();
            items.push(entry(t("View file history"), move |app, _, cx| app.open_file_history(Some(path.clone()), cx)));
            if e.status == '?' {
                let path = e.path.clone();
                items.push(entry(t("Ignore this file"), move |app, _, cx| app.ignore_path(&path, cx)));
                let dir = split_path(&e.path).0.to_string();
                if !dir.is_empty() {
                    items.push(entry(format!("Ignore everything in {dir}/"), move |app, _, cx| app.ignore_path(&format!("{dir}/"), cx)));
                }
            } else {
                let (open, copy) = (e.path.clone(), e.path.clone());
                items.push(entry(t("Open file permalink"), move |app, _, cx| app.file_permalink(&open, true, cx)));
                items.push(entry(t("Copy file permalink"), move |app, _, cx| app.file_permalink(&copy, false, cx)));
            }
            items.push(entry(t("Show in folder"), move |_, _, cx| cx.reveal_path(&reveal)));
            let path = e.path.clone();
            items.push(entry(t("Copy path"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.clone()))));
        }
        self.open_menu(at, items, cx);
    }

    pub(super) fn commit_file_menu(&mut self, f: Change, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let Some(hash) = g.detail.as_ref().map(|d| d.hash.clone()) else { return };
        let abs = g.root.join(f.path.replace('/', "\\"));
        let (view, reveal, history, copy, restore) = (abs.clone(), abs, f.path.clone(), f.path.clone(), f.path);
        let items = vec![
            entry(t("View current version"), move |_, _, cx| cx.open_with_system(&view)),
            entry(t("View file history"), move |app, _, cx| app.open_file_history(Some(history.clone()), cx)),
            entry(t("Show in folder"), move |_, _, cx| cx.reveal_path(&reveal)),
            entry(t("Copy path"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
            entry(t("Restore this version to the working copy"), move |app, window, cx| {
                let (path, hash) = (restore.clone(), hash.clone());
                app.ask(t("Restore file"), format!("Replace {path} in the working copy with its version from {}?", short(&hash)), t("Restore"), true, move |app, _, cx| app.git_op("Restore file", vec![step(&["checkout", &hash, "--", &path])], false, None, cx), window, cx);
            })
            .separated(),
        ];
        self.open_menu(at, items, cx);
    }

    pub(super) fn branch_menu(&mut self, b: LocalBranch, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let current_name = g.status.branch.clone();
        let on = if current_name.is_empty() { "HEAD".to_string() } else { current_name.clone() };
        let current = b.name == current_name;
        let mut items = Vec::new();
        if !current {
            let (co, mg, rb) = (b.name.clone(), b.name.clone(), b.name.clone());
            items.push(entry(format!("Check out {}", b.name), move |app, _, cx| app.checkout_branch(&co, cx)));
            items.push(entry(format!("Merge {} into {on}", b.name), move |app, _, cx| app.merge_branch(&mg, false, cx)));
            items.push(entry(format!("Rebase {on} onto {}", b.name), move |app, window, cx| app.rebase_onto(rb.clone(), window, cx)));
        }
        if !g.refs.remotes.is_empty() {
            let label = if b.upstream.is_empty() { t("Push to remote…").to_string() } else { format!("Push to {}", b.upstream) };
            let target = b.clone();
            let item = entry(label, move |app, window, cx| if current { app.push(window, cx) } else { app.push_branch(target.clone(), window, cx) });
            items.push(if current { item } else { item.separated() });
        }
        let (at_hash, rename, copy, delete) = (b.hash.clone(), b.clone(), b.name.clone(), b);
        items.push(entry(t("New branch from here…"), move |app, window, cx| app.new_branch(Some(at_hash.clone()), window, cx)));
        items.push(entry(t("Rename…"), move |app, window, cx| {
            let old = rename.name.clone();
            app.open_form(t("Rename branch"), None, vec![Spec::Text { id: "name", label: "New name", value: old.clone(), placeholder: "", required: true }], t("Rename"), false, move |app, v, _, cx| {
                let name = v.text("name");
                if name != old {
                    app.git_op("Rename branch", vec![step(&["branch", "-m", &old, &name])], false, None, cx);
                }
            }, window, cx);
        }));
        items.push(entry(t("Copy name"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))));
        if !current {
            items.push(entry(t("Delete…"), move |app, window, cx| {
                let name = delete.name.clone();
                app.open_form(t("Delete branch"), Some(format!("Delete the local branch {name}?")), vec![Spec::Check { id: "force", label: t("Delete even if it has commits that aren't merged anywhere").to_string(), value: false }], t("Delete"), true, move |app, v, _, cx| {
                    let flag = if v.on("force") { "-D" } else { "-d" };
                    app.git_op(format!("Delete {name}"), vec![step(&["branch", flag, &name])], false, None, cx);
                }, window, cx);
            }).danger());
        }
        self.open_menu(at, items, cx);
    }

    fn push_branch(&mut self, b: LocalBranch, window: &mut Window, cx: &mut Context<Self>) {
        if !b.upstream.is_empty() && !b.gone {
            let (remote, rest) = b.upstream.split_once('/').unwrap_or((&b.upstream, ""));
            let spec = format!("{}:{rest}", b.name);
            self.git_op(format!("Push {}", b.name), vec![step(&["push", remote, &spec])], false, None, cx);
            return;
        }
        let Some(g) = &self.git else { return };
        let remotes = g.refs.remotes.clone();
        let value = if remotes.iter().any(|r| r == "origin") { "origin".to_string() } else { remotes.first().cloned().unwrap_or_default() };
        let name = b.name;
        self.open_form(format!("Push {name}"), None, vec![Spec::Select { id: "remote", label: "Remote", options: remotes, value }], t("Push"), false, move |app, v, _, cx| {
            let remote = v.text("remote");
            app.git_op(format!("Push {name}"), vec![step(&["push", "-u", &remote, &name])], false, None, cx);
        }, window, cx);
    }

    pub(super) fn remote_branch_menu(&mut self, b: RemoteBranch, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let on = self.git.as_ref().map_or("HEAD".to_string(), |g| if g.status.branch.is_empty() { "HEAD".to_string() } else { g.status.branch.clone() });
        let (co, mg, rb, copy, del) = (b.clone(), b.name.clone(), b.name.clone(), b.name.clone(), b.clone());
        let items = vec![
            entry(t("Check out as local branch…"), move |app, window, cx| app.checkout_remote(co.clone(), window, cx)),
            entry(format!("Merge {} into {on}", b.name), move |app, _, cx| app.merge_branch(&mg, false, cx)),
            entry(format!("Rebase {on} onto {}", b.name), move |app, window, cx| app.rebase_onto(rb.clone(), window, cx)),
            entry(t("Copy name"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
            entry(format!("Delete {} on the remote…", b.name), move |app, window, cx| {
                let short_name = del.name[del.remote.len() + 1..].to_string();
                let (remote, full) = (del.remote.clone(), del.name.clone());
                app.ask(t("Delete remote branch"), format!("Delete {short_name} from {remote}? This affects everyone using the remote."), t("Delete"), true, move |app, _, cx| app.git_op(format!("Delete {full}"), vec![step(&["push", &remote, "--delete", &short_name])], false, None, cx), window, cx);
            }).danger(),
        ];
        self.open_menu(at, items, cx);
    }

    pub(super) fn tag_menu(&mut self, tag: Tag, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let (co, branch_at) = (tag.name.clone(), tag.hash.clone());
        let mut items = vec![
            entry(format!("Check out {}", tag.name), move |app, window, cx| app.checkout_detached(co.clone(), window, cx)),
            entry(t("New branch from here…"), move |app, window, cx| app.new_branch(Some(branch_at.clone()), window, cx)),
        ];
        for r in &g.refs.remotes {
            let (remote, name) = (r.clone(), tag.name.clone());
            items.push(entry(format!("Push to {r}"), move |app, _, cx| app.git_op(format!("Push {name}"), vec![step(&["push", &remote, &format!("refs/tags/{name}")])], false, None, cx)));
        }
        let (copy, del) = (tag.name.clone(), tag.name);
        items.push(entry(t("Copy name"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))));
        items.push(entry(t("Delete…"), move |app, window, cx| {
            let name = del.clone();
            app.ask(t("Delete tag"), format!("Delete the local tag {name}?"), t("Delete"), true, move |app, _, cx| app.git_op(format!("Delete tag {name}"), vec![step(&["tag", "-d", &name])], false, None, cx), window, cx);
        }).danger());
        self.open_menu(at, items, cx);
    }

    pub(super) fn ref_menu(&mut self, kind: RefKind, name: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        match kind {
            RefKind::Tag => {
                if let Some(tag) = g.refs.tags.iter().find(|x| x.name == name).cloned() {
                    self.tag_menu(tag, at, cx);
                }
            }
            RefKind::Remote => {
                if let Some(b) = g.refs.remote_branches.iter().find(|x| x.name == name).cloned() {
                    self.remote_branch_menu(b, at, cx);
                }
            }
            _ => {
                if let Some(b) = g.refs.locals.iter().find(|x| x.name == name).cloned() {
                    self.branch_menu(b, at, cx);
                }
            }
        }
    }

    pub(super) fn commit_menu(&mut self, c: Commit, at: Point<Pixels>, cx: &mut Context<Self>) {
        use overlays::entry;
        let Some(g) = &self.git else { return };
        let on = if g.status.branch.is_empty() { "HEAD".to_string() } else { g.status.branch.clone() };
        let is_head = g.status.head.as_deref() == Some(c.hash.as_str());
        let on_branch = !g.status.branch.is_empty();
        let (co, branch_here, tag_here) = (c.hash.clone(), c.hash.clone(), c.hash.clone());
        let mut items = vec![
            entry(t("Check out this commit…"), move |app, window, cx| app.checkout_detached(co.clone(), window, cx)),
            entry(t("New branch here…"), move |app, window, cx| app.new_branch(Some(branch_here.clone()), window, cx)),
            entry(t("New tag here…"), move |app, window, cx| app.new_tag(Some(tag_here.clone()), window, cx)),
        ];
        if !is_head {
            let (mg, rb, pick) = (c.hash.clone(), c.hash.clone(), c.clone());
            let target = on.clone();
            items.push(entry(format!("Merge into {on}"), move |app, _, cx| app.merge_branch(&mg, false, cx)).separated());
            items.push(entry(format!("Rebase {on} onto this commit"), move |app, window, cx| app.rebase_onto(rb.clone(), window, cx)));
            items.push(entry(format!("Cherry-pick onto {on}"), move |app, window, cx| {
                let hash = pick.hash.clone();
                let merge = pick.parents.len() > 1;
                app.ask(t("Cherry-pick"), format!("Apply the changes of {} \"{}\" as a new commit on {target}?", short(&hash), pick.subject), t("Cherry-pick"), false, move |app, _, cx| {
                    let mut args = vec!["cherry-pick"];
                    if merge {
                        args.extend(["-m", "1"]);
                    }
                    args.push(&hash);
                    app.git_op("Cherry-pick", vec![step(&args)], false, None, cx);
                }, window, cx);
            }));
        }
        let revert = c.clone();
        let item = entry(t("Revert this commit…"), move |app, window, cx| {
            let hash = revert.hash.clone();
            let merge = revert.parents.len() > 1;
            app.ask(t("Revert"), format!("Create a new commit that undoes {} \"{}\"?", short(&hash), revert.subject), t("Revert"), false, move |app, _, cx| {
                let mut args = vec!["revert", "--no-edit"];
                if merge {
                    args.extend(["-m", "1"]);
                }
                args.push(&hash);
                app.git_op("Revert", vec![step(&args)], false, None, cx);
            }, window, cx);
        });
        items.push(if is_head { item.separated() } else { item });
        if on_branch && !is_head {
            let target = c.clone();
            items.push(entry(format!("Reset {on} to this commit…"), move |app, window, cx| app.reset_to(target.clone(), window, cx)));
        }
        let (hash, subject) = (c.hash, c.subject);
        items.push(entry(t("Copy commit hash"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(hash.clone()))).separated());
        items.push(entry(t("Copy message"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(subject.clone()))));
        self.open_menu(at, items, cx);
    }
}

fn conflict_label(label: &str, branch: &str) -> String {
    if (label.eq_ignore_ascii_case("HEAD") || label.eq_ignore_ascii_case("Updated upstream")) && !branch.is_empty() {
        return branch.to_string();
    }
    if label.is_empty() { "this side".to_string() } else { label.to_string() }
}

fn conflict_rows(rows: &mut Vec<DiffRow>, path: &str, text: Option<&String>, branch: &str) {
    let Some(text) = text else {
        rows.push(DiffRow::Note(t("This file can't be shown.").to_string()));
        return;
    };
    let parts = git::parse_conflicts(text);
    if !parts.iter().any(|p| matches!(p, ConflictPart::Conflict { .. })) {
        rows.push(DiffRow::ConflictDone { path: path.to_string() });
        return;
    }
    let count = parts.len();
    for (i, part) in parts.into_iter().enumerate() {
        match part {
            ConflictPart::Plain(plain) => {
                // Around a conflict, a few lines of what's unchanged.
                let lines: Vec<&str> = plain.lines().collect();
                let keep: Vec<String> = if lines.len() <= 6 {
                    lines.iter().map(|l| (*l).to_string()).collect()
                } else {
                    let mut k = Vec::new();
                    if i != 0 {
                        k.extend(lines[..3].iter().map(|l| (*l).to_string()));
                    }
                    k.push("…".to_string());
                    if i != count - 1 {
                        k.extend(lines[lines.len() - 3..].iter().map(|l| (*l).to_string()));
                    }
                    k
                };
                rows.extend(keep.into_iter().map(DiffRow::ConflictContext));
            }
            ConflictPart::Conflict { ours_label, theirs_label, ours, theirs, .. } => {
                let (ours_name, theirs_name) = (conflict_label(&ours_label, branch), conflict_label(&theirs_label, branch));
                rows.push(DiffRow::Conflict { path: path.to_string(), index: i, ours: ours_name.clone(), theirs: theirs_name.clone() });
                rows.push(DiffRow::ConflictSide { label: ours_name, text: String::new(), ours: true });
                rows.extend(ours.lines().map(|l| DiffRow::ConflictSide { label: String::new(), text: l.to_string(), ours: true }));
                rows.push(DiffRow::ConflictSide { label: theirs_name, text: String::new(), ours: false });
                rows.extend(theirs.lines().map(|l| DiffRow::ConflictSide { label: String::new(), text: l.to_string(), ours: false }));
            }
        }
    }
}
