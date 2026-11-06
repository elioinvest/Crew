//! The worktree, branch and stash pickers, on the Git panel and next to the project chip under the
//! prompt box. Pick or type to create a worktree: it starts on a detached HEAD, and the branch picker
//! then creates or checks out the branch to use there. A session belongs to one folder, so "switching"
//! shows that worktree's session or starts one.

use crew_core::git;
use crew_core::worktrees::{self, Archived, RemoveError, Worktree, WorktreeList};
use gpui::Point;

use super::forms::{PickButton, PickItem, PickRow, PickRows};
use super::git_panel::{BarKind, fmt_date, step};
use super::overlays::{Action, ToastKind};
use super::*;
use crate::i18n::zh;

fn action(f: impl Fn(&mut CrewApp, &mut Window, &mut Context<CrewApp>) + 'static) -> Action {
    Rc::new(f)
}

#[derive(Clone)]
struct BranchInfo {
    name: String,
    hash: String,
    upstream: String,
    track: String,
    when: String,
    remote: String,
}

impl CrewApp {
    pub(super) fn worktree_error(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        if self.git.is_some() {
            self.git_bar(text, BarKind::Error, cx);
        } else {
            self.toast(text.into(), ToastKind::Error, cx);
        }
    }

    fn sessions_in(&self, folder: &Path) -> Vec<u64> {
        let text = folder_text(folder);
        self.open.iter().filter(|o| !o.exited && folder_text(&o.cwd) == text).map(|o| o.key).collect()
    }

    /// Shows the worktree: its running session, its latest session, or a new one with the same agent.
    fn switch_to_worktree(&mut self, path: PathBuf, main: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.sessions_in(&path).first().copied() {
            self.show(key, window, cx);
            return;
        }
        let (agent, provider) = self.active().map_or((None, None), |o| ((o.agent == "claude" || o.agent == "codex").then(|| o.agent.clone()), o.provider.clone()));
        let text = folder_text(&path);
        let past = self.projects.iter().flat_map(|p| p.sessions.iter()).find(|s| s.cwd.as_deref().is_some_and(|c| folder_text(c) == text)).cloned();
        if let Some(s) = past {
            self.open_session(&s, &main, window, cx);
            return;
        }
        let agent = agent.unwrap_or_else(|| self.default_agent());
        self.start_new_with(&agent, path, provider, None, window, cx);
    }

    fn create_worktree(&mut self, cwd: PathBuf, name: Option<String>, base: Option<(String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        self.toast(match (&name, zh()) {
            (Some(n), true) => format!("正在创建工作树 {n}…"),
            (Some(n), false) => format!("Creating worktree {n}…"),
            (None, true) => "正在创建工作树…".to_string(),
            (None, false) => "Creating worktree…".to_string(),
        }, ToastKind::Plain, cx);
        let (paths, setting, setup) = (self.paths.clone(), self.config.str("worktreeDirectory").to_string(), self.config.str("worktreeSetup").trim().to_string());
        let work = cx.background_spawn(async move {
            let parts = base.as_ref().map(|(r, b)| (r.as_str(), b.as_str()));
            worktrees::create_worktree(&paths, &setting, &cwd, name.as_deref(), parts)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |app, window, cx| match result {
                Err(why) => app.worktree_error(why, cx),
                Ok(made) => {
                    match &made.warning {
                        Some(w) => app.worktree_error(w.clone(), cx),
                        None => app.toast(if zh() { format!("已创建工作树 {}", made.name) } else { format!("Worktree {} created", made.name) }, ToastKind::Ok, cx),
                    }
                    app.run_worktree_setup(setup.clone(), made.path.clone(), made.main.clone(), made.name.clone(), cx);
                    app.switch_to_worktree(made.path, made.main, window, cx);
                    app.rescan(cx);
                }
            });
        })
        .detach();
    }

    /// The setup command runs in the background; a failure doesn't undo the worktree.
    fn run_worktree_setup(&mut self, command: String, path: PathBuf, main: PathBuf, name: String, cx: &mut Context<Self>) {
        if command.is_empty() {
            return;
        }
        self.toast(if zh() { format!("正在 {name} 中运行设置命令…") } else { format!("Running the setup command in {name}…") }, ToastKind::Plain, cx);
        let work = cx.background_spawn(async move { worktrees::run_setup(&command, &path, &main) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |app, cx| match result {
                Ok(()) => app.toast(if zh() { format!("{name} 的设置已完成") } else { format!("Setup finished in {name}") }, ToastKind::Ok, cx),
                Err(output) => app.worktree_error(format!("The setup command failed in {name}. {output}"), cx),
            });
        })
        .detach();
    }

    /// Deleting: refused while a session runs there; git refuses a folder with changes, and then asks.
    fn delete_worktree(&mut self, cwd: PathBuf, w: Worktree, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sessions_in(&w.path).is_empty() {
            self.worktree_error(format!("Close the session running in {} first.", w.name), cx);
            return;
        }
        let setting = self.config.str("worktreeDirectory").to_string();
        match worktrees::remove_worktree(&self.paths, &setting, &cwd, &w.path, force) {
            Err(RemoveError::Dirty) => {
                let (again_cwd, again) = (cwd, w.clone());
                self.ask(
                    t("Delete worktree"),
                    format!("Worktree \"{}\" contains modified or untracked files. Force delete it?", w.name),
                    t("Force delete"),
                    true,
                    move |app, window, cx| app.delete_worktree(again_cwd.clone(), again.clone(), true, window, cx),
                    window,
                    cx,
                );
            }
            Err(RemoveError::Other(why)) => self.worktree_error(why, cx),
            Ok(()) => {
                self.toast(if zh() { format!("已删除工作树 {}", w.name) } else { format!("Worktree {} deleted", w.name) }, ToastKind::Ok, cx);
                self.after_worktree_gone(&w.path, cx);
            }
        }
    }

    pub(super) fn archive_worktree(&mut self, cwd: PathBuf, path: PathBuf, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sessions_in(&path).is_empty() {
            self.worktree_error(format!("Close the session running in {name} first."), cx);
            return;
        }
        self.ask(
            t("Archive worktree"),
            format!("Save \"{name}\" and remove its folder? Staged, unstaged and untracked changes are kept and come back when you restore it. Files Git ignores (such as node_modules or .env) are deleted with the folder."),
            t("Archive"),
            false,
            move |app, _, cx| {
                let setting = app.config.str("worktreeDirectory").to_string();
                match worktrees::archive_worktree(&app.paths, &setting, &cwd, &path) {
                    Err(why) => app.worktree_error(why, cx),
                    Ok(_) => {
                        app.toast(if zh() { format!("已归档工作树 {name}") } else { format!("Worktree {name} archived") }, ToastKind::Ok, cx);
                        app.after_worktree_gone(&path, cx);
                        app.rescan(cx);
                    }
                }
            },
            window,
            cx,
        );
    }

    /// The Git panel moves off a folder that's gone.
    fn after_worktree_gone(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.git.as_ref().is_some_and(|g| folder_text(&g.root) == folder_text(path)) {
            self.git = None;
        }
        self.refresh_branch(cx);
        cx.notify();
    }

    fn restore_archived(&mut self, a: &Archived, window: &mut Window, cx: &mut Context<Self>) {
        self.toast(if zh() { "正在恢复工作树…" } else { "Restoring worktree…" }, ToastKind::Plain, cx);
        match worktrees::restore_worktree(&self.paths, &a.id) {
            Err(why) => self.worktree_error(why, cx),
            Ok(back) => {
                match &back.note {
                    Some(note) => self.worktree_error(note.clone(), cx),
                    None => self.toast(if zh() { format!("已恢复工作树 {}", back.name) } else { format!("Worktree {} restored", back.name) }, ToastKind::Ok, cx),
                }
                self.rescan(cx);
                self.switch_to_worktree(back.path, back.main, window, cx);
            }
        }
    }

    pub(super) fn open_worktree_picker(&mut self, at: Point<Pixels>, cwd: PathBuf, above: bool, window: &mut Window, cx: &mut Context<Self>) {
        let info = match worktrees::list_worktrees(&self.paths, &cwd) {
            Ok(i) => i,
            Err(why) => return self.worktree_error(why, cx),
        };
        let head = worktrees::head_of(&cwd);
        let current = head.as_ref().and_then(|h| h.branch.clone()).unwrap_or_else(|| "HEAD".to_string());
        let remote_base = info.default_branch.as_ref().and_then(|d| d.split_once('/')).map(|(r, b)| (r.to_string(), b.to_string()));
        let offer_current = remote_base.as_ref().is_none_or(|(_, b)| head.as_ref().and_then(|h| h.branch.as_deref()) != Some(b.as_str()));
        let base_label = info.default_branch.clone().unwrap_or_else(|| current.clone());
        let rows: PickRows = Rc::new(move |app: &CrewApp, q: &str, raw: &str| worktree_rows(app, &info, &cwd, &current, &base_label, remote_base.clone(), offer_current, q, raw));
        self.open_picker(at, above, "Select or type to create a worktree…", rows, window, cx);
    }

    pub(super) fn open_stash_picker(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let rows: PickRows = Rc::new(|app: &CrewApp, q: &str, _| {
            let Some(g) = &app.git else { return Vec::new() };
            let list: Vec<&git::Stash> = g.refs.stashes.iter().filter(|s| q.is_empty() || s.message.to_lowercase().contains(q)).collect();
            if list.is_empty() {
                return vec![PickItem::Note(if g.refs.stashes.is_empty() { t("No stashes yet") } else { t("No matches") }.to_string())];
            }
            list.into_iter()
                .map(|s| {
                    let label = s.message.split_once(": ").filter(|(p, _)| p.starts_with("WIP on") || p.starts_with("On ")).map_or(s.message.as_str(), |x| x.1).to_string();
                    let (show, apply, pop, drop, message) = (s.reference.clone(), s.reference.clone(), s.reference.clone(), s.reference.clone(), s.message.clone());
                    PickItem::Row(PickRow {
                        label: if label.is_empty() { s.reference.clone() } else { label },
                        sub: Some(format!("{} · {}", s.reference, if s.at > 0 { fmt_date(s.at) } else { String::new() })),
                        action: Some(action(move |app, _, cx| {
                            app.set_git_view(super::git_panel::GitView::History, cx);
                            app.open_commit(show.clone(), cx);
                        })),
                        buttons: vec![
                            PickButton { label: t("Apply").to_string(), danger: false, keep: false, action: action(move |app, _, cx| app.git_op(format!("Apply {apply}"), vec![step(&["stash", "apply", "--index", &apply])], false, None, cx)) },
                            PickButton { label: t("Pop").to_string(), danger: false, keep: false, action: action(move |app, _, cx| app.git_op(format!("Pop {pop}"), vec![step(&["stash", "pop", "--index", &pop])], false, None, cx)) },
                            PickButton {
                                label: t("Drop").to_string(),
                                danger: true,
                                keep: false,
                                action: action(move |app, window, cx| {
                                    let reference = drop.clone();
                                    app.ask(t("Drop stash"), format!("Delete {reference} ({message})? This can't be undone."), t("Drop"), true, move |app, _, cx| app.git_op(format!("Drop {reference}"), vec![step(&["stash", "drop", &reference])], false, None, cx), window, cx);
                                }),
                            },
                        ],
                        ..PickRow::default()
                    })
                })
                .collect()
        });
        self.open_picker(at, false, "Find a stash…", rows, window, cx);
    }

    /// In the panel the action shows there; from the chip it runs on its own and reports by toast.
    fn branch_act(&mut self, cwd: &Path, label: String, args: Vec<String>, cx: &mut Context<Self>) {
        if self.git.as_ref().is_some_and(|g| folder_text(&g.root) == folder_text(cwd)) {
            self.git_op(label, vec![(args, None)], false, None, cx);
            return;
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let r = git::run(cwd, &refs);
        if !r.ok() {
            self.toast(format!("{label} failed. {}", r.message()), ToastKind::Error, cx);
        }
        self.refresh_branch(cx);
    }

    fn delete_branch_from_picker(&mut self, cwd: PathBuf, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.ask(
            t("Delete branch"),
            format!("Delete the local branch {name}?"),
            t("Delete"),
            true,
            move |app, window, cx| {
                let r = git::run(&cwd, &["branch", "-d", &name]);
                if r.ok() {
                    if app.git.is_some() {
                        app.git_reload(true, cx);
                    }
                    app.refresh_branch(cx);
                    return;
                }
                let (cwd, name) = (cwd.clone(), name.clone());
                app.ask(t("Delete branch"), format!("{name} has commits that aren't merged anywhere. Delete it anyway?"), t("Delete anyway"), true, move |app, _, cx| {
                    app.branch_act(&cwd, format!("Delete {name}"), vec!["branch".into(), "-D".into(), name.clone()], cx);
                }, window, cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn open_branch_picker(&mut self, at: Point<Pixels>, cwd: PathBuf, above: bool, window: &mut Window, cx: &mut Context<Self>) {
        let refs = git::run(&cwd, &["for-each-ref", "--sort=-committerdate", "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(upstream:short)%00%(upstream:track)%00%(committerdate:relative)", "refs/heads", "refs/remotes", "refs/tags"]);
        let (mut locals, mut remotes, mut tags) = (Vec::new(), Vec::new(), Vec::new());
        for line in refs.stdout.lines().filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\0').collect();
            let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
            let info = BranchInfo { name: get(1), hash: get(2), upstream: get(3), track: get(4), when: get(5), remote: get(1).split('/').next().unwrap_or("").to_string() };
            if get(0).starts_with("refs/heads/") {
                locals.push(info);
            } else if get(0).starts_with("refs/remotes/") {
                if !get(0).ends_with("/HEAD") {
                    remotes.push(info);
                }
            } else {
                tags.push(info);
            }
        }
        let head = worktrees::head_of(&cwd);
        // A branch can only be checked out in one worktree at a time.
        let text = folder_text(&cwd);
        let used_in: HashMap<String, Worktree> = worktrees::list_worktrees(&self.paths, &cwd).map_or(HashMap::new(), |i| i.worktrees.into_iter().filter(|w| folder_text(&w.path) != text).filter_map(|w| w.branch.clone().map(|b| (b, w))).collect());
        let rows: PickRows = Rc::new(move |app: &CrewApp, q: &str, raw: &str| {
            let in_panel = app.git.as_ref().is_some_and(|g| folder_text(&g.root) == folder_text(&cwd));
            let matches = |s: &str| q.is_empty() || s.to_lowercase().contains(q);
            let mut out = Vec::new();
            let current = head.as_ref().and_then(|h| h.branch.clone());
            if !raw.is_empty() && !locals.iter().any(|b| b.name.to_lowercase() == q) {
                let name = raw.split_whitespace().collect::<Vec<_>>().join("-");
                let from = current.clone().or_else(|| head.as_ref().and_then(|h| h.sha.clone())).unwrap_or_else(|| "HEAD".to_string());
                let (c, n) = (cwd.clone(), name.clone());
                out.push(PickItem::Row(PickRow {
                    label: if zh() { format!("创建分支“{name}”") } else { format!("Create branch \"{name}\"") },
                    sub: Some(format!("from {from}")),
                    action: Some(action(move |app, _, cx| app.branch_act(&c, format!("Create branch {n}"), vec!["switch".into(), "-c".into(), n.clone()], cx))),
                    ..PickRow::default()
                }));
            }
            let local: Vec<&BranchInfo> = locals.iter().filter(|b| matches(&b.name)).collect();
            if !local.is_empty() {
                out.push(PickItem::Section(t("Branches").to_string()));
                for b in local {
                    let is_current = current.as_deref() == Some(b.name.as_str());
                    let other = used_in.get(&b.name);
                    let mut sub: Vec<String> = Vec::new();
                    if !b.upstream.is_empty() {
                        sub.push(format!("→ {}{}", b.upstream, if b.track.is_empty() { String::new() } else { format!(" {}", b.track) }));
                    }
                    sub.push(b.when.clone());
                    if let Some(w) = other {
                        sub.push(format!("checked out in {}", w.name));
                    }
                    let (c1, c2, n1, n2, n3) = (cwd.clone(), cwd.clone(), b.name.clone(), b.name.clone(), b.name.clone());
                    out.push(PickItem::Row(PickRow {
                        label: b.name.clone(),
                        sub: Some(sub.join(" • ")),
                        on: is_current,
                        disabled: other.is_some(),
                        action: (!is_current).then(|| action(move |app, _, cx| app.branch_act(&c1, format!("Check out {n1}"), vec!["switch".into(), n1.clone()], cx))),
                        buttons: if is_current || other.is_some() { Vec::new() } else { vec![PickButton { label: t("Delete").to_string(), danger: true, keep: false, action: action(move |app, window, cx| app.delete_branch_from_picker(c2.clone(), n2.clone(), window, cx)) }] },
                        menu: in_panel.then(|| {
                            Rc::new(move |app: &mut CrewApp, at: Point<Pixels>, cx: &mut Context<CrewApp>| {
                                if let Some(b) = app.git.as_ref().and_then(|g| g.refs.locals.iter().find(|x| x.name == n3).cloned()) {
                                    app.branch_menu(b, at, cx);
                                }
                            }) as Rc<dyn Fn(&mut CrewApp, Point<Pixels>, &mut Context<CrewApp>)>
                        }),
                        ..PickRow::default()
                    }));
                }
            }
            let remote: Vec<&BranchInfo> = remotes.iter().filter(|b| matches(&b.name)).take(60).collect();
            if !remote.is_empty() {
                out.push(PickItem::Section(t("Remote branches").to_string()));
                for b in remote {
                    let short_name = b.name[b.remote.len() + 1..].to_string();
                    let has_local = locals.iter().any(|l| l.name == short_name);
                    let (c, full, n3) = (cwd.clone(), b.name.clone(), b.name.clone());
                    out.push(PickItem::Row(PickRow {
                        label: b.name.clone(),
                        sub: Some(b.when.clone()),
                        action: Some(action(move |app, _, cx| {
                            let args: Vec<String> = if has_local { vec!["switch".into(), short_name.clone()] } else { vec!["switch".into(), "-c".into(), short_name.clone(), "--track".into(), full.clone()] };
                            app.branch_act(&c, format!("Check out {short_name}"), args, cx);
                        })),
                        menu: in_panel.then(|| {
                            Rc::new(move |app: &mut CrewApp, at: Point<Pixels>, cx: &mut Context<CrewApp>| {
                                if let Some(b) = app.git.as_ref().and_then(|g| g.refs.remote_branches.iter().find(|x| x.name == n3).cloned()) {
                                    app.remote_branch_menu(b, at, cx);
                                }
                            }) as Rc<dyn Fn(&mut CrewApp, Point<Pixels>, &mut Context<CrewApp>)>
                        }),
                        ..PickRow::default()
                    }));
                }
            }
            if in_panel && !q.is_empty() {
                let tagged: Vec<&BranchInfo> = tags.iter().filter(|t| matches(&t.name)).take(30).collect();
                if !tagged.is_empty() {
                    out.push(PickItem::Section(t("Tags").to_string()));
                    for tag in tagged {
                        let (hash, name) = (tag.hash.clone(), tag.name.clone());
                        out.push(PickItem::Row(PickRow {
                            label: tag.name.clone(),
                            sub: Some(tag.when.clone()),
                            action: Some(action(move |app, _, cx| app.jump_to_commit(hash.clone(), cx))),
                            menu: Some(Rc::new(move |app: &mut CrewApp, at: Point<Pixels>, cx: &mut Context<CrewApp>| {
                                if let Some(g) = app.git.as_ref().and_then(|g| g.refs.tags.iter().find(|x| x.name == name).cloned()) {
                                    app.tag_menu(g, at, cx);
                                }
                            })),
                            ..PickRow::default()
                        }));
                    }
                }
            }
            if out.is_empty() {
                out.push(PickItem::Note(t("No matches").to_string()));
            }
            out
        });
        self.open_picker(at, above, "Select a branch or type to create one…", rows, window, cx);
    }
}

#[allow(clippy::too_many_arguments)]
fn worktree_rows(app: &CrewApp, info: &WorktreeList, cwd: &Path, current: &str, base_label: &str, remote_base: Option<(String, String)>, offer_current: bool, q: &str, raw: &str) -> Vec<PickItem> {
    let zh_on = zh();
    let mut out = Vec::new();
    let matches = |s: &str| q.is_empty() || s.to_lowercase().contains(q);
    let create = |label: String, name: Option<String>, base: Option<(String, String)>, cwd: PathBuf| {
        PickItem::Row(PickRow { label, action: Some(action(move |app, window, cx| app.create_worktree(cwd.clone(), name.clone(), base.clone(), window, cx))), ..PickRow::default() })
    };
    let named = !raw.is_empty() && !info.worktrees.iter().any(|w| w.name.to_lowercase() == q);
    if named {
        let label = |base: &str| if zh_on { format!("基于 {base} 创建“{raw}”") } else { format!("Create \"{raw}\" based on {base}") };
        out.push(create(label(base_label), Some(raw.to_string()), remote_base.clone(), cwd.to_path_buf()));
        if remote_base.is_some() && offer_current {
            out.push(create(label(current), Some(raw.to_string()), None, cwd.to_path_buf()));
        }
    } else if raw.is_empty() {
        let label = |base: &str| if zh_on { format!("基于 {base} 创建新工作树") } else { format!("Create new worktree based on {base}") };
        if remote_base.is_some() {
            out.push(create(label(base_label), None, remote_base.clone(), cwd.to_path_buf()));
        }
        if offer_current {
            out.push(create(label(current), None, None, cwd.to_path_buf()));
        }
    }
    let here = folder_text(cwd);
    let row = |w: &Worktree| {
        let (open_path, main, reveal) = (w.path.clone(), info.main.clone(), w.path.clone());
        let mut buttons = vec![PickButton { label: t("Folder").to_string(), danger: false, keep: false, action: action(move |_, _, cx| cx.open_with_system(&reveal)) }];
        if !w.is_main {
            if w.crew {
                let (c, target) = (cwd.to_path_buf(), w.clone());
                buttons.push(PickButton { label: t("Archive").to_string(), danger: false, keep: false, action: action(move |app, window, cx| app.archive_worktree(c.clone(), target.path.clone(), target.name.clone(), window, cx)) });
            }
            let (c, target) = (cwd.to_path_buf(), w.clone());
            buttons.push(PickButton { label: t("Delete").to_string(), danger: true, keep: false, action: action(move |app, window, cx| app.delete_worktree(c.clone(), target.clone(), false, window, cx)) });
        }
        let sub = [Some(w.branch.clone().unwrap_or_else(|| "detached".to_string())), (!w.sha.is_empty()).then(|| w.sha.chars().take(7).collect()), Some(w.path.to_string_lossy().to_string())];
        PickItem::Row(PickRow {
            label: w.name.clone(),
            sub: Some(sub.into_iter().flatten().collect::<Vec<_>>().join(" • ")),
            right: w.is_main.then(|| t("Main checkout").to_string()),
            on: folder_text(&w.path) == here,
            disabled: w.prunable,
            action: Some(action(move |app, window, cx| app.switch_to_worktree(open_path.clone(), main.clone(), window, cx))),
            buttons,
            ..PickRow::default()
        })
    };
    let visible: Vec<&Worktree> = info.worktrees.iter().filter(|w| matches(&w.name) || matches(w.branch.as_deref().unwrap_or("")) || matches(&w.path.to_string_lossy())).collect();
    let (open, others): (Vec<&Worktree>, Vec<&Worktree>) = visible.into_iter().partition(|w| !app.sessions_in(&w.path).is_empty() || folder_text(&w.path) == here);
    if !open.is_empty() {
        out.push(PickItem::Section(t("Open in this window").to_string()));
        out.extend(open.into_iter().map(&row));
    }
    if !others.is_empty() {
        out.push(PickItem::Section(t("Worktrees").to_string()));
        out.extend(others.into_iter().map(&row));
    }
    let archived: Vec<&Archived> = info.archived.iter().filter(|a| matches(&a.name) || matches(a.branch.as_deref().unwrap_or(""))).collect();
    if !archived.is_empty() {
        out.push(PickItem::Section(t("Archived").to_string()));
        for a in archived {
            let (back, gone, name) = (a.clone(), a.id.clone(), a.name.clone());
            out.push(PickItem::Row(PickRow {
                label: a.name.clone(),
                sub: Some(format!("{} • {}", a.branch.as_deref().unwrap_or("detached"), fmt_date(a.at / 1000))),
                action: Some(action(move |app, window, cx| app.restore_archived(&back, window, cx))),
                buttons: vec![PickButton {
                    label: t("Delete").to_string(),
                    danger: true,
                    keep: false,
                    action: action(move |app, window, cx| {
                        let id = gone.clone();
                        app.ask(t("Delete archive"), format!("Delete the archive of \"{name}\"? Its saved changes are lost."), t("Delete"), true, move |app, _, cx| {
                            worktrees::forget_archived(&app.paths, &id);
                            app.rescan(cx);
                        }, window, cx);
                    }),
                }],
                ..PickRow::default()
            }));
        }
    }
    if out.is_empty() {
        out.push(PickItem::Note(t("No matches").to_string()));
    }
    out
}
