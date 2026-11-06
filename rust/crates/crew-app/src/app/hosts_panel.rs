//! SSH hosts: the list in the sidebar, the host editor, a host's terminal beside an agent, and what
//! Crew asks while an agent's crew-ssh call waits. crew-service holds the connections and the secrets;
//! this side only shows them.

use crew_core::hosts::HostInput;
use crew_core::secrets;
use crew_service::ssh::ActivityItem;
use crew_service::{Answer, Prompt, PromptKind};
use gpui::MouseDownEvent;

use gpui_component::tooltip::Tooltip;
use super::overlays::{MenuEntry, ToastKind, entry};
use super::*;
use crate::i18n::zh;

const TRUST_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) struct HostEditor {
    id: Option<String>,
    title: String,
    name: Entity<InputState>,
    address: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    key_path: Entity<InputState>,
    passphrase: Entity<InputState>,
    password: Entity<InputState>,
    group: Entity<InputState>,
    notes: Entity<TextareaState>,
    auth: String,
    approval: String,
    has_password: bool,
    has_passphrase: bool,
    clear_password: bool,
    clear_passphrase: bool,
    can_seal: bool,
    /// The outcome of the last connection test: the message, and whether it worked.
    status: Option<(String, Option<bool>)>,
    activity: Vec<ActivityItem>,
}

/// A question with its answer boxes, shown one at a time.
pub(super) struct PromptView {
    prompt: Prompt,
    inputs: Vec<Entity<InputState>>,
    remember: bool,
}

fn auth_label(auth: &str) -> &'static str {
    match auth {
        "key" => t("Private key file"),
        "agent" => "ssh-agent",
        "password" => t("Password"),
        _ => t("Automatic (key, ssh-agent or saved password)"),
    }
}

fn approval_label(approval: &str) -> &'static str {
    match approval {
        "all" => t("Ask before every command"),
        "auto" => t("Run everything without asking"),
        _ => t("Ask before anything that could change the host (recommended)"),
    }
}

/// What the agent's trust screen has selected, when there is one on screen.
fn trust_screen_selection(screen: &[String]) -> Option<String> {
    let lower = screen.join("\n").to_lowercase();
    let asks = ["trust this folder", "trust this directory", "trust the files in this folder", "trust the files in this directory", "quick safety check", "do you trust"];
    if !asks.iter().any(|a| lower.contains(a)) {
        return None;
    }
    let selected = screen.iter().find_map(|l| {
        let t = l.trim_start();
        ["❯", "›", ">"].iter().find_map(|m| t.strip_prefix(m)).map(|r| r.trim().to_string())
    });
    Some(selected.unwrap_or_default())
}

/// As the original: the taskbar button flashes until the window is brought forward.
fn flash_if_behind(window: &Window) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{FLASHW_ALL, FLASHW_TIMERNOFG, FLASHWINFO, FlashWindowEx};
    use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if window.is_window_active() {
        return;
    }
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(own) = handle.as_raw() else { return };
    let info = FLASHWINFO { cbSize: size_of::<FLASHWINFO>() as u32, hwnd: HWND(own.hwnd.get() as *mut _), dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG, uCount: 0, dwTimeout: 0 };
    let _ = unsafe { FlashWindowEx(&info) };
}

fn says_yes(option: &str) -> bool {
    let text = option.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ').to_lowercase();
    text.starts_with("yes") || text.starts_with("trust")
}

impl CrewApp {
    pub(super) fn reload_hosts(&mut self, cx: &mut Context<Self>) {
        self.hosts = hosts::list(&self.paths);
        cx.notify();
    }

    pub(super) fn host_by_id(&self, id: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// A host's workspace is listed under the host, not among the projects.
    pub(super) fn is_host_workspace(&self, folder: &Path) -> bool {
        let text = folder_text(folder);
        self.hosts.iter().any(|h| h.workspace.as_deref().is_some_and(|w| folder_text(Path::new(w)) == text))
    }

    pub(super) fn host_of_folder(&self, folder: &Path) -> Option<&Host> {
        let text = folder_text(folder);
        self.hosts.iter().find(|h| h.workspace.as_deref().is_some_and(|w| folder_text(Path::new(w)) == text))
    }

    fn terminal_id(h: &Host) -> String {
        format!("ssh:{}", h.id)
    }

    /// New sessions not on disk yet on top, then the newest.
    pub(super) fn host_sessions(&self, h: &Host) -> Vec<Session> {
        let Some(workspace) = h.workspace.as_deref().filter(|w| !w.is_empty()) else { return Vec::new() };
        let text = folder_text(Path::new(workspace));
        let mut list: Vec<Session> = self
            .open
            .iter()
            .filter(|o| o.agent != "ssh" && !o.on_disk && folder_text(&o.cwd) == text)
            .map(|o| Session {
                id: o.id.clone(),
                agent: o.agent.clone(),
                file: PathBuf::new(),
                cwd: Some(o.cwd.clone()),
                branch: None,
                title: Some(t("New session").into()),
                mtime: 0.,
                size: 0,
                provider: o.provider.clone(),
                worktree: None,
            })
            .collect();
        if let Some(p) = self.projects.iter().find(|p| folder_text(&p.path) == text) {
            list.extend(p.sessions.iter().cloned());
        }
        list
    }

    /// The host's terminal: where it's showing, else in the focused pane. It exists at once and shows
    /// the connecting; the service asks here for anything it needs along the way.
    pub(super) fn show_host_terminal(&mut self, host_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<u64> {
        let h = self.host_by_id(host_id)?.clone();
        let id = Self::terminal_id(&h);
        if let Some(o) = self.open.iter().find(|o| o.id == id) {
            let (key, exited) = (o.key, o.exited);
            if !exited {
                self.show(key, window, cx);
                return Some(key);
            }
            self.close_terminal_only(key, cx);
        }
        let workspace = hosts::write_workspace(&self.paths, &h).unwrap_or_else(|_| hosts::workspace_path(&self.paths, &h));
        match self.service.open_terminal(&h.id, self.palette.clone()) {
            Ok(term) => {
                let title = format!("{} · {}", h.name, t("Terminal"));
                Some(self.attach("ssh", id, workspace, Some(title), true, term, window, cx))
            }
            Err(e) => {
                self.toast(e, ToastKind::Error, cx);
                None
            }
        }
    }

    fn close_terminal_only(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(i) = self.open.iter().position(|o| o.key == key) {
            self.open.remove(i).view.read(cx).term.kill();
        }
        self.layout.forget(key);
    }

    /// Reconnects a host's terminal in its pane.
    pub(super) fn reconnect_host_terminal(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(host_id) = id.strip_prefix("ssh:") {
            let host_id = host_id.to_string();
            self.show_host_terminal(&host_id, window, cx);
        }
    }

    /// Opens a host's workspace: its terminal, and beside it an agent session (the given one, else its
    /// latest, else a new one), each in a pane of its own.
    pub(super) fn open_host(&mut self, host_id: &str, session: Option<Session>, agent: Option<String>, provider: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.host_by_id(host_id).cloned() else { return };
        let workspace = match hosts::write_workspace(&self.paths, &h) {
            Ok(w) => w,
            Err(e) => {
                self.toast(format!("{e}"), ToastKind::Error, cx);
                return;
            }
        };
        self.reload_hosts(cx);
        let Some(h) = self.host_by_id(host_id).cloned() else { return };
        let target = session.or_else(|| if agent.is_some() { None } else { self.host_sessions(&h).into_iter().next() });

        let term_id = Self::terminal_id(&h);
        let shown = self.open.iter().find(|o| o.id == term_id && !o.exited).map(|o| o.key).and_then(|k| self.layout.leaf_of(k));
        let term_leaf = match shown {
            Some(leaf) => leaf,
            None => {
                let Some(key) = self.show_host_terminal(&h.id, window, cx) else { return };
                let Some(leaf) = self.layout.leaf_of(key) else { return };
                leaf
            }
        };
        if let Some(s) = &target {
            let open = self.open.iter().find(|o| o.id == s.id && o.agent == s.agent).map(|o| o.key);
            if let Some(leaf) = open.and_then(|k| self.layout.leaf_of(k)) {
                let previous = self.layout.focused_session();
                self.layout.focus = leaf;
                self.focus_changed(previous, window, cx);
                return;
            }
        }

        // The agent: in another pane of this host if there is one, else a new pane to the right.
        let leaves = self.layout.leaves();
        let beside = leaves.iter().find(|(leaf, session)| {
            *leaf != term_leaf && session.and_then(|k| self.by_key(k)).is_some_and(|o| folder_text(&o.cwd) == folder_text(&workspace))
        });
        let empty = if leaves.len() > 1 { leaves.iter().find(|(leaf, session)| *leaf != term_leaf && session.is_none()) } else { None };
        let previous = self.layout.focused_session();
        match beside.or(empty) {
            Some((leaf, _)) => self.layout.focus = *leaf,
            None => {
                self.layout.focus = term_leaf;
                self.layout.split(Dir::Row);
            }
        }
        self.focus_changed(previous, window, cx);

        let before: HashSet<u64> = self.open.iter().map(|o| o.key).collect();
        match target {
            Some(s) => self.open_session(&s, &workspace, window, cx),
            None => {
                let agent = agent.unwrap_or_else(|| self.default_agent());
                match provider {
                    Some(_) => self.start_new_with(&agent, workspace.clone(), provider, None, window, cx),
                    None => self.new_session(&agent, workspace.clone(), window, cx),
                }
            }
        }
        let started: Vec<u64> = self.open.iter().map(|o| o.key).filter(|k| !before.contains(k)).collect();
        for key in started {
            self.trust_workspace(key, cx);
        }
    }

    /// The agents ask whether to trust a new folder (Claude's prompt starts on "No, exit"). A host's
    /// workspace is Crew's own folder holding only CLAUDE.md / AGENTS.md, so Crew answers yes there,
    /// and nowhere else.
    fn trust_workspace(&self, key: u64, cx: &mut Context<Self>) {
        let Some(o) = self.by_key(key) else { return };
        if !self.is_host_workspace(&o.cwd) {
            return;
        }
        let term = o.view.read(cx).term.clone();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            while started.elapsed() < TRUST_TIMEOUT {
                cx.background_executor().timer(Duration::from_millis(400)).await;
                let open = this.update(cx, |app, _| app.by_key(key).is_some_and(|o| !o.exited)).unwrap_or(false);
                if !open {
                    return;
                }
                let Some(selected) = trust_screen_selection(&term.screen_text()) else { continue };
                if says_yes(&selected) {
                    term.write(b"\r");
                    return;
                }
                term.write(b"\x1b[B");
            }
        })
        .detach();
    }

    /// A host's agent sessions: Claude, Codex, or a custom provider, which gets the host's
    /// instructions from the workspace like the others.
    fn host_agent_entries(&self, id: &str, name: Option<&str>) -> Vec<MenuEntry> {
        let label = |who: &str| match (name, zh()) {
            (Some(n), true) => format!("在 {n} 上新建 {who} 会话"),
            (Some(n), false) => format!("New {who} session on {n}"),
            (None, true) => format!("新建 {who} 会话"),
            (None, false) => format!("New {who} session"),
        };
        let mut items: Vec<MenuEntry> = [("claude", "Claude"), ("codex", "Codex")]
            .into_iter()
            .map(|(agent, who)| {
                let id = id.to_string();
                entry(label(who), move |app, window, cx| app.open_host(&id, None, Some(agent.to_string()), None, window, cx))
            })
            .collect();
        for (i, p) in self.providers.iter().enumerate() {
            let (id, agent, provider) = (id.to_string(), p.cli(), p.id.clone());
            let row = entry(label(&p.name), move |app, window, cx| app.open_host(&id, None, Some(agent.to_string()), Some(provider.clone()), window, cx));
            items.push(if i == 0 { row.separated() } else { row });
        }
        items
    }

    /// What a session runs on and where: "Deepseek · web-1 · root@203.0.113.10", or its folder.
    pub(super) fn session_place(&self, o: &OpenSession) -> String {
        let who = o.provider.as_ref().and_then(|id| self.providers.iter().find(|p| p.id == *id)).map_or(
            match o.agent.as_str() {
                "claude" => "Claude",
                "codex" => "Codex",
                _ => "SSH",
            },
            |p| p.name.as_str(),
        );
        let place = match o.id.strip_prefix("ssh:").and_then(|id| self.host_by_id(id)) {
            Some(h) => h.address(),
            None => self.host_of_folder(&o.cwd).map_or_else(|| o.cwd.to_string_lossy().to_string(), |h| format!("{} · {}", h.name, h.address())),
        };
        format!("{who} · {place}")
    }

    /// A session row's icon: its provider's, else Claude's or Codex's.
    pub(super) fn session_badge(&self, provider: Option<&str>, agent: &str, size: f32) -> AnyElement {
        match provider.and_then(|id| self.providers.iter().find(|p| p.id == id)) {
            Some(p) => self.provider_icon(p),
            None => self.agent_badge(agent, size),
        }
    }

    pub(super) fn open_session_icon(&self, o: &OpenSession) -> AnyElement {
        if o.agent == "ssh" {
            return div().flex_none().text_color(self.colors.running).child(Icon::new(IconName::SquareTerminal).size(px(14.))).into_any_element();
        }
        self.session_badge(o.provider.as_deref(), &o.agent, 13.)
    }

    /// The bar on a session showing in a pane that isn't the one in front.
    pub(super) fn shown_mark(&self) -> AnyElement {
        div().absolute().left_0().top(px(4.)).bottom(px(4.)).w(px(2.)).rounded(px(1.)).bg(self.colors.muted).into_any_element()
    }

    /// Whether a pane shows this host's terminal or an agent in its workspace.
    fn host_shown(&self, h: &Host) -> bool {
        let term_id = Self::terminal_id(h);
        if !self.open.iter().any(|o| o.id == term_id) {
            return false;
        }
        self.layout.leaves().iter().filter_map(|(_, session)| session.and_then(|k| self.by_key(k))).any(|o| o.id == term_id || self.host_of_folder(&o.cwd).is_some_and(|x| x.id == h.id))
    }

    fn host_menu(&mut self, id: &str, at: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(h) = self.host_by_id(id).cloned() else { return };
        let mut items = vec![{
            let id = id.to_string();
            entry(t("Open terminal and agent"), move |app, window, cx| app.open_host(&id, None, None, None, window, cx))
        }];
        items.extend(self.host_agent_entries(id, None));
        let host = h.clone();
        items.push(entry(t("Edit"), move |app, window, cx| app.edit_host(Some(host.clone()), window, cx)).separated());
        let test_id = id.to_string();
        items.push(entry(t("Test connection"), move |app, _, cx| app.test_host(&test_id, cx)));
        if self.service.is_connected(id) {
            let drop_id = id.to_string();
            items.push(entry(t("Disconnect"), move |app, _, cx| {
                app.service.disconnect(&drop_id);
                cx.notify();
            }));
        }
        let name = h.name.clone();
        items.push(entry(t("Copy name"), move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(name.clone()))));
        let host = h.clone();
        items.push(entry(t("Delete"), move |app, _, cx| app.ask_delete_host(&host, cx)).danger());
        self.open_menu(at, items, cx);
        self.set_menu_owner(format!("host:{}", id));
    }

    fn ask_delete_host(&mut self, h: &Host, cx: &mut Context<Self>) {
        let id = h.id.clone();
        let title = if zh() { format!("删除 {}？", h.name) } else { format!("Delete {}?", h.name) };
        self.confirm(
            title,
            t("Its saved password and settings are removed from Crew. The server itself isn't touched."),
            t("Delete"),
            move |app, _, cx| {
                app.service.forget_host(&id);
                if let Err(e) = hosts::delete(&app.paths, &id) {
                    app.toast(format!("{e}"), ToastKind::Error, cx);
                }
                app.reload_hosts(cx);
            },
            cx,
        );
    }

    fn test_host(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(h) = self.host_by_id(id) else { return };
        let name = h.name.clone();
        self.toast(if zh() { format!("正在连接 {name}…") } else { format!("Connecting to {name}…") }, ToastKind::Plain, cx);
        let (service, id) = (self.service.clone(), id.to_string());
        let test = cx.background_spawn(async move { service.test_host(&id) });
        cx.spawn(async move |this, cx| {
            let result = test.await;
            let _ = this.update(cx, |app, cx| match result {
                Ok(m) => app.toast(if zh() { format!("{name}：已连接 · {m}") } else { format!("{name}: connected · {m}") }, ToastKind::Ok, cx),
                Err(m) => app.toast(format!("{name}: {m}"), ToastKind::Error, cx),
            });
        })
        .detach();
    }

    fn import_hosts(&mut self, cx: &mut Context<Self>) {
        let (text, kind): (String, ToastKind) = match hosts::import_ssh_config(&self.paths) {
            None => (t("No ~/.ssh/config file was found.").into(), ToastKind::Plain),
            Some(0) => (t("Every host in ~/.ssh/config is already here.").into(), ToastKind::Plain),
            Some(n) if zh() => (format!("已从 ~/.ssh/config 导入 {n} 台主机。"), ToastKind::Ok),
            Some(n) => (format!("Imported {n} host{} from ~/.ssh/config.", if n == 1 { "" } else { "s" }), ToastKind::Ok),
        };
        self.toast(text, kind, cx);
        self.reload_hosts(cx);
    }

    fn install_skill(&mut self, cx: &mut Context<Self>) {
        let done = self.service.install_skill();
        if done.is_empty() {
            self.toast(t("Couldn't write the skill files."), ToastKind::Error, cx);
        } else {
            self.toast(t("Installed the crew-ssh skill. New sessions pick it up."), ToastKind::Ok, cx);
        }
    }

    fn explain_hosts(&mut self, cx: &mut Context<Self>) {
        let paragraphs = [
            "Sessions started in Crew have a crew-ssh command. Ask the agent things like \"check the disk on web-1\" or \"restart nginx on the staging servers\" and it runs crew-ssh with the host's name.",
            "While a host's terminal is open, the agent shares it with you: it reads what's on screen and types into it (commands, answers to prompts, keys like Ctrl+C), and you see everything it types. Passwords and codes asked in the terminal are yours to type.",
            "Crew signs in with the host's saved key, ssh-agent or password. The agent never sees credentials.",
            "Commands that only read (df, ps, systemctl status, journalctl…) run at once. Anything that could change something waits for you to approve it here. Change this per host.",
            "The first connection to a host shows its key fingerprint to trust. If the key later changes, Crew blocks the connection.",
            "Use \"Teach Claude Code and Codex to use crew-ssh\" once so the agents know the command without being told.",
        ];
        self.info(t("How agents use these hosts"), paragraphs.iter().map(|p| SharedString::from(t(p))).collect(), cx);
    }

    // ---------- the editor ----------

    pub(super) fn edit_host(&mut self, host: Option<Host>, window: &mut Window, cx: &mut Context<Self>) {
        let is_new = host.is_none();
        let h = host.unwrap_or_else(|| Host { port: Some(22), auth: "auto".into(), approval: "changes".into(), ..Host::default() });
        let plain = |value: String, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(t(placeholder)).default_value(value))
        };
        let secret = |saved: bool, empty: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            let hint = if saved { t("Saved (type to replace)") } else { t(empty) };
            cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(hint))
        };
        let name = plain(h.name.clone(), "e.g. web-1", window, cx);
        let address = plain(h.host.clone(), "example.com or 203.0.113.10", window, cx);
        let port = plain(h.port().to_string(), "22", window, cx);
        let user = plain(h.user.clone(), "e.g. root or ubuntu", window, cx);
        let key_path = plain(h.key_path.clone(), "Default: ~/.ssh/id_ed25519, id_ecdsa or id_rsa", window, cx);
        let group = plain(h.group.clone(), "Optional, e.g. production", window, cx);
        let passphrase = secret(h.has_passphrase(), "Only if the key has one", window, cx);
        let password = secret(h.has_password(), "Leave empty to be asked when needed", window, cx);
        let notes_text = h.notes.clone();
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 6)
                .placeholder(t("Instructions for agents on this host, e.g. \"Production: never restart the database. The app lives in /srv/app and runs under pm2.\""))
                .default_value(notes_text)
        });
        let focus = if is_new { address.clone() } else { name.clone() };
        focus.update(cx, |s, cx| s.focus(window, cx));
        self.host_editor = Some(HostEditor {
            id: (!is_new).then(|| h.id.clone()),
            title: match (is_new, zh()) {
                (true, _) => t("Add SSH host").to_string(),
                (false, true) => format!("编辑 {}", h.name),
                (false, false) => format!("Edit {}", h.name),
            },
            name,
            address,
            port,
            user,
            key_path,
            passphrase,
            password,
            group,
            notes,
            auth: if h.auth.is_empty() { "auto".into() } else { h.auth.clone() },
            approval: if h.approval.is_empty() { "changes".into() } else { h.approval.clone() },
            has_password: h.has_password(),
            has_passphrase: h.has_passphrase(),
            clear_password: false,
            clear_passphrase: false,
            can_seal: secrets::can_seal(),
            status: None,
            activity: if is_new { Vec::new() } else { self.service.activity(&h.id) },
        });
        cx.notify();
    }

    /// Saves what's in the editor. The next use signs in with the new settings.
    fn save_host_editor(&mut self, cx: &mut Context<Self>) -> Option<Host> {
        let e = self.host_editor.as_ref()?;
        let value = |s: &Entity<InputState>| s.read(cx).value().to_string();
        let host = value(&e.address);
        if host.trim().is_empty() {
            self.toast(t("Enter the host's address."), ToastKind::Error, cx);
            return None;
        }
        let input = HostInput {
            id: e.id.clone(),
            name: value(&e.name),
            host,
            port: value(&e.port),
            user: value(&e.user),
            auth: e.auth.clone(),
            key_path: value(&e.key_path),
            group: value(&e.group),
            notes: e.notes.read(cx).value().to_string(),
            approval: e.approval.clone(),
            password: if e.can_seal { value(&e.password) } else { String::new() },
            passphrase: if e.can_seal { value(&e.passphrase) } else { String::new() },
            clear_password: e.clear_password,
            clear_passphrase: e.clear_passphrase,
        };
        match hosts::save(&self.paths, &input) {
            Ok(saved) => {
                self.service.disconnect(&saved.id);
                if let Some(e) = self.host_editor.as_mut() {
                    e.id = Some(saved.id.clone());
                }
                self.reload_hosts(cx);
                Some(saved)
            }
            Err(err) => {
                self.toast(format!("{err}"), ToastKind::Error, cx);
                None
            }
        }
    }

    fn test_from_editor(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = self.save_host_editor(cx) else { return };
        let connecting = if zh() { format!("正在连接 {}…", saved.name) } else { format!("Connecting to {}…", saved.name) };
        if let Some(e) = self.host_editor.as_mut() {
            e.status = Some((connecting, None));
        }
        let service = self.service.clone();
        let test = cx.background_spawn(async move { service.test_host(&saved.id) });
        cx.spawn(async move |this, cx| {
            let result = test.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(e) = app.host_editor.as_mut() {
                    e.status = Some(match result {
                        Ok(m) => (if zh() { format!("已连接：{m}") } else { format!("Connected: {m}") }, Some(true)),
                        Err(m) => (m, Some(false)),
                    });
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn browse_key(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(file) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |app, cx| {
                app.pending_key_file = Some(file.to_string_lossy().to_string());
                cx.notify();
            });
        })
        .detach();
    }

    /// Text inputs need the window to change, which a file picker's answer doesn't have.
    pub(super) fn apply_pending_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.pending_key_file.take() else { return };
        if let Some(e) = &self.host_editor {
            e.key_path.update(cx, |s, cx| s.set_value(file, window, cx));
        }
    }

    pub(super) fn render_host_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let e = self.host_editor.as_ref()?;
        let input = |s: &Entity<InputState>| Input::new(s).small();
        let row = || div().flex().gap(px(10.));
        let auth_entries: Vec<MenuEntry> = ["auto", "key", "agent", "password"]
            .into_iter()
            .map(|a| {
                entry(auth_label(a), move |app, _, _| {
                    if let Some(e) = app.host_editor.as_mut() {
                        e.auth = a.to_string();
                    }
                })
            })
            .collect();
        let approval_entries: Vec<MenuEntry> = ["changes", "all", "auto"]
            .into_iter()
            .map(|a| {
                entry(approval_label(a), move |app, _, _| {
                    if let Some(e) = app.host_editor.as_mut() {
                        e.approval = a.to_string();
                    }
                })
            })
            .collect();
        let shows_key = e.auth == "auto" || e.auth == "key";
        let shows_password = e.auth == "auto" || e.auth == "password";
        let link = |id: &'static str, label: &'static str, on: fn(&mut HostEditor)| {
            div()
                .id(id)
                .text_size(px(12.))
                .text_color(c.accent)
                .cursor_pointer()
                .on_click(cx.listener(move |app, _, _, cx| {
                    if let Some(e) = app.host_editor.as_mut() {
                        on(e);
                    }
                    cx.notify();
                }))
                .child(t(label))
        };
        let forget_passphrase = link("forget-passphrase", "Forget the saved passphrase", |e| e.clear_passphrase = true);
        let forget_password = link("forget-password", "Forget the saved password", |e| e.clear_password = true);
        let forgotten = || div().text_size(px(12.)).text_color(c.muted).child(t("Will be forgotten when you save."));
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(self.heading(e.title.clone()))
            .child(
                row()
                    .child(self.field(t("Name"), input(&e.name), Some(t("What you and the agents call it").into())))
                    .child(self.field(t("Group"), input(&e.group), None)),
            )
            .child(
                row()
                    .child(self.field(t("Address"), input(&e.address), None))
                    .child(div().w(px(80.)).flex_none().child(self.field(t("Port"), input(&e.port), None)))
                    .child(self.field(t("User"), input(&e.user), None)),
            )
            .child(self.field(t("Sign in with"), self.select("host-auth", auth_label(&e.auth), auth_entries, cx), None))
            .when(shows_key, |d| {
                d.child(
                    row()
                        .child(self.field(
                            t("Key file"),
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(div().flex_1().min_w(px(0.)).child(input(&e.key_path)))
                                .child(self.button("host-browse", t("Browse…"), false).on_click(cx.listener(|app, _, _, cx| app.browse_key(cx)))),
                            None,
                        ))
                        .child(
                            self.field(t("Key passphrase"), input(&e.passphrase), None).map(|d| match (e.has_passphrase, e.clear_passphrase) {
                                (_, true) => d.child(forgotten()),
                                (true, false) => d.child(forget_passphrase),
                                _ => d,
                            }),
                        ),
                )
            })
            .when(shows_password, |d| {
                d.child(self.field(t("Password"), input(&e.password), None).map(|d| match (e.has_password, e.clear_password) {
                    (_, true) => d.child(forgotten()),
                    (true, false) => d.child(forget_password),
                    _ => d.child(div().text_size(px(12.)).text_color(c.muted).child(if e.can_seal {
                        t("Saved encrypted with Windows data protection.")
                    } else {
                        t("This system can't encrypt it, so it isn't saved.")
                    })),
                }))
            })
            .child(self.field(t("Agents"), self.select("host-approval", approval_label(&e.approval), approval_entries, cx), None))
            .child(self.field(t("Notes for agents"), Textarea::new(&e.notes).small(), None))
            .when(!e.activity.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(div().text_size(px(12.5)).text_color(c.muted).child(t("Recent agent commands")))
                        .children(e.activity.iter().map(|a| {
                            let result = if a.decision == "denied" {
                                t("denied").to_string()
                            } else if a.error.is_some() {
                                t("failed").to_string()
                            } else {
                                let code = a.code.map_or(String::new(), |c| c.to_string());
                                if a.decision == "approved" { format!("exit {code} · {}", t("approved")) } else { format!("exit {code}") }
                            };
                            div()
                                .flex()
                                .gap(px(8.))
                                .text_size(px(12.))
                                .child(div().flex_none().text_color(c.muted).child(rel_time(a.at as f64)))
                                .child(div().flex_1().min_w(px(0.)).truncate().font_family("Cascadia Mono").child(a.command.clone()))
                                .child(div().flex_none().text_color(if a.decision == "denied" { gpui::rgb(0xc4462d).into() } else { c.muted }).child(result))
                        })),
                )
            })
            .when_some(e.status.clone(), |d, (text, ok)| {
                d.child(
                    div()
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(c.hover)
                        .text_size(px(12.5))
                        .text_color(match ok {
                            Some(true) => c.running,
                            Some(false) => gpui::rgb(0xd93d42).into(),
                            None => c.text,
                        })
                        .child(text),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(4.))
                    .when_some(e.id.clone(), |d, id| {
                        d.child(
                            div()
                                .id("host-delete")
                                .text_size(px(13.))
                                .text_color(gpui::rgb(0xd93d42))
                                .cursor_pointer()
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    let Some(h) = app.host_by_id(&id).cloned() else { return };
                                    app.host_editor = None;
                                    app.ask_delete_host(&h, cx);
                                }))
                                .child(t("Delete")),
                        )
                    })
                    .child(div().flex_1())
                    .child(self.button("host-test", t("Test connection"), false).on_click(cx.listener(|app, _, _, cx| app.test_from_editor(cx))))
                    .child(self.button("host-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                        app.host_editor = None;
                        cx.notify();
                    })))
                    .child(self.button("host-save", t("Save"), true).on_click(cx.listener(|app, _, _, cx| {
                        if app.save_host_editor(cx).is_some() {
                            app.host_editor = None;
                        }
                        cx.notify();
                    }))),
            );
        Some(self.modal("host-editor", 620., body))
    }

    // ---------- the list ----------

    fn toggle_host_fold(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut folded = self.config.list("hostFolded");
        match folded.iter().position(|f| f == id) {
            Some(i) => {
                folded.remove(i);
            }
            None => folded.push(id.to_string()),
        }
        self.save_setting("hostFolded", json!(folded));
        cx.notify();
    }

    fn render_host_rows(&self, h: &Host, folded: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = self.colors;
        let connected = self.service.is_connected(&h.id);
        let shown = self.host_shown(h);
        let host_tip = {
            let notes = if h.notes.is_empty() { String::new() } else { format!("\n\n{}: {}", t("Notes for agents"), h.notes) };
            SharedString::from(format!("{}\n{}\n{}{notes}", h.name, h.address(), t("Click to open its terminal and agent")))
        };
        let group = SharedString::from(format!("host-row-{}", h.id));
        let menu_open = self.menu_owner() == Some(format!("host:{}", h.id).as_str());
        let (open_id, menu_id, more_id, plus_id, fold_id) = (h.id.clone(), h.id.clone(), h.id.clone(), h.id.clone(), h.id.clone());
        let mut rows = vec![
            div()
                .id(SharedString::from(format!("host-{}", h.id)))
                .group(group.clone())
                .relative()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(34.))
                .py(px(4.))
                .pl(px(12.))
                .pr(px(4.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(14.))
                .text_color(c.text)
                .hover(move |s| s.bg(c.hover))
                .when(menu_open, |d| d.bg(c.hover))
                .on_click(cx.listener(move |app, _, window, cx| app.open_host(&open_id, None, None, None, window, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &MouseDownEvent, _, cx| app.host_menu(&menu_id, e.position, cx)))
                .child(div().flex_none().text_color(if shown { c.accent } else if connected { c.running } else { c.muted }).child(Icon::empty().path(assets::SERVER).size(px(16.))))
                .child(div().id(SharedString::from(format!("host-name-{}", h.id))).flex_1().min_w(px(0.)).truncate().when(self.menu.is_none(), |d| d.tooltip(move |window, cx| Tooltip::new(host_tip.clone()).build(window, cx))).child(h.name.clone()))
                .child(div().flex_none().max_w(relative(0.45)).truncate().text_size(px(12.)).text_color(c.muted).when(menu_open, |d| d.invisible()).group_hover(group.clone(), |s| s.invisible()).child(h.address()))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(26.))
                        .flex()
                        .items_center()
                        .opacity(if menu_open { 1. } else { 0. })
                        .group_hover(group, |s| s.opacity(1.))
                        .child(self.row_button(SharedString::from(format!("host-new-{}", h.id)), IconName::Plus).p(px(3.)).on_click(cx.listener(move |app, e: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            let name = app.host_by_id(&plus_id).map(|h| h.name.clone());
                            let items = app.host_agent_entries(&plus_id, name.as_deref());
                            app.open_menu(e.position(), items, cx);
                            app.set_menu_owner(format!("host:{}", plus_id));
                        })))
                        .child(self.row_button(SharedString::from(format!("host-more-{}", h.id)), Icon::empty().path(assets::DOTS)).p(px(3.)).on_click(cx.listener(move |app, e: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            app.host_menu(&more_id, e.position(), cx);
                        }))),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("host-fold-{}", h.id)))
                        .flex_none()
                        .p(px(3.))
                        .text_color(c.muted)
                        .occlude()
                        .on_click(cx.listener(move |app, _, _, cx| {
                            cx.stop_propagation();
                            app.toggle_host_fold(&fold_id, cx);
                        }))
                        .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(14.))),
                )
                .into_any_element(),
        ];
        if folded {
            return rows;
        }

        let term_id = Self::terminal_id(h);
        let term = self.open.iter().find(|o| o.id == term_id);
        let term_active = term.is_some_and(|o| Some(o.key) == self.layout.focused_session());
        let term_alive = term.is_some_and(|o| !o.exited);
        let term_shown = term.and_then(|o| self.layout.leaf_of(o.key)).is_some();
        let term_tip = SharedString::from(if zh() { format!("你在 {} 上的终端", h.name) } else { format!("Your terminal on {}", h.name) });
        let show_id = h.id.clone();
        let mut children: Vec<AnyElement> = vec![
            div()
                .id(SharedString::from(format!("host-term-{}", h.id)))
                .relative()
                .when(term_shown && !term_active, |d| d.child(self.shown_mark()))
                .when(self.menu.is_none(), |d| d.tooltip(move |window, cx| Tooltip::new(term_tip.clone()).build(window, cx)))
                .flex()
                .items_center()
                .gap(px(8.))
                .min_h(px(30.))
                .pl(px(10.))
                .pr(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(13.5))
                .text_color(c.text)
                .when(term_active, |d| d.bg(c.card))
                .when(!term_active, |d| d.hover(move |s| s.bg(c.hover)))
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.show_host_terminal(&show_id, window, cx);
                }))
                .when(term_alive, |d| d.child(div().flex_none().size(px(7.)).rounded_full().bg(c.running)))
                .child(div().flex_none().text_color(c.running).child(Icon::new(IconName::SquareTerminal).size(px(14.))))
                .child(div().flex_1().min_w(px(0.)).truncate().child(t("Terminal")))
                .into_any_element(),
        ];
        for s in self.host_sessions(h) {
            let open = self.open.iter().find(|o| o.id == s.id && o.agent == s.agent);
            let active = open.is_some_and(|o| Some(o.key) == self.layout.focused_session());
            let elsewhere = self.live_elsewhere.get(&s.id).copied();
            let alive = open.is_some_and(|o| !o.exited) || (open.is_none() && elsewhere.is_some());
            let busy = alive && open.map_or(elsewhere == Some(true), |o| o.busy);
            let (host_id, session) = (h.id.clone(), s.clone());
            let row_shown = open.and_then(|o| self.layout.leaf_of(o.key)).is_some();
            let row_tip = SharedString::from(s.title.clone().unwrap_or_default());
            let row_provider = s.provider.as_deref().or(open.and_then(|o| o.provider.as_deref()));
            let (menu_session, menu_project) = (s.clone(), hosts::workspace_path(&self.paths, h));
            children.push(
                div()
                    .id(SharedString::from(format!("host-session-{}-{}", h.id, s.id)))
                    .relative()
                    .when(row_shown && !active, |d| d.child(self.shown_mark()))
                    .when(self.menu.is_none(), |d| d.tooltip(move |window, cx| Tooltip::new(row_tip.clone()).build(window, cx)))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_h(px(30.))
                    .pl(px(10.))
                    .pr(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(13.5))
                    .text_color(c.text)
                    .when(active, |d| d.bg(c.card))
                    .when(!active, |d| d.hover(move |s| s.bg(c.hover)))
                    .on_click(cx.listener(move |app, _, window, cx| app.open_host(&host_id, Some(session.clone()), None, None, window, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &MouseDownEvent, _, cx| app.session_menu(menu_session.clone(), menu_project.clone(), e.position, cx)))
                    .when(busy, |d| d.child(Spinner::new().color(c.accent).xsmall()))
                    .when(alive && !busy, |d| d.child(div().flex_none().size(px(7.)).rounded_full().bg(c.running)))
                    .child(self.session_badge(row_provider, &s.agent, 14.))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(s.title.clone().unwrap_or_else(|| t("Untitled").to_string())))
                    .when(s.mtime > 0., |d| d.child(div().flex_none().text_size(px(12.)).text_color(c.muted).child(rel_time(s.mtime))))
                    .into_any_element(),
            );
        }
        rows.push(div().flex().flex_col().ml(px(18.)).pl(px(6.)).mb(px(4.)).border_l_1().border_color(c.border).children(children).into_any_element());
        rows
    }

    pub(super) fn render_hosts(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let actions = div()
            .flex()
            .gap(px(2.))
            .child(self.icon_button("hosts-more", Icon::empty().path(assets::DOTS)).p(px(4.)).on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let items = vec![
                    entry(t("Add host…"), |app, window, cx| app.edit_host(None, window, cx)),
                    entry(t("Import from ~/.ssh/config"), |app, _, cx| app.import_hosts(cx)),
                    entry(t("Teach Claude Code and Codex to use crew-ssh"), |app, _, cx| app.install_skill(cx)).separated(),
                    entry(t("How agents use these hosts"), |app, _, cx| app.explain_hosts(cx)),
                ];
                app.open_menu(e.position, items, cx);
            })))
            .child(self.icon_button("hosts-add", IconName::Plus).p(px(4.)).on_click(cx.listener(|app, _, window, cx| app.edit_host(None, window, cx))))
            .into_any_element();
        let head = self.section_head("hosts-toggle", "Hosts", "hostsCollapsed", actions, cx);
        let folded = self.config.list("hostFolded");
        let mut sorted: Vec<&Host> = self.hosts.iter().collect();
        sorted.sort_by(|a, b| (!a.group.is_empty()).cmp(&!b.group.is_empty()).then_with(|| a.group.cmp(&b.group)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last_group = "";
        for h in sorted {
            if h.group != last_group && !h.group.is_empty() {
                rows.push(div().pt(px(8.)).pb(px(2.)).px(px(12.)).text_size(px(11.)).text_color(c.muted).child(h.group.to_uppercase()).into_any_element());
            }
            last_group = &h.group;
            rows.extend(self.render_host_rows(h, folded.contains(&h.id), cx));
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .px(px(12.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .text_size(px(12.5))
                    .text_color(c.muted)
                    .child(t("Add a server here and the agents can manage it with crew-ssh."))
                    .child(div().id("hosts-empty-add").text_color(c.accent).cursor_pointer().on_click(cx.listener(|app, _, window, cx| app.edit_host(None, window, cx))).child(t("Add a host")))
                    .child(div().id("hosts-empty-import").text_color(c.accent).cursor_pointer().on_click(cx.listener(|app, _, _, cx| app.import_hosts(cx))).child(t("Import from ~/.ssh/config")))
                    .into_any_element(),
            );
        }
        let collapsed = self.config.bool("hostsCollapsed");
        div()
            .flex()
            .flex_col()
            .flex_none()
            .max_h(relative(0.36))
            .child(head)
            .when(!collapsed, |d| d.child(div().id("host-list").flex_1().min_h(px(0.)).overflow_y_scroll().pb(px(6.)).children(rows)))
            .into_any_element()
    }

    // ---------- what Crew asks ----------

    /// Host-key, password and 2FA questions come one at a time in a dialog; approvals stack as cards.
    pub(super) fn on_prompt(&mut self, prompt: Prompt, window: &mut Window, cx: &mut Context<Self>) {
        flash_if_behind(window);
        if matches!(prompt.kind, PromptKind::Approve { .. }) {
            self.approvals.push(prompt);
        } else {
            self.prompt_queue.push_back(prompt);
            if self.prompt_shown.is_none() {
                self.next_prompt(window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn on_prompt_cancelled(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.approvals.retain(|p| p.id != id);
        self.prompt_queue.retain(|p| p.id != id);
        if self.prompt_shown.as_ref().is_some_and(|p| p.prompt.id == id) {
            self.next_prompt(window, cx);
        }
        cx.notify();
    }

    fn next_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt_queue.pop_front() else {
            self.prompt_shown = None;
            return;
        };
        let inputs: Vec<Entity<InputState>> = match &prompt.kind {
            PromptKind::Password { .. } | PromptKind::Passphrase { .. } => vec![cx.new(|cx| InputState::new(window, cx).masked(true))],
            PromptKind::Keyboard { prompts, .. } => prompts.iter().map(|(_, echo)| {
                let masked = !*echo;
                cx.new(|cx| InputState::new(window, cx).masked(masked))
            }).collect(),
            _ => Vec::new(),
        };
        if let Some(first) = inputs.first() {
            first.update(cx, |s, cx| s.focus(window, cx));
        }
        if let Some(last) = inputs.last() {
            cx.subscribe_in(last, window, |app, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    app.secret_answer(window, cx);
                }
            })
            .detach();
        }
        self.prompt_shown = Some(PromptView { prompt, inputs, remember: false });
    }

    fn answer_prompt(&mut self, answer: Option<Answer>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.prompt_shown.take() {
            self.service.answer(p.prompt.id, answer);
        }
        self.next_prompt(window, cx);
        cx.notify();
    }

    fn secret_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.prompt_shown else { return };
        let answer = match &p.prompt.kind {
            PromptKind::Keyboard { .. } => Answer::Keyboard(p.inputs.iter().map(|i| i.read(cx).value().to_string()).collect()),
            _ => Answer::Secret { value: p.inputs.first().map(|i| i.read(cx).value().to_string()).unwrap_or_default(), remember: p.remember },
        };
        self.answer_prompt(Some(answer), window, cx);
    }

    pub(super) fn render_prompt(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let p = self.prompt_shown.as_ref()?;
        let danger = gpui::rgb(0xd93d42);
        let text = |s: String| div().text_color(c.text).child(s);
        let fingerprint = |s: &str| div().px(px(10.)).py(px(6.)).rounded(px(8.)).bg(c.hover).font_family("Cascadia Mono").text_size(px(12.5)).child(s.to_string());
        let label = |s: &'static str| div().text_size(px(12.5)).text_color(c.muted).child(t(s));
        let actions = || div().flex().justify_end().gap(px(8.)).pt(px(4.));
        let cancel = |id: &'static str, name: &'static str, primary: bool| {
            self.button(id, t(name), primary).on_click(cx.listener(|app, _, window, cx| app.answer_prompt(None, window, cx)))
        };
        let submit = |name: &'static str| self.button("prompt-submit", t(name), true).on_click(cx.listener(|app, _, window, cx| app.secret_answer(window, cx)));
        let body = match &p.prompt.kind {
            PromptKind::HostKey { host, address, key_type, fingerprint: print } => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(self.heading(if zh() { format!("信任 {host}？") } else { format!("Trust {host}?") }))
                .child(text(if zh() {
                    format!("这是第一次连接 {address}。请确认它的 {key_type} 密钥指纹与服务器一致：")
                } else {
                    format!("This is the first connection to {address}. Check that its {key_type} key fingerprint matches the server's:")
                }))
                .child(fingerprint(print))
                .child(div().text_size(px(12.)).text_color(c.muted).child(format!("{} ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub", t("On the server:"))))
                .child(actions().child(cancel("prompt-cancel", "Cancel", false)).child(self.button("prompt-trust", t("Trust and connect"), true).on_click(cx.listener(|app, _, window, cx| app.answer_prompt(Some(Answer::Trust), window, cx))))),
            PromptKind::HostKeyChanged { host, address, key_type, fingerprint: print, previous } => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(self.heading(if zh() { format!("{host} 的密钥已改变") } else { format!("{host}'s key has changed") }).text_color(danger))
                .child(text(if zh() {
                    format!("{address} 的 {key_type} 密钥与之前信任的不同。服务器重装时会这样，但也可能是有人在拦截连接。Crew 已阻止这次连接。")
                } else {
                    format!("The {key_type} key of {address} isn't the one you trusted before. This happens when a server is reinstalled, but it can also mean someone is intercepting the connection. Crew blocked it.")
                }))
                .child(label("Trusted before"))
                .child(fingerprint(previous))
                .child(label("Now"))
                .child(fingerprint(print))
                .child(
                    actions()
                        .child(cancel("prompt-keep-blocking", "Keep blocking", true))
                        .child(self.button("prompt-replace", t("Replace the key and connect"), false).text_color(danger).on_click(cx.listener(|app, _, window, cx| app.answer_prompt(Some(Answer::Trust), window, cx)))),
                ),
            PromptKind::Password { .. } | PromptKind::Passphrase { .. } => {
                let (title, why) = match &p.prompt.kind {
                    PromptKind::Password { host } => (
                        if zh() { format!("{host} 的密码") } else { format!("Password for {host}") },
                        t("An agent is connecting to this host. The password stays in Crew.").to_string(),
                    ),
                    PromptKind::Passphrase { host, key_file } => (
                        t("Key passphrase").to_string(),
                        if zh() { format!("用于 {key_file}，连接 {host}。") } else { format!("For {key_file}, to connect to {host}.") },
                    ),
                    _ => unreachable!(),
                };
                let remember = p.remember;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(self.heading(title))
                    .child(text(why))
                    .children(p.inputs.iter().map(|i| Input::new(i).small()))
                    .child(
                        div()
                            .id("prompt-remember")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .cursor_pointer()
                            .text_size(px(13.))
                            .on_click(cx.listener(|app, _, _, cx| {
                                if let Some(p) = app.prompt_shown.as_mut() {
                                    p.remember = !p.remember;
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(15.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(if remember { c.accent } else { c.border })
                                    .when(remember, |d| d.bg(c.accent).text_color(gpui::white()).child(Icon::new(IconName::Check).size(px(11.)))),
                            )
                            .child(t("Remember it (saved encrypted)")),
                    )
                    .child(actions().child(cancel("prompt-cancel", "Cancel", false)).child(submit("Connect")))
            }
            PromptKind::Keyboard { host, instructions, prompts } => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(self.heading(if zh() { format!("登录 {host}") } else { format!("Sign in to {host}") }))
                .when(!instructions.is_empty(), |d| d.child(text(instructions.clone())))
                .children(prompts.iter().zip(&p.inputs).map(|((q, _), i)| self.field(q.clone(), Input::new(i).small(), None)))
                .child(actions().child(cancel("prompt-cancel", "Cancel", false)).child(submit("Continue"))),
            PromptKind::Approve { .. } => return None,
        };
        Some(self.modal("ssh-prompt", 480., body))
    }

    /// Approval cards, over the corner of the window. They wait: the agent's call waits with them.
    pub(super) fn render_approvals(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        if self.approvals.is_empty() {
            return None;
        }
        let cards = self.approvals.iter().filter_map(|p| {
            let PromptKind::Approve { host, action, command, cwd, session, agent, .. } = &p.kind else { return None };
            let who = match agent.as_str() {
                "claude" => "Claude",
                "codex" => "Codex",
                _ => t("An agent"),
            };
            let title = self.open.iter().find(|o| &o.id == session).and_then(|o| o.title.clone());
            let verb = match action.as_str() {
                "upload" => t("wants to upload a file to"),
                "download" => t("wants to download a file from"),
                "input" => t("wants to type into your terminal on"),
                _ => t("wants to run on"),
            };
            let id = p.id;
            let answer = move |app: &mut CrewApp, a: Option<Answer>, cx: &mut Context<CrewApp>| {
                app.service.answer(id, a);
                app.approvals.retain(|p| p.id != id);
                cx.notify();
            };
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(c.card)
                    .border_1()
                    .border_color(c.border)
                    .shadow_md()
                    .text_size(px(13.))
                    .relative()
                    .overflow_hidden()
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(c.accent))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(4.))
                            .child(div().flex_none().text_color(c.accent).child(Icon::empty().path(assets::SERVER).size(px(14.))))
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(who))
                            .when_some(title, |d, title| d.child(div().text_color(c.muted).child(format!("· {title}"))))
                            .child(verb)
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(host.clone())),
                    )
                    .child(div().id(("approval-command", id as usize)).max_h(px(160.)).overflow_y_scroll().px(px(10.)).py(px(6.)).rounded(px(8.)).bg(c.hover).font_family("Cascadia Mono").text_size(px(12.5)).child(command.clone()))
                    .when(!cwd.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(c.muted).child(format!("{} {cwd}", t("in")))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(self.button(("approval-deny", id as usize), t("Deny"), false).on_click(cx.listener(move |app, _, _, cx| answer(app, None, cx))))
                            .child(div().flex_1())
                            .child(self.button(("approval-host", id as usize), t("Allow 15 min"), false).tooltip(|window, cx| Tooltip::new(t("Run everything the agents send to this host for the next 15 minutes")).build(window, cx)).on_click(cx.listener(move |app, _, _, cx| answer(app, Some(Answer::AllowHost), cx))))
                            .child(self.button(("approval-once", id as usize), t("Allow"), true).on_click(cx.listener(move |app, _, _, cx| answer(app, Some(Answer::Once), cx)))),
                    ),
            )
        });
        Some(
            div()
                .absolute()
                .right(px(16.))
                .bottom(px(44.))
                .w(px(440.))
                .max_w(relative(0.9))
                .flex()
                .flex_col()
                .gap(px(8.))
                .occlude()
                .children(cards)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn reads_the_agents_trust_screens() {
        let claude = lines("Quick safety check: Is this a project you created?\n ❯ No, exit\n   Yes, I trust this folder");
        assert_eq!(trust_screen_selection(&claude).as_deref(), Some("No, exit"));
        assert!(!says_yes("No, exit"));
        assert!(says_yes("Yes, I trust this folder"));
        let codex = lines("Trust this folder? Codex can read files\n> 1. Trust and continue\n  2. Quit");
        assert!(says_yes(&trust_screen_selection(&codex).unwrap()));
        assert!(!says_yes("2. Quit"));
        assert_eq!(trust_screen_selection(&lines("> nothing to trust here\n")), None);
    }
}
