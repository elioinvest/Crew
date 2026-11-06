//! The sidebar's right-click menus: a project's and a session's, with renaming and deleting.

use gpui::{ClipboardItem, Focusable, MouseDownEvent, Point, Subscription};

use super::overlays::{MenuEntry, ToastKind, entry};
use super::*;
use crew_core::composer;

pub(super) struct Renaming {
    pub(super) id: String,
    pub(super) agent: String,
    pub(super) input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl CrewApp {
    pub(super) fn copy_text(&self, text: impl Into<String>, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
    }

    pub(super) fn project_menu(&mut self, path: PathBuf, at: Point<Pixels>, cx: &mut Context<Self>) {
        let mut items = self.new_session_entries(&path);
        let any_provider = !self.providers.is_empty();
        let (up, down, top) = (path.clone(), path.clone(), path.clone());
        let move_up = entry("Move up", move |app, _, cx| app.shift_project(&up, -1, cx));
        items.push(if any_provider { move_up.separated() } else { move_up });
        items.push(entry("Move down", move |app, _, cx| app.shift_project(&down, 1, cx)));
        items.push(entry("Move to top", move |app, _, cx| app.shift_project(&top, isize::MIN, cx)));
        let (git, folder, copy, hide) = (path.clone(), path.clone(), path.clone(), path);
        items.push(entry("Open in Git", move |app, window, cx| app.open_git(git.clone(), window, cx)));
        items.push(entry("Open folder", move |_, _, cx| cx.open_with_system(&folder)));
        items.push(entry("Copy path", move |app, _, cx| app.copy_text(copy.to_string_lossy(), cx)));
        let owner = format!("project:{}", hide.display());
        items.push(entry("Hide from list", move |app, _, cx| app.hide_project(&hide, cx)).danger());
        self.open_menu(at, items, cx);
        self.set_menu_owner(owner);
    }

    fn shift_project(&mut self, path: &Path, delta: isize, cx: &mut Context<Self>) {
        let mut paths: Vec<PathBuf> = self.projects.iter().map(|p| p.path.clone()).collect();
        let Some(i) = paths.iter().position(|p| same_folder(p, path)) else { return };
        let j = i.saturating_add_signed(delta).min(paths.len() - 1);
        if i == j {
            return;
        }
        let moved = paths.remove(i);
        paths.insert(j, moved);
        let order: Vec<String> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
        self.save_setting("projectOrder", json!(order));
        self.rescan(cx);
    }

    fn hide_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        let extra: Vec<String> = self.config.list("extraProjects").into_iter().filter(|p| !same_folder(Path::new(p), path)).collect();
        let mut hidden = self.config.list("hiddenProjects");
        if !hidden.iter().any(|p| same_folder(Path::new(p), path)) {
            hidden.push(path.to_string_lossy().to_string());
        }
        self.save_setting("extraProjects", json!(extra));
        self.save_setting("hiddenProjects", json!(hidden));
        self.rescan(cx);
    }

    pub(super) fn session_menu(&mut self, session: Session, project: PathBuf, at: Point<Pixels>, cx: &mut Context<Self>) {
        let pending = session.file.as_os_str().is_empty();
        let open_key = self.open.iter().find(|o| o.id == session.id && o.agent == session.agent).map(|o| o.key);
        let mut items: Vec<MenuEntry> = Vec::new();
        let (s, p) = (session.clone(), project.clone());
        items.push(entry("Open", move |app, window, cx| app.open_session(&s, &p, window, cx)));
        if let Some(w) = session.worktree.as_ref().filter(|w| w.archived_id.is_none() && !pending) {
            let (cwd, path, name) = (project.clone(), w.path.clone(), w.name.clone());
            items.push(entry(format!("{} {}…", t("Archive worktree"), w.name), move |app, window, cx| app.archive_worktree(cwd.clone(), path.clone(), name.clone(), window, cx)));
        }
        for (label, dir) in [("Open in split right", Dir::Row), ("Open in split down", Dir::Column)] {
            let (s, p) = (session.clone(), project.clone());
            items.push(entry(label, move |app, window, cx| app.open_beside(&s, &p, dir, window, cx)));
        }
        if let Some(key) = open_key {
            items.push(entry("Restart", move |app, window, cx| {
                app.show(key, window, cx);
                app.restart_active(window, cx);
            }));
            items.push(entry("Close", move |app, window, cx| app.close_session(key, window, cx)).danger());
        }
        if !pending {
            let s = session.clone();
            items.push(entry("Rename", move |app, window, cx| app.start_rename(&s, window, cx)));
        }
        let owner = format!("session:{}", session.id);
        let id = session.id.clone();
        items.push(entry("Copy session ID", move |app, _, cx| app.copy_text(id.clone(), cx)));
        if !pending {
            let s = session;
            items.push(entry("Delete", move |app, _, cx| app.ask_delete_session(&s, cx)).danger());
        }
        self.open_menu(at, items, cx);
        self.set_menu_owner(owner);
    }

    fn open_beside(&mut self, session: &Session, project: &Path, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        // Move it rather than focus where it already is.
        if let Some(key) = self.open.iter().find(|o| o.id == session.id && o.agent == session.agent).map(|o| o.key) {
            self.layout.forget(key);
        }
        self.split(dir, window, cx);
        self.open_session(session, project, window, cx);
    }

    pub(super) fn start_rename(&mut self, session: &Session, window: &mut Window, cx: &mut Context<Self>) {
        let title = session.title.clone().unwrap_or_default();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(title, window, cx);
            state
        });
        let events = cx.subscribe_in(&input, window, |app, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                app.finish_rename(true, window, cx);
            }
        });
        let handle = input.read(cx).focus_handle(cx);
        let blur = cx.on_blur(&handle, window, |app, window, cx| app.finish_rename(true, window, cx));
        let select = cx.on_focus(&handle, window, |_, window, cx| window.dispatch_action(Box::new(gpui_component::input::SelectAll), cx));
        input.update(cx, |s, cx| s.focus(window, cx));
        self.renaming = Some(Renaming { id: session.id.clone(), agent: session.agent.clone(), input, _subscriptions: vec![events, blur, select] });
        cx.notify();
    }

    fn finish_rename(&mut self, save: bool, _: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.renaming.take() else { return };
        cx.notify();
        let title = r.input.read(cx).value().trim().to_string();
        if !save || title.is_empty() {
            return;
        }
        let homes = providers::homes(&self.paths);
        if !composer::rename_session(&self.paths, &homes, &r.id, &title, &r.agent) {
            self.toast(t("Couldn't rename the session."), ToastKind::Error, cx);
            return;
        }
        // Show it right away; the next scan confirms it.
        for s in self.projects.iter_mut().flat_map(|p| p.sessions.iter_mut()).filter(|s| s.id == r.id) {
            s.title = Some(title.clone());
        }
        if let Some(o) = self.open.iter_mut().find(|o| o.id == r.id) {
            o.title = Some(title);
        }
        self.rescan(cx);
    }

    fn ask_delete_session(&mut self, session: &Session, cx: &mut Context<Self>) {
        let name = session.title.clone().unwrap_or_else(|| session.id.clone());
        let question = if crate::i18n::zh() { format!("删除“{name}”？") } else { format!("Delete \"{name}\"?") };
        let detail = if session.agent == "codex" {
            t("The Codex session is permanently deleted with `codex delete`. This cannot be undone. If the session is running, it will be stopped.")
        } else {
            t("The session file is moved to the Recycle Bin. If the session is running, it will be stopped.")
        };
        let (id, agent) = (session.id.clone(), session.agent.clone());
        self.confirm(t("Delete session"), format!("{question} {detail}"), t("Delete"), move |app, window, cx| app.delete_session(id.clone(), agent.clone(), window, cx), cx);
    }

    fn delete_session(&mut self, id: String, agent: String, window: &mut Window, cx: &mut Context<Self>) {
        // Stop a running agent first so it doesn't recreate the file.
        let running = self.open.iter().find(|o| o.id == id && o.agent == agent).map(|o| o.key);
        if let Some(key) = running {
            self.close_session(key, window, cx);
        }
        let (paths, command) = (self.paths.clone(), self.config.str("codexCommand").to_string());
        let work = cx.background_spawn(async move {
            let homes = providers::homes(&paths);
            composer::delete_session(&paths, &homes, &id, &agent, &command, running.is_some())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |app, cx| {
                if let Err(why) = result {
                    app.toast(why, ToastKind::Error, cx);
                }
                app.rescan(cx);
            });
        })
        .detach();
    }

    /// The row a session being renamed shows instead of its card.
    pub(super) fn render_rename_row(&self, session_id: &str) -> Option<AnyElement> {
        let r = self.renaming.as_ref().filter(|r| r.id == session_id)?;
        Some(
            div()
                .my(px(2.))
                .py(px(4.))
                .pl(px(SESSION_INDENT + 6.))
                .pr(px(6.))
                .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| cx.stop_propagation())
                .child(Input::new(&r.input).small())
                .into_any_element(),
        )
    }
}
