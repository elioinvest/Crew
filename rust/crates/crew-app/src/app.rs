//! The window: sidebar of projects and sessions, panes of terminals, the prompt box.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use crew_core::hosts::{self, Host};
use crew_core::providers::Provider;
use crew_core::roster::{self, RosterAgent};
use crew_core::usage::{self, PlanUsage, PlanWindow, Today, UsageScanner};
use crew_core::{Config, Paths, Project, Session, agents, env, providers, scan_projects, worktrees};
use crew_service::{Event, Prompt, Service};
use crew_term::{Command, Palette, Terminal};
use gpui::{
    AnyElement, Bounds, BoxShadow, Context, Entity, Image, ImageFormat, KeyDownEvent, MouseButton, MouseMoveEvent, ObjectFit,
    PathPromptOptions, Pixels, SharedString, Window, canvas, div, hsla, img, point, prelude::*, px, relative,
};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, IconName, Sizable, TitleBar};
use serde_json::{Value, json};

use crate::i18n::{show_more, t};
use crate::{assets, usage_probe};
use crate::panes::{Dir, Layout, Node};
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::theme::Colors;

mod agents_panel;
mod agent_setup;
mod attachments;
mod browser_panel;
mod composer_tools;
mod drag_drop;
mod files_panel;
mod forms;
mod git_panel;
mod git_pickers;
mod git_view;
mod hosts_panel;
mod overlays;
mod phone;
mod session_menus;
mod term_links;
mod settings_page;
mod usage_page;

/// The pane showing an opened file, among the panes' session keys.
const FILE_KEY: u64 = u64::MAX;
const UI_FONT: &str = "Segoe UI";
/// How far sessions sit in from their agent group.
const SESSION_INDENT: f32 = 16.;
const SIDEBAR_WIDTH: f32 = 300.;
const SIDEBAR_MIN: f32 = 200.;
const SIDEBAR_MAX: f32 = 560.;
const RESCAN: Duration = Duration::from_secs(3);
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Each agent's group shows this many before "Show more".
const SESSIONS_SHOWN: usize = 8;
const USAGE_TICK: Duration = Duration::from_secs(30);
const CODEX_CHECK_EVERY: Duration = Duration::from_secs(2 * 60);
const CLAUDE_CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
const TOKENS_EVERY: Duration = Duration::from_secs(5 * 60);

struct OpenSession {
    key: u64,
    id: String,
    agent: String,
    cwd: PathBuf,
    title: Option<String>,
    view: Entity<TerminalView>,
    started: SystemTime,
    /// Whether a transcript exists yet: a session that does is resumed rather than started again.
    on_disk: bool,
    exited: bool,
    busy: bool,
    /// The custom provider it runs on.
    provider: Option<String>,
    /// What was in the prompt box when another session was shown.
    draft: String,
    /// The worktree its folder is: it's listed under the repository's project.
    worktree: Option<worktrees::SessionWorktree>,
    chips: composer_tools::SessionChips,
    attachments: Vec<attachments::Attachment>,
    /// The images each message from this box carried, for "[Image #N]" before it's in the transcript.
    sent_images: Vec<Vec<PathBuf>>,
}

fn open_key_of(open: Option<&OpenSession>) -> Option<u64> {
    open.map(|o| o.key)
}

#[derive(Clone, Copy)]
enum Drag {
    Sidebar,
    Files,
    Git,
    Browser,
    Split { id: u64, dir: Dir },
}

pub struct CrewApp {
    paths: Paths,
    config: Config,
    projects: Vec<Project>,
    search: Entity<InputState>,
    composer: Entity<TextareaState>,
    open: Vec<OpenSession>,
    next_key: u64,
    layout: Layout,
    /// The project a new session starts in.
    current_project: Option<PathBuf>,
    sidebar_hidden: bool,
    sidebar_width: f32,
    drag: Option<Drag>,
    /// Where each split was last drawn, to turn a dragged divider's position into a ratio.
    split_bounds: Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>,
    /// The open session's branch: read from disk when something changes, never while drawing.
    branch: Option<String>,
    notice: Option<SharedString>,
    colors: Colors,
    palette: Palette,
    logo: Arc<Image>,
    hosts: Vec<Host>,
    roster: Vec<RosterAgent>,
    providers: Vec<Provider>,
    provider_images: HashMap<String, Arc<Image>>,
    /// Agent groups showing all their sessions.
    show_all: HashSet<String>,
    plan_claude: Option<PlanUsage>,
    plan_codex: Option<PlanUsage>,
    today: Option<Today>,
    scanner: Arc<Mutex<UsageScanner>>,
    claude_checked: Option<Instant>,
    codex_checked: Option<Instant>,
    tokens_checked: Option<Instant>,
    /// Checks the refresh button is waiting for.
    usage_forced: usize,
    service: Service,
    menu: Option<overlays::Menu>,
    toast: Option<overlays::Toast>,
    dialog: Option<overlays::Dialog>,
    host_editor: Option<hosts_panel::HostEditor>,
    pending_key_file: Option<String>,
    prompt_queue: VecDeque<Prompt>,
    prompt_shown: Option<hosts_panel::PromptView>,
    approvals: Vec<Prompt>,
    agent_editor: Option<agents_panel::AgentEditor>,
    run_cards: Vec<agents_panel::RunCard>,
    run_result: Option<agents_panel::RunResult>,
    run_clock: bool,
    settings: Option<settings_page::SettingsPage>,
    provider_editor: Option<settings_page::ProviderEditor>,
    usage_page: Option<usage_page::UsagePage>,
    price_editor: Option<usage_page::PriceEditor>,
    usage_data: Option<usage::UsageData>,
    /// Holds the focus when nothing else does: with none, keys don't reach the window's shortcuts.
    root_focus: gpui::FocusHandle,
    focus_return: Option<gpui::WeakFocusHandle>,
    files: Option<files_panel::FilesPanel>,
    git: Option<git_panel::GitPanel>,
    browser: Option<browser_panel::BrowserPanel>,
    see_through: bool,
    image_view: Option<PathBuf>,
    image_hover: term_links::ImageHover,
    /// Agents found on this computer, so each is only looked for once.
    agents_found: HashSet<String>,
    missing_agent: Option<agent_setup::MissingAgent>,
    /// The installer's pane, and the session waiting for it.
    installing: Option<(u64, agent_setup::PendingStart)>,
    /// The project a dragged one would land next to, and whether after it.
    project_drop: Option<(PathBuf, bool)>,
    saved_layout: Value,
    /// Sessions with a live agent anywhere on this machine, and whether each is working.
    live_elsewhere: HashMap<String, bool>,
    claude_catalog: Option<Arc<crew_core::composer::ClaudeCatalog>>,
    codex_list: Option<(Instant, Arc<Vec<crew_core::composer::CodexModel>>)>,
    codex_list_loading: bool,
    phone: phone::PhoneState,
    /// What a chip just couldn't do.
    hint: Option<String>,
    items_cache: HashMap<String, (Instant, Option<Rc<crew_core::composer::Items>>)>,
    suggest: Option<composer_tools::Suggest>,
    renaming: Option<session_menus::Renaming>,
    form: Option<forms::Form>,
    picker: Option<forms::Picker>,
    file_view: Option<files_panel::FileView>,
    files_width: Option<f32>,
    /// What to do once the user has answered about the file's unsaved changes.
    pending_after_save: Option<overlays::Action>,
    /// What's waiting for the next draw, which has a window to act on.
    deferred_action: Option<overlays::Action>,
    _subscriptions: Vec<gpui::Subscription>,
}

/// One group of a project's sessions: an agent, or a custom provider.
struct SessionGroup<'a> {
    key: String,
    label: String,
    icon: AnyElement,
    /// What its + starts.
    agent: &'static str,
    provider: Option<String>,
    sessions: Vec<&'a Session>,
}

fn data_url_image(url: &str) -> Option<Arc<Image>> {
    let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
    let format = match head.split(';').next()? {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::Webp,
        "image/gif" => ImageFormat::Gif,
        "image/svg+xml" => ImageFormat::Svg,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

fn due(checked: Option<Instant>, every: Duration) -> bool {
    checked.is_none_or(|t| t.elapsed() >= every)
}

fn window_name(minutes: i64) -> String {
    match minutes {
        300 => "5-hour".into(),
        10080 => t("week").into(),
        m if m % 1440 == 0 => format!("{}-day", m / 1440),
        m => format!("{}-hour", (m as f64 / 60.).round() as i64),
    }
}

/// A plan's limits in full, for hovering its item in the status bar.
fn usage_details(agent: &str, plan: &PlanUsage, windows: &[PlanWindow], now: i64) -> String {
    let name = if agent == "codex" { "Codex" } else { "Claude" };
    let mut lines = vec![match &plan.plan {
        Some(p) => format!("{name} ({p} plan)"),
        None => name.to_string(),
    }];
    for w in windows {
        let limit = if w.minutes == 10080 { "Weekly".to_string() } else { window_name(w.minutes) };
        let reset = w.resets_at.map_or(String::new(), |at| {
            let when = chrono::TimeZone::timestamp_millis_opt(&chrono::Local, at).single().map_or(String::new(), |d| d.format("%a %H:%M").to_string());
            format!(", resets in {} ({when})", usage::until_text(at - now))
        });
        lines.push(format!("{limit} limit: {}% used{reset}", w.used.round() as i64));
    }
    let ago = rel_time(plan.at as f64);
    lines.push(if ago == t("now") { t("Updated just now").to_string() } else { format!("Updated {ago} ago") });
    lines.join("\n")
}

/// How long ago, as the sidebar shows it.
fn rel_time(ms: f64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.);
    let s = now - ms / 1000.;
    match s {
        s if s < 60. => t("now").into(),
        s if s < 3600. => format!("{}m", (s / 60.) as u64),
        s if s < 86400. => format!("{}h", (s / 3600.) as u64),
        s if s < 86400. * 30. => format!("{}d", (s / 86400.) as u64),
        s => format!("{}mo", (s / (86400. * 30.)) as u64),
    }
}

/// For comparing while drawing: no disk access, unlike [`same_folder`].
fn folder_text(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
}

fn same_folder(a: &Path, b: &Path) -> bool {
    worktrees::norm(a) == worktrees::norm(b)
}

impl CrewApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let paths = Paths::system();
        let config = Config::load(&paths);
        let dark = match config.str("theme") {
            "dark" => true,
            "light" => false,
            _ => matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark),
        };
        gpui_component::Theme::change(if dark { gpui_component::ThemeMode::Dark } else { gpui_component::ThemeMode::Light }, None, cx);
        crate::i18n::init(config.str("language"));
        window.set_window_title(&format!("Crew {VERSION}"));

        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("Search")));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t("Ask anything. @ files, $ skills, / commands"))
                .auto_grow(1, 10)
                .submit_on_enter(true)
        });
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&composer, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter { shift: false, .. } => this.submit(window, cx),
                    InputEvent::Change => {
                        this.hint = None;
                        this.update_suggest(cx);
                        cx.notify();
                    }
                    _ => {}
                }
            }),
        ];
        let sidebar_width = config.f32("sidebarWidth", SIDEBAR_WIDTH).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let restored_layout = Layout::restored(config.raw().get("paneLayout"));
        let saved_layout_shape = restored_layout.shape();
        let (service, events) = Service::start(paths.clone());
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update_in(cx, |app, window, cx| app.on_service_event(event, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let stopping = service.clone();
        cx.on_app_quit(move |app: &mut Self, cx| {
            crate::tray::remove();
            // Whatever the agents started goes with them.
            for o in &app.open {
                o.view.read(cx).term.kill_tree();
            }
            stopping.shutdown();
            async {}
        })
        .detach();
        let mut tray_events = crate::tray::start(&paths);
        cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = futures::StreamExt::next(&mut tray_events).await {
                let done = this.update_in(cx, |_, window, cx| match event {
                    crate::tray::TrayEvent::Open => crate::tray::show_window(window),
                    crate::tray::TrayEvent::Quit => cx.quit(),
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
        let closing = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| closing.update(cx, |app, _| app.should_close(window)).unwrap_or(true));
        let mut app = Self {
            paths,
            config,
            projects: Vec::new(),
            search,
            composer,
            open: Vec::new(),
            next_key: 1,
            layout: restored_layout,
            current_project: None,
            sidebar_hidden: false,
            sidebar_width,
            drag: None,
            split_bounds: Rc::default(),
            branch: None,
            notice: None,
            colors: if dark { Colors::dark() } else { Colors::light() },
            palette: if dark { Palette::dark() } else { Palette::light() },
            logo: assets::logo(),
            hosts: Vec::new(),
            roster: Vec::new(),
            providers: Vec::new(),
            provider_images: HashMap::new(),
            show_all: HashSet::new(),
            plan_claude: None,
            plan_codex: None,
            today: None,
            scanner: Arc::default(),
            claude_checked: None,
            codex_checked: None,
            tokens_checked: None,
            usage_forced: 0,
            service,
            menu: None,
            toast: None,
            dialog: None,
            host_editor: None,
            pending_key_file: None,
            prompt_queue: VecDeque::new(),
            prompt_shown: None,
            approvals: Vec::new(),
            agent_editor: None,
            run_cards: Vec::new(),
            run_result: None,
            run_clock: false,
            settings: None,
            provider_editor: None,
            usage_page: None,
            price_editor: None,
            usage_data: None,
            root_focus: cx.focus_handle(),
            focus_return: None,
            files: None,
            git: None,
            browser: None,
            see_through: false,
            image_view: None,
            image_hover: term_links::ImageHover::default(),
            agents_found: HashSet::new(),
            missing_agent: None,
            installing: None,
            project_drop: None,
            saved_layout: saved_layout_shape,
            live_elsewhere: HashMap::new(),
            claude_catalog: None,
            codex_list: None,
            codex_list_loading: false,
            phone: phone::PhoneState::default(),
            hint: None,
            items_cache: HashMap::new(),
            suggest: None,
            renaming: None,
            form: None,
            picker: None,
            file_view: None,
            files_width: None,
            pending_after_save: None,
            deferred_action: None,
            _subscriptions: subscriptions,
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let alive = this.update_in(cx, |app, window, cx| {
                    if window.is_window_active() && app.git.as_ref().is_some_and(|g| !g.busy) {
                        app.git_reload(false, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let mut ticks = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_millis(400)).await;
                ticks = ticks.wrapping_add(1);
                let alive = this.update_in(cx, |app, window, cx| {
                    if window.is_window_active() {
                        app.read_active_screen(cx);
                        if ticks % 15 == 0 {
                            if let Some(key) = app.layout.focused_session() {
                                app.load_session_settings(key, cx);
                            }
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let found = this.update(cx, |app, cx| {
                    let paths = app.paths.clone();
                    let codex: Vec<(String, PathBuf)> = app.projects.iter().flat_map(|p| p.sessions.iter()).filter(|s| s.agent == "codex").map(|s| (s.id.clone(), s.file.clone())).collect();
                    cx.background_spawn(async move {
                        let homes = providers::homes(&paths);
                        crew_core::composer::live_sessions(&paths, &homes, &codex)
                    })
                });
                let Ok(found) = found else { break };
                let live = found.await;
                let alive = this.update(cx, |app, cx| {
                    if app.live_elsewhere != live {
                        app.live_elsewhere = live;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(4)).await;
            }
        })
        .detach();
        app.load_model_lists(cx);
        app.clean_attachments(cx);
        if app.config.bool("browserOpen") {
            app.open_browser(None, window, cx);
        }
        app.rescan(cx);
        app.offer_ego_setup(cx);
        app.refresh_usage(false, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(USAGE_TICK).await;
                // Also counts the reset times down.
                if this.update_in(cx, |app, window, cx| app.refresh_usage(false, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RESCAN).await;
                if this.update(cx, |app, cx| app.rescan(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        app
    }

    /// Closing the window keeps Crew and its running sessions in the tray, if the setting says so and
    /// the icon is there to come back through.
    fn should_close(&mut self, window: &mut Window) -> bool {
        if !self.config.bool("closeToTray") || !crate::tray::is_shown() {
            return true;
        }
        crate::tray::hide_window(window);
        if !self.config.bool("trayHintShown") {
            crate::tray::explain_once();
            self.save_setting("trayHintShown", json!(true));
        }
        false
    }

    fn on_service_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::HostsChanged => self.reload_hosts(cx),
            Event::Prompt(p) => self.on_prompt(p, window, cx),
            Event::PromptCancelled(id) => self.on_prompt_cancelled(id, window, cx),
            Event::Run(run) => self.on_run(run, cx),
            Event::Activity(_) => cx.notify(),
            Event::RemoteChanged => self.refresh_phone(cx),
            Event::RemoteOpen(open) => self.on_remote_open(open, window, cx),
            Event::RemoteAsk(ask) => self.on_remote_ask(ask, window, cx),
        }
    }

    fn by_key(&self, key: u64) -> Option<&OpenSession> {
        self.open.iter().find(|o| o.key == key)
    }

    /// The session in the focused pane: what the prompt box talks to.
    fn active(&self) -> Option<&OpenSession> {
        self.layout.focused_session().and_then(|k| self.by_key(k))
    }

    /// Lists sessions again, away from the window's thread, and applies what changed.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let mut config = self.config.clone();
        let scan = cx.background_spawn(async move {
            let homes = providers::homes(&paths);
            let (list, order_changed) = scan_projects(&paths, &mut config, &homes);
            (list, order_changed.then(|| config.list("projectOrder")), hosts::list(&paths), roster::list(&paths), providers::list(&paths))
        });
        cx.spawn(async move |this, cx| {
            let (list, order, hosts, roster, provider_list) = scan.await;
            let _ = this.update(cx, |app, cx| {
                if app.hosts != hosts || app.roster != roster || app.providers != provider_list {
                    if app.providers != provider_list {
                        app.provider_images = provider_list
                            .iter()
                            .filter_map(|p| Some((p.id.clone(), data_url_image(p.image.as_deref()?)?)))
                            .collect();
                    }
                    app.hosts = hosts;
                    app.roster = roster;
                    app.providers = provider_list;
                    cx.notify();
                }
                if let Some(order) = order {
                    app.save_setting("projectOrder", json!(order));
                }
                app.refresh_branch(cx);
                if app.projects != list {
                    app.projects = list;
                    app.match_open_sessions();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reads the saved numbers, and while Crew is in front runs the live checks that are due: they
    /// start a hidden CLI, so they wait while it's in the background. `force` is the refresh button.
    fn refresh_usage(&mut self, force: bool, window: &Window, cx: &mut Context<Self>) {
        cx.notify();
        if !self.config.bool("showUsage") {
            return;
        }
        let paths = self.paths.clone();
        let saved = cx.background_spawn(async move { (usage::claude_from_hook(&paths), usage::codex_from_logs(&paths)) });
        cx.spawn(async move |this, cx| {
            let (claude, codex) = saved.await;
            let _ = this.update(cx, |app, cx| {
                app.plan_claude = usage::newest(app.plan_claude.take(), claude);
                app.plan_codex = usage::newest(app.plan_codex.take(), codex);
                cx.notify();
            });
        })
        .detach();

        let front = force || window.is_window_active();
        if front && (force || due(self.codex_checked, CODEX_CHECK_EVERY)) {
            self.codex_checked = Some(Instant::now());
            let config = self.config.clone();
            let check = cx.background_spawn(async move { usage::codex_live(&config) });
            self.usage_forced += usize::from(force);
            cx.spawn(async move |this, cx| {
                let live = check.await;
                let _ = this.update(cx, |app, cx| {
                    app.plan_codex = usage::newest(app.plan_codex.take(), live);
                    app.usage_forced -= usize::from(force);
                    cx.notify();
                });
            })
            .detach();
        }
        if front && (force || due(self.claude_checked, CLAUDE_CHECK_EVERY)) {
            self.claude_checked = Some(Instant::now());
            let (config, paths) = (self.config.clone(), self.paths.clone());
            let check = cx.background_spawn(async move { usage_probe::claude(&config, &paths) });
            self.usage_forced += usize::from(force);
            cx.spawn(async move |this, cx| {
                let live = check.await;
                let _ = this.update(cx, |app, cx| {
                    app.plan_claude = usage::newest(app.plan_claude.take(), live);
                    app.usage_forced -= usize::from(force);
                    cx.notify();
                });
            })
            .detach();
        }
        if force || due(self.tokens_checked, TOKENS_EVERY) {
            self.tokens_checked = Some(Instant::now());
            let (paths, scanner) = (self.paths.clone(), self.scanner.clone());
            let scan = cx.background_spawn(async move {
                let homes = providers::homes(&paths);
                scanner.lock().map(|mut s| s.scan(&paths, &homes)).ok()
            });
            cx.spawn(async move |this, cx| {
                let data = scan.await;
                let _ = this.update(cx, |app, cx| {
                    if data.is_some() {
                        app.usage_data = data;
                        app.recompute_today();
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn recompute_today(&mut self) {
        self.today = self.usage_data.as_ref().map(|d| d.today(&self.config));
    }

    /// The theme and text size changed: everything already open follows.
    fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = self.config.str("theme") == "dark";
        gpui_component::Theme::change(if dark { gpui_component::ThemeMode::Dark } else { gpui_component::ThemeMode::Light }, None, cx);
        self.colors = if dark { Colors::dark() } else { Colors::light() };
        self.palette = if dark { Palette::dark() } else { Palette::light() };
        let (palette, size) = (self.palette.clone(), self.config.f32("fontSize", 14.));
        for o in &self.open {
            o.view.update(cx, |v, cx| v.set_appearance(palette.clone(), size, cx));
        }
        self.search.update(cx, |s, cx| s.set_placeholder(t("Search"), window, cx));
        self.composer.update(cx, |c, cx| c.set_placeholder(t("Ask anything. @ files, $ skills, / commands"), window, cx));
        self.recompute_today();
    }

    fn refresh_branch(&mut self, cx: &mut Context<Self>) {
        let head = self.active().and_then(|o| worktrees::head_of(&o.cwd));
        let branch = head.and_then(|h| h.branch.or(h.sha));
        if branch != self.branch {
            self.branch = branch;
            cx.notify();
        }
    }

    /// Writes one setting over what's on disk now, so a change the other app made isn't undone.
    fn save_setting(&mut self, key: &str, value: Value) {
        self.config.set(key, value.clone());
        let mut on_disk = Config::load(&self.paths);
        on_disk.set(key, value);
        if let Err(e) = on_disk.save(&self.paths) {
            self.notice = Some(format!("Couldn't save settings: {e}").into());
        }
    }

    /// Keeps open sessions in step with their files. A new Codex session picks its own id, so its
    /// file is recognised by its folder and by appearing after the session started.
    fn match_open_sessions(&mut self) {
        let sessions: Vec<&Session> = self.projects.iter().flat_map(|p| p.sessions.iter()).collect();
        let mut claimed: Vec<String> = self.open.iter().filter(|o| o.on_disk).map(|o| o.id.clone()).collect();
        for o in &mut self.open {
            if let Some(s) = sessions.iter().find(|s| s.id == o.id && s.agent == o.agent) {
                o.on_disk = true;
                o.title = s.title.clone();
                continue;
            }
            if o.agent != "codex" || o.on_disk {
                continue;
            }
            let found = sessions.iter().find(|s| {
                s.agent == "codex"
                    && s.provider == o.provider
                    && !claimed.contains(&s.id)
                    && s.cwd.as_deref().is_some_and(|c| same_folder(c, &o.cwd))
                    && std::fs::metadata(&s.file)
                        .and_then(|m| m.created())
                        .is_ok_and(|t| t + Duration::from_secs(5) >= o.started)
            });
            if let Some(s) = found {
                providers::rebind(&self.paths, &o.id, &s.id);
                self.service.remote_session_changed(&o.id, &s.id);
                o.id = s.id.clone();
                o.title = s.title.clone();
                o.on_disk = true;
                claimed.push(s.id.clone());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, agent: &str, id: String, cwd: PathBuf, title: Option<String>, resume: bool, provider: Option<String>, prompt: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = None;
        let wanted = agent_setup::PendingStart { agent: agent.to_string(), id: id.clone(), cwd: cwd.clone(), title: title.clone(), resume, provider: provider.clone(), prompt: prompt.map(str::to_string) };
        if !self.agent_ready(wanted, cx) {
            return;
        }
        // A custom provider: the one chosen for a new session, or the one a resumed session ran on,
        // with the model and effort picked in it (kept for the next resume).
        let bound = if resume { providers::binding(&self.paths, &id) } else { None };
        let provider = if resume { bound.as_ref().map(|b| b.provider.clone()) } else { provider };
        let (model, effort) = (bound.as_ref().and_then(|b| b.model.clone()), bound.as_ref().and_then(|b| b.effort.clone()));
        if let Some(pid) = &provider {
            providers::bind(&self.paths, &id, &providers::Binding { provider: pid.clone(), model: model.clone(), effort: effort.clone() });
        }
        let run = provider.as_deref().and_then(|pid| providers::launch(&self.paths, pid, model.as_deref(), effort.as_deref())).filter(|r| r.cli == agent);
        let pre: &[String] = run.as_ref().map_or(&[], |r| r.args.as_slice());
        let argv = agents::agent_argv_on(&self.config, &self.paths, agent, &id, resume, prompt, pre);
        let launch = agents::shell_command(&argv);
        if agent == "codex" {
            self.service.write_codex_rules(); // lets crew-ssh and crew out of its sandbox
        }
        let mut session_env = self.service.session_env(env::clean_env(), &id, agent);
        if let Some(r) = &run {
            providers::apply_env(&mut session_env, &r.env);
        }
        let mut environment: Vec<(String, String)> = session_env.into_iter().collect();
        environment.extend(launch.env);
        let command = Command {
            program: launch.program,
            args: launch.args,
            cwd: Some(agents::work_dir(Some(&cwd), &self.paths)),
            env: environment,
            replace_env: true,
        };
        let term = match Terminal::spawn(&command, 80, 24, self.palette.clone()) {
            Ok(t) => t,
            Err(e) => {
                self.notice = Some(format!("Couldn't start {agent}: {e}").into());
                cx.notify();
                return;
            }
        };
        let key = self.attach(agent, id, cwd, title, resume, Arc::new(term), window, cx);
        if let Some(o) = self.open.iter_mut().find(|o| o.key == key) {
            o.provider = provider;
            // What was picked in it last time.
            o.chips.provider_model = model;
            o.chips.provider_effort = effort;
        }
    }

    /// Gives a running terminal a pane; returns its key.
    #[allow(clippy::too_many_arguments)]
    fn attach(&mut self, agent: &str, id: String, cwd: PathBuf, title: Option<String>, on_disk: bool, term: Arc<Terminal>, window: &mut Window, cx: &mut Context<Self>) -> u64 {
        let (palette, size) = (self.palette.clone(), self.config.f32("fontSize", 14.));
        if agent != "install" {
            self.service.remote_terminal(&id, agent, &cwd.to_string_lossy(), &term);
        }
        let view = cx.new(|cx| TerminalView::new(term, palette, size, cx));
        let key = self.next_key;
        self.next_key += 1;
        cx.subscribe_in(&view, window, move |app, _, event, window, cx| {
            if matches!(event, TerminalEvent::Restart) {
                app.show(key, window, cx);
                app.restart_active(window, cx);
                return;
            }
            if let TerminalEvent::OpenLink(target, ctrl) = event {
                app.on_terminal_link(key, target.clone(), *ctrl, window, cx);
                return;
            }
            if let TerminalEvent::HoverImage(hover) = event {
                app.on_terminal_image_hover(key, hover.clone(), cx);
                return;
            }
            if matches!(event, TerminalEvent::Exited) && app.installing.as_ref().is_some_and(|(k, _)| *k == key) {
                app.installer_exited(key, window, cx);
                return;
            }
            if let Some(o) = app.open.iter_mut().find(|o| o.key == key) {
                match event {
                    TerminalEvent::Exited => o.exited = true,
                    TerminalEvent::Busy(busy) => o.busy = *busy,
                    _ => {}
                }
                cx.notify();
            }
        })
        .detach();
        let worktree = worktrees::Registry::load(&self.paths).session_worktree(&cwd);
        self.open.push(OpenSession { key, id, agent: agent.to_string(), cwd, title, view, started: SystemTime::now(), on_disk, exited: false, busy: false, sent_images: Vec::new(), provider: None, draft: String::new(), worktree, chips: Default::default(), attachments: Vec::new() });
        self.show(key, window, cx);
        key
    }

    fn show(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.layout.focused_session();
        self.layout.show(key);
        self.focus_changed(previous, window, cx);
        // "New session" stays on projects.
        if let Some(cwd) = self.by_key(key).map(|o| o.cwd.clone()).filter(|c| !self.is_host_workspace(c)) {
            self.current_project = Some(cwd);
        }
        self.notice = None;
        self.refresh_branch(cx);
        // A host's terminal takes the keyboard itself; an agent's prompt goes through the box.
        match self.by_key(key).filter(|o| o.agent == "ssh").map(|o| gpui::Focusable::focus_handle(o.view.read(cx), cx)) {
            Some(terminal) => window.focus(&terminal, cx),
            None => self.composer.update(cx, |c, cx| c.focus(window, cx)),
        }
        cx.notify();
    }

    /// The prompt box belongs to the focused pane's session, so text typed for one is never sent to
    /// another: each change of session puts the box's text away and brings out the new one's.
    fn focus_changed(&mut self, previous: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.layout.focused_session();
        if previous != current {
            let text = self.composer.read(cx).value().to_string();
            if let Some(o) = self.open.iter_mut().find(|o| Some(o.key) == previous) {
                o.draft = text;
            }
            let draft = self.open.iter().find(|o| Some(o.key) == current).map(|o| o.draft.clone()).unwrap_or_default();
            // A host's terminal: the box is its command line.
            let host = self.open.iter().find(|o| Some(o.key) == current && o.agent == "ssh").and_then(|o| self.host_of_folder(&o.cwd)).map(|h| h.name.clone());
            let placeholder: SharedString = match host {
                Some(name) if crate::i18n::zh() => format!("输入要在 {name} 上运行的命令").into(),
                Some(name) => format!("Type a command for {name}").into(),
                None => t("Ask anything. @ files, $ skills, / commands").into(),
            };
            self.composer.update(cx, |c, cx| {
                c.set_placeholder(placeholder, window, cx);
                c.set_value(draft, window, cx);
            });
        }
        self.hint = None;
        self.suggest = None;
        if let Some(key) = current {
            self.load_session_settings(key, cx);
        }
        self.refresh_branch(cx);
        self.files_follow_active(window, cx);
        self.git_follow_active(window, cx);
        cx.notify();
    }

    fn open_session(&mut self, session: &Session, project: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.open.iter().find(|o| o.id == session.id && o.agent == session.agent).map(|o| o.key) {
            self.show(key, window, cx);
            return;
        }
        // Opening a session of an archived worktree brings the folder back first.
        if let Some(id) = session.worktree.as_ref().and_then(|w| w.archived_id.clone()) {
            match worktrees::restore_worktree(&self.paths, &id) {
                Ok(back) => {
                    if let Some(note) = back.note {
                        self.worktree_error(note, cx);
                    }
                    self.rescan(cx);
                }
                Err(why) => return self.worktree_error(why, cx),
            }
        }
        let cwd = session.cwd.clone().unwrap_or_else(|| project.to_path_buf());
        self.start(&session.agent.clone(), session.id.clone(), cwd, session.title.clone(), true, session.provider.clone(), None, window, cx);
    }

    fn new_session(&mut self, agent: &str, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.config.str("defaultAgent") != agent {
            self.save_setting("defaultAgent", json!(agent));
        }
        self.start_new_with(agent, cwd, None, None, window, cx);
    }

    fn start_new_with(&mut self, agent: &str, cwd: PathBuf, provider: Option<String>, prompt: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.start(agent, uuid::Uuid::new_v4().to_string(), cwd, None, false, provider, prompt, window, cx);
    }

    /// New Claude, Codex or custom provider session in a folder.
    fn new_session_entries(&self, folder: &Path) -> Vec<overlays::MenuEntry> {
        let zh = crate::i18n::zh();
        let label = |name: &str| if zh { format!("新建 {name} 会话") } else { format!("New {name} session") };
        let mut items: Vec<overlays::MenuEntry> = [("claude", "Claude"), ("codex", "Codex")]
            .into_iter()
            .map(|(agent, name)| {
                let folder = folder.to_path_buf();
                overlays::entry(label(name), move |app, window, cx| app.new_session(agent, folder.clone(), window, cx))
            })
            .collect();
        for (i, p) in self.providers.iter().enumerate() {
            let (folder, id, cli) = (folder.to_path_buf(), p.id.clone(), p.cli());
            let item = overlays::entry(label(&p.name), move |app, window, cx| app.start_new_with(cli, folder.clone(), Some(id.clone()), None, window, cx));
            items.push(if i == 0 { item.separated() } else { item });
        }
        items
    }

    fn default_folder(&self) -> PathBuf {
        self.current_project
            .clone()
            .or_else(|| self.projects.iter().find(|p| p.exists && !self.is_host_workspace(&p.path)).map(|p| p.path.clone()))
            .unwrap_or_else(|| self.paths.home.clone())
    }

    fn default_agent(&self) -> String {
        if self.config.str("defaultAgent") == "codex" { "codex".into() } else { "claude".into() }
    }

    fn close_session(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.open.iter().position(|o| o.key == key) else { return };
        self.open.remove(i).view.read(cx).term.kill();
        self.layout.forget(key);
        // The pane goes on to a session that's running but not showing, if there is one.
        if self.layout.focused_session().is_none() {
            let waiting = self.open.iter().rev().find(|o| self.layout.leaf_of(o.key).is_none()).map(|o| o.key);
            if let Some(next) = waiting {
                self.show(next, window, cx);
            }
        }
        self.refresh_branch(cx);
        cx.notify();
    }

    fn restart_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.layout.focused_session() else { return };
        let Some(i) = self.open.iter().position(|o| o.key == key) else { return };
        let o = self.open.remove(i);
        o.view.read(cx).term.kill();
        self.layout.forget(key);
        // The same session coming back: what's in the box stays, ready to send once it's up.
        let text = self.composer.read(cx).value().to_string();
        if o.agent == "ssh" {
            self.reconnect_host_terminal(&o.id, window, cx);
        } else {
            let files = o.attachments;
            self.start(&o.agent, o.id, o.cwd, o.title, o.on_disk, o.provider.clone(), None, window, cx);
            if let Some(key) = self.layout.focused_session() {
                if let Some(back) = self.open.iter_mut().find(|n| n.key == key) {
                    back.attachments = files;
                }
            }
        }
        self.composer.update(cx, |c, cx| c.set_value(text, window, cx));
    }

    /// Sends the box's text the way a paste would: long text arrives whole, and line breaks in it
    /// don't submit it. An Enter too soon after a long paste counts as part of it, so it waits.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active() else { return };
        if active.exited {
            self.restart_active(window, cx);
            return;
        }
        let shell = active.agent == "ssh";
        let active_key = active.key;
        let term = active.view.read(cx).term.clone();
        let text = self.composer.read(cx).value().to_string();
        if shell {
            if text.trim().is_empty() {
                term.write(b"\r");
                return;
            }
            self.composer.update(cx, |c, cx| c.set_value("", window, cx));
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            let mut lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();
            while lines.len() > 1 && lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            cx.spawn(async move |_, cx| {
                let several = lines.len() > 1;
                for line in lines {
                    term.write(format!("{line}\r").as_bytes());
                    if several {
                        cx.background_executor().timer(Duration::from_millis(120)).await; // let the shell take each line
                    }
                }
            })
            .detach();
            return;
        }
        if self.by_key(active_key).is_some_and(|o| !o.attachments.is_empty()) {
            self.send_with_attachments(active_key, &text, window, cx);
            return;
        }
        let body = text.trim().replace("\r\n", "\r").replace('\n', "\r");
        if body.is_empty() {
            term.write(b"\r"); // confirm a prompt shown in the terminal
            return;
        }
        self.composer.update(cx, |c, cx| c.set_value("", window, cx));
        let key = active_key;
        let body = self.with_skills(key, &body);
        let body = agents_panel::with_agent_note(&body, &self.roster).replace('\n', "\r");
        term.write(format!("\x1b[200~{body}\x1b[201~").as_bytes());
        let wait = if body.chars().count() > 200 { 700 } else { 150 };
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            term.write(b"\r");
        })
        .detach();
    }

    /// While the agent works and the box is empty, the send button stops it: Esc interrupts both.
    fn can_stop(&self, cx: &Context<Self>) -> bool {
        self.active().is_some_and(|o| o.agent != "ssh" && o.busy && !o.exited && o.attachments.is_empty()) && self.composer.read(cx).value().trim().is_empty()
    }

    fn send_or_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_stop(cx) {
            if let Some(o) = self.active() {
                o.view.read(cx).term.write(b"\x1b");
            }
            self.composer.update(cx, |c, cx| c.focus(window, cx));
        } else if !self.composer.read(cx).value().trim().is_empty() || self.active().is_some_and(|o| !o.attachments.is_empty()) {
            self.submit(window, cx);
        }
    }

    /// With an empty box, keys that drive the agent's own menus go to the terminal.
    fn composer_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active().filter(|o| !o.exited) else { return };
        let m = &event.keystroke.modifiers;
        let seq: Option<&[u8]> = match event.keystroke.key.as_str() {
            "tab" if m.shift => Some(b"\x1b[Z"),
            "escape" if !m.shift => Some(b"\x1b"),
            "up" if !m.shift => Some(b"\x1b[A"),
            "down" if !m.shift => Some(b"\x1b[B"),
            _ => None,
        };
        let Some(seq) = seq else { return };
        if event.keystroke.key != "tab" && (!self.composer.read(cx).value().is_empty() || !active.attachments.is_empty()) {
            return;
        }
        active.view.read(cx).term.write(seq);
        cx.stop_propagation();
    }

    /// Before anything else sees the key, or a terminal would turn it into a control character.
    fn window_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control && k.key == "s" && self.layout.focused_session() == Some(FILE_KEY) {
            self.save_file(cx);
            cx.stop_propagation();
            return;
        }
        if k.modifiers.control && !k.modifiers.alt && self.browser_key(&k.key, k.modifiers.shift, window, cx) {
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && self.browser_escape(window, cx) {
            cx.stop_propagation();
            return;
        }
        if k.key == "f5" && self.git.is_some() {
            self.git_reload(true, cx);
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && self.close_top_overlay() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if k.modifiers.control && !k.modifiers.alt && (k.key == "\\" || k.key == "|") {
            self.split(if k.modifiers.shift { Dir::Column } else { Dir::Row }, window, cx);
            cx.stop_propagation();
        }
    }

    /// Esc closes what's on top, one layer at a time. Questions an agent is waiting on stay.
    fn close_top_overlay(&mut self) -> bool {
        if self.image_view.take().is_some() || self.menu.take().is_some() || self.dialog.take().is_some() || self.pending_after_save.take().is_some() || self.form.take().is_some() || self.picker.take().is_some() || self.renaming.take().is_some() {
            return true;
        }
        if self.prompt_shown.is_some() {
            return false;
        }
        self.price_editor.take().is_some()
            || self.provider_editor.take().is_some()
            || self.run_result.take().is_some()
            || self.agent_editor.take().is_some()
            || self.host_editor.take().is_some()
            || self.settings.take().is_some()
            || self.usage_page.take().is_some()
    }

    /// They share the right side.
    fn close_other_right_panels(&mut self) {
        self.git = None;
    }

    fn split(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.layout.focused_session();
        self.layout.split(dir);
        self.focus_changed(previous, window, cx);
    }

    /// What you grab to resize a panel: wider than the line it sits on, half on each side. Goes last
    /// in a positioned parent, so nothing on either side takes the press.
    fn resize_grip(&self, id: &'static str, drag: Drag, left: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let active = self.drag.is_some_and(|d| std::mem::discriminant(&d) == std::mem::discriminant(&drag));
        div()
            .id(id)
            .group(id)
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(left))
            .w(px(10.))
            .occlude()
            .cursor_col_resize()
            .on_mouse_down(MouseButton::Left, cx.listener(move |app, _, _, cx| {
                app.drag = Some(drag);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().absolute().top_0().bottom_0().left(px(3.)).w(px(4.)).when(active, |d| d.bg(c.border)).group_hover(id, move |s| s.bg(c.border)))
            .into_any_element()
    }

    /// While a divider is dragged, this lies over everything: panels that block the mouse don't stop
    /// the drag, terminals don't select text, and letting go is seen wherever it happens.
    fn render_drag_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let row = matches!(self.drag, Some(Drag::Split { dir: Dir::Column, .. }));
        let (moved, released) = (cx.entity(), cx.entity());
        div()
            .id("drag-layer")
            .absolute()
            .inset_0()
            .occlude()
            .map(|d| if row { d.cursor_row_resize() } else { d.cursor_col_resize() })
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let moved = moved.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == gpui::DispatchPhase::Capture {
                                moved.update(cx, |app, cx| app.mouse_moved(event, window, cx));
                            }
                        });
                        let released = released.clone();
                        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                            if phase == gpui::DispatchPhase::Capture && event.button == MouseButton::Left {
                                released.update(cx, |app, cx| app.mouse_released(cx));
                            }
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn mouse_moved(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };
        match drag {
            Drag::Sidebar => self.sidebar_width = f32::from(event.position.x).clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            Drag::Browser => {
                let right = f32::from(window.viewport_size().width);
                let room = right - if self.sidebar_hidden { 0. } else { self.sidebar_width };
                self.set_browser_width((right - f32::from(event.position.x)).clamp(320., (room - 320.).max(320.)));
            }
            Drag::Git => {
                let right = f32::from(window.viewport_size().width) - self.browser_shown_width();
                let room = right - if self.sidebar_hidden { 0. } else { self.sidebar_width };
                let width = right - f32::from(event.position.x);
                if let Some(g) = self.git.as_mut() {
                    if g.diff_open {
                        g.wide_width = width.clamp(460., (room - 60.).max(460.));
                    } else {
                        g.list_width = width.clamp(300., (room - 360.).max(300.));
                    }
                }
            }
            Drag::Files => {
                let right = f32::from(window.viewport_size().width) - self.browser_shown_width();
                // Keep the terminal usable.
                let most = (right - self.sidebar_width - 360.).max(220.);
                self.files_width = Some((right - f32::from(event.position.x)).clamp(220., most));
            }
            Drag::Split { id, dir } => {
                let Some(b) = self.split_bounds.borrow().get(&id).copied() else { return };
                let ratio = match dir {
                    Dir::Row => (event.position.x - b.origin.x) / b.size.width,
                    Dir::Column => (event.position.y - b.origin.y) / b.size.height,
                };
                self.layout.set_ratio(id, ratio);
            }
        }
        cx.notify();
    }

    fn mouse_released(&mut self, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Sidebar) => self.save_setting("sidebarWidth", json!(self.sidebar_width)),
            Some(Drag::Files) => {
                if let Some(width) = self.files_width {
                    self.save_setting("filesWidth2", json!(width));
                }
            }
            Some(Drag::Browser) => self.save_browser_width(),
            Some(Drag::Git) => {
                if let Some((list, wide)) = self.git.as_ref().map(|g| (g.list_width, g.wide_width)) {
                    self.save_setting("gitListWidth", json!(list));
                    self.save_setting("gitWidth", json!(wide));
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn add_project(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(folder) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |app, cx| {
                let text = folder.to_string_lossy().to_string();
                let hidden: Vec<String> = app.config.list("hiddenProjects").into_iter().filter(|h| !same_folder(Path::new(h), &folder)).collect();
                app.save_setting("hiddenProjects", json!(hidden));
                let mut extra = app.config.list("extraProjects");
                if !extra.iter().any(|x| same_folder(Path::new(x), &folder)) {
                    extra.push(text);
                    app.save_setting("extraProjects", json!(extra));
                }
                app.current_project = Some(folder);
                app.rescan(cx);
            });
        })
        .detach();
    }

    fn toggle_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        let key = path.to_string_lossy().to_string();
        self.config.toggle_collapsed(&key);
        let collapsed = self.config.raw().get("collapsed").cloned().unwrap_or(json!({}));
        self.save_setting("collapsed", collapsed);
        self.current_project = Some(path.to_path_buf());
        cx.notify();
    }

    fn toggle_collapsed(&mut self, key: &str, cx: &mut Context<Self>) {
        self.config.toggle_collapsed(key);
        let collapsed = self.config.raw().get("collapsed").cloned().unwrap_or(json!({}));
        self.save_setting("collapsed", collapsed);
        cx.notify();
    }

    fn toggle_setting(&mut self, key: &str, cx: &mut Context<Self>) {
        let on = !self.config.bool(key);
        self.save_setting(key, json!(on));
        cx.notify();
    }

    fn agent_badge(&self, agent: &str, size: f32) -> AnyElement {
        let c = self.colors;
        let (path, color) = if agent == "codex" { (assets::OPENAI, c.text) } else { (assets::CLAUDE, c.accent) };
        div().flex().flex_none().text_color(color).child(Icon::empty().path(path).size(px(size))).into_any_element()
    }

    /// Its image, its emoji, or its first letter on a colour of its own.
    fn provider_icon(&self, p: &Provider) -> AnyElement {
        let base = div().flex().flex_none().items_center().justify_center().size(px(16.)).rounded(px(4.)).overflow_hidden();
        if let Some(image) = self.provider_images.get(&p.id) {
            return base.child(img(image.clone()).size_full().object_fit(ObjectFit::Contain)).into_any_element();
        }
        if let Some(emoji) = &p.emoji {
            return base.text_size(px(12.)).child(emoji.clone()).into_any_element();
        }
        let hue = p.id.chars().fold(0u32, |h, c| (h * 31 + c as u32) % 360);
        let letter = p.name.trim().chars().next().unwrap_or('?').to_uppercase().to_string();
        base.bg(hsla(hue as f32 / 360., 0.55, 0.48, 1.)).text_color(gpui::white()).text_size(px(10.)).child(letter).into_any_element()
    }

    fn icon_button(&self, id: impl Into<gpui::ElementId>, icon: impl Into<Icon>) -> gpui::Stateful<gpui::Div> {
        self.row_button(id, icon).occlude()
    }

    /// A button a row shows while hovered. It mustn't block the mouse: the row would count as no
    /// longer hovered, and hide the button as soon as the mouse reached it.
    fn row_button(&self, id: impl Into<gpui::ElementId>, icon: impl Into<Icon>) -> gpui::Stateful<gpui::Div> {
        let c = self.colors;
        div()
            .id(id)
            .flex()
            .flex_none()
            .p(px(6.))
            .rounded(px(8.))
            .text_color(c.muted)
            .cursor_pointer()
            .hover(move |s| s.bg(c.hover).text_color(c.text))
            .child(Icon::new(icon).size(px(16.)))
    }

    fn render_session(&self, session: &Session, project: &Path, cx: &mut Context<Self>) -> AnyElement {
        let draggable_project = project.to_path_buf();
        let c = self.colors;
        let open = self.open.iter().find(|o| o.id == session.id && o.agent == session.agent);
        let is_active = open.is_some_and(|o| Some(o.key) == self.layout.focused_session());
        let elsewhere = self.live_elsewhere.get(&session.id).copied();
        let running = open.is_some_and(|o| !o.exited) || (open.is_none() && elsewhere.is_some());
        let working = running && open.map_or(elsewhere == Some(true), |o| o.busy);
        if let Some(row) = self.render_rename_row(&session.id) {
            return row;
        }
        let title = session.title.clone().unwrap_or_default();
        let s = session.clone();
        let (menu_session, menu_project, more_session, more_project) = (session.clone(), project.to_path_buf(), session.clone(), project.to_path_buf());
        let row_group = SharedString::from(format!("session-group-{}", session.id));
        let shown_here = open_key_of(open).and_then(|k| self.layout.leaf_of(k)).is_some();
        let menu_open = self.menu_owner() == Some(format!("session:{}", session.id).as_str());
        let open_key = self.open.iter().find(|o| o.id == session.id && o.agent == session.agent).map(|o| o.key);
        let project = project.to_path_buf();
        let row = div().id(SharedString::from(format!("session-{}-{}", session.agent, session.id)));
        self.session_drag_handler(row, session, &draggable_project)
            .relative()
            .when(shown_here && !is_active, |d| d.child(self.shown_mark()))
            .flex()
            .flex_col()
            .gap(px(2.))
            .py(px(7.))
            .ml(px(SESSION_INDENT))
            .pl(px(10.))
            .pr(px(8.))
            .my(px(2.))
            .rounded(px(10.))
            .cursor_pointer()
            .when(working && !is_active, |d| d.bg(c.accent.opacity(0.07)))
            .when(is_active, |d| {
                d.bg(c.card).shadow(vec![
                    BoxShadow { color: c.card_edge, offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false },
                    BoxShadow { color: c.card_edge, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: false },
                ])
            })
            .when(!is_active, |d| d.hover(move |h| h.bg(c.hover)))
            .when(!is_active && menu_open, |d| d.bg(c.hover))
            .group(row_group.clone())
            .on_click(cx.listener(move |app, e: &gpui::ClickEvent, window, cx| {
                if e.click_count() == 2 && !s.file.as_os_str().is_empty() {
                    app.start_rename(&s, window, cx);
                } else {
                    app.open_session(&s, &project, window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| app.session_menu(menu_session.clone(), menu_project.clone(), e.position, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w(px(0.))
                    .min_h(px(20.))
                    .text_size(px(13.5))
                    .text_color(c.text)
                    .relative()
                    .when(working, |d| d.child(Spinner::new().color(c.accent).xsmall()))
                    .when(running && !working, |d| d.child(div().flex_none().size(px(7.)).rounded_full().bg(c.running)))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(title))
                    .when(session.mtime > 0., |d| d.child(div().flex_none().text_size(px(12.)).text_color(c.muted).when(menu_open, |d| d.invisible()).group_hover(row_group.clone(), |s| s.invisible()).child(rel_time(session.mtime))))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .flex()
                            .items_center()
                            .opacity(if menu_open { 1. } else { 0. })
                            .group_hover(row_group.clone(), |s| s.opacity(1.))
                            .when_some(open_key, |d, key| {
                                d.child(self.row_button(SharedString::from(format!("session-close-{}", session.id)), IconName::Close).p(px(2.)).on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.close_session(key, window, cx);
                                })))
                            })
                            .child(self.row_button(SharedString::from(format!("session-more-{}", session.id)), Icon::empty().path(assets::DOTS)).p(px(2.)).on_click(cx.listener(move |app, e: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                app.session_menu(more_session.clone(), more_project.clone(), e.position(), cx);
                            }))),
                    ),
            )
            .map(|d| {
                let tree = session.worktree.as_ref().map(|w| {
                    let archived = w.archived_id.is_some();
                    div().flex_none().max_w(px(120.)).truncate().px(px(5.)).rounded(px(4.)).bg(c.hover).when(archived, |d| d.opacity(0.6)).child(w.name.clone())
                });
                if working {
                    d.child(div().flex().items_center().gap(px(6.)).text_size(px(12.)).text_color(c.accent).child(t("Working…")).children(tree))
                } else if session.branch.is_some() || tree.is_some() {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .min_w(px(0.))
                            .text_size(px(12.))
                            .text_color(c.muted)
                            .when_some(session.branch.clone(), |d, branch| d.child(Icon::empty().path(assets::GIT_BRANCH).size(px(12.))).child(div().min_w(px(0.)).truncate().child(branch)))
                            .children(tree),
                    )
                } else {
                    d
                }
            })
            .into_any_element()
    }

    /// Claude first, then GPT, then one group per custom provider. Empty ones aren't listed.
    fn session_groups<'a>(&self, sessions: &[&'a Session], project_key: &str) -> Vec<SessionGroup<'a>> {
        let mut groups = Vec::new();
        for (agent, label) in [("claude", "Claude"), ("codex", "GPT")] {
            groups.push(SessionGroup {
                key: format!("{project_key}::{agent}"),
                label: label.into(),
                icon: self.agent_badge(agent, 12.),
                agent,
                provider: None,
                sessions: sessions.iter().copied().filter(|s| s.agent == agent && s.provider.is_none()).collect(),
            });
        }
        for p in &self.providers {
            groups.push(SessionGroup {
                key: format!("{project_key}::provider:{}", p.id),
                label: p.name.clone(),
                icon: self.provider_icon(p),
                agent: p.cli(),
                provider: Some(p.id.clone()),
                sessions: sessions.iter().copied().filter(|s| s.provider.as_deref() == Some(&p.id)).collect(),
            });
        }
        groups.retain(|g| !g.sessions.is_empty());
        groups
    }

    fn render_group(&self, group: SessionGroup, project: &Path, searching: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = self.colors;
        let total = group.sessions.len();
        let folded = !searching && self.config.is_collapsed(&group.key);
        let hover = SharedString::from(format!("group-{}", group.key));
        let (toggle, add_key, more_key) = (group.key.clone(), group.key.clone(), group.key.clone());
        let group_provider = group.provider.clone();
        let add_folder = project.to_path_buf();
        let agent = group.agent;
        let head = div()
            .id(SharedString::from(format!("group-row-{}", group.key)))
            .group(hover.clone())
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(28.))
            .py(px(4.))
            .pl(px(10.))
            .pr(px(8.))
            .mt(px(2.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c.muted)
            .hover(move |s| s.bg(c.hover))
            .on_click(cx.listener(move |app, _, _, cx| app.toggle_collapsed(&toggle, cx)))
            .child(group.icon)
            .child(div().flex_1().min_w(px(0.)).truncate().child(group.label))
            .child(div().flex_none().font_weight(gpui::FontWeight::NORMAL).child(total.to_string()))
            .child(
                div().opacity(0.).group_hover(hover, |s| s.opacity(1.)).child(
                    self.row_button(SharedString::from(format!("group-new-{add_key}")), IconName::Plus).p(px(2.)).on_click(cx.listener(
                        move |app, _, window, cx| {
                            cx.stop_propagation();
                            app.start_new_with(agent, add_folder.clone(), group_provider.clone(), None, window, cx);
                        },
                    )),
                ),
            )
            .child(
                div().flex_none().text_color(c.muted).child(
                    Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(13.)),
                ),
            )
            .into_any_element();
        let mut rows = vec![head];
        if folded {
            return rows;
        }
        let limit = if searching || self.show_all.contains(&group.key) { usize::MAX } else { SESSIONS_SHOWN };
        rows.extend(group.sessions.iter().take(limit).map(|s| self.render_session(s, project, cx)));
        if total > limit {
            let label = show_more(total - limit);
            rows.push(
                div()
                    .id(SharedString::from(format!("group-more-{more_key}")))
                    .py(px(7.))
                    .ml(px(SESSION_INDENT))
                    .pl(px(10.))
                    .rounded(px(10.))
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(c.muted)
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.show_all.insert(more_key.clone());
                        cx.notify();
                    }))
                    .child(label)
                    .into_any_element(),
            );
        }
        rows
    }

    /// A folding section under the projects: Hosts or Agents.
    fn section_head(&self, id: &'static str, title: &'static str, setting: &'static str, actions: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let collapsed = self.config.bool(setting);
        div()
            .flex()
            .flex_none()
            .items_center()
            .mt(px(4.))
            .pt(px(6.))
            .pb(px(2.))
            .pl(px(12.))
            .pr(px(4.))
            .border_t_1()
            .border_color(c.border)
            .text_size(px(13.))
            .text_color(c.muted)
            .child(
                div()
                    .id(id)
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .hover(move |s| s.text_color(c.text))
                    .on_click(cx.listener(move |app, _, _, cx| app.toggle_setting(setting, cx)))
                    .child(t(title))
                    .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(13.))),
            )
            .child(actions)
            .into_any_element()
    }

    /// The 5-hour window first (percent used, time until it resets), then the weekly one.
    fn usage_item(&self, agent: &str, plan: &PlanUsage, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let now = usage::now_ms();
        // A window whose reset time has passed starts again from zero.
        let wins: Vec<PlanWindow> = plan
            .windows
            .iter()
            .map(|w| if w.resets_at.is_some_and(|r| r <= now) { PlanWindow { used: 0., resets_at: None, minutes: w.minutes } } else { w.clone() })
            .collect();
        let first = &wins[0];
        let level = if first.used >= 90. { gpui::rgb(0xc4462d).into() } else if first.used >= 70. { gpui::rgb(0xc98a12).into() } else { c.text };
        let details = usage_details(agent, plan, &wins, now);
        div()
            .id(SharedString::from(format!("usage-plan-{agent}")))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .px(px(6.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(move |s| s.bg(c.hover).text_color(c.text))
            .on_click(cx.listener(|app, _, window, cx| app.open_usage_limits(window, cx)))
            .when(self.menu.is_none(), |d| d.tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(details.clone()).build(window, cx)))
            .when(now - plan.at > 6 * 3_600_000, |d| d.opacity(0.6))
            .child(self.agent_badge(agent, 13.))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(level).child(format!("{}%", first.used.round() as i64)))
            .when_some(first.resets_at, |d, r| d.child(usage::until_text(r - now)))
            .when_some(wins.get(1), |d, second| {
                d.child(div().opacity(0.8).child(format!("· {} {}%", window_name(second.minutes), second.used.round() as i64)))
            })
            .into_any_element()
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let plans: Vec<AnyElement> = [("claude", &self.plan_claude), ("codex", &self.plan_codex)]
            .into_iter()
            .filter_map(|(agent, plan)| plan.as_ref().filter(|p| !p.windows.is_empty()).map(|p| self.usage_item(agent, p, cx)))
            .collect();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(14.))
            .h(px(26.))
            .pl(px(12.))
            .pr(px(8.))
            .border_t_1()
            .border_color(c.border)
            .bg(c.side_bg)
            .text_color(c.muted)
            .text_size(px(12.))
            .whitespace_nowrap()
            .overflow_hidden()
            .child(div().flex_none().child(t("Usage")))
            .child(
                div().flex().items_center().gap(px(4.)).min_w(px(0.)).overflow_hidden().map(|d| {
                    if plans.is_empty() { d.child(t("Checking plan usage…")) } else { d.children(plans) }
                }),
            )
            .when_some(self.today, |d, today| {
                d.child(
                    div()
                        .id("usage-today")
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(5.))
                        .px(px(6.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(c.hover).text_color(c.text))
                        .on_click(cx.listener(|app, _, window, cx| app.open_usage_costs(window, cx)))
                        .child(Icon::empty().path(assets::CHART).size(px(13.)))
                        .child(format!("{} {} · {}", t("Today"), usage::fmt_usd(today.cost), usage::fmt_tokens(today.tokens))),
                )
            })
            .map(|d| {
                if self.usage_forced > 0 {
                    d.child(div().flex_none().p(px(2.)).child(Spinner::new().color(c.muted).xsmall()))
                } else {
                    d.child(
                        self.icon_button("usage-refresh", IconName::RefreshCw).p(px(2.)).on_click(cx.listener(|app, _, window, cx| app.refresh_usage(true, window, cx))),
                    )
                }
            })
            .into_any_element()
    }

    fn render_project(&self, project: &Project, query: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let matches = |s: &Session| s.title.as_deref().is_some_and(|t| t.to_lowercase().contains(query));
        let name_matches = query.is_empty() || project.name.to_lowercase().contains(query);
        // A new session has no file until its first prompt, but it still needs a row to come back to.
        let pending: Vec<Session> = self
            .open
            .iter()
            .filter(|o| !o.on_disk && name_matches && folder_text(o.worktree.as_ref().map_or(&o.cwd, |w| &w.main)) == folder_text(&project.path))
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
                worktree: o.worktree.clone(),
            })
            .collect();
        let sessions: Vec<&Session> = pending.iter().chain(project.sessions.iter().filter(|s| name_matches || matches(s))).collect();
        if !name_matches && sessions.is_empty() {
            return None;
        }
        let key = project.path.to_string_lossy().to_string();
        let collapsed = query.is_empty() && self.config.is_collapsed(&key);
        let (toggle, add) = (project.path.clone(), project.path.clone());
        let (menu_path, more_path) = (project.path.clone(), project.path.clone());
        let group = SharedString::from(format!("project-{key}"));
        let menu_open = self.menu_owner() == Some(format!("project:{}", project.path.display()).as_str());
        Some(
            div()
                .flex()
                .flex_col()
                .child(
                    self.project_drag_handlers(div().id(SharedString::from(format!("project-row-{key}"))), project, cx)
                        .group(group.clone())
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .py(px(4.))
                        .pl(px(12.))
                        .pr(px(8.))
                        .min_h(px(34.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_size(px(14.))
                        .text_color(if project.exists { c.text } else { c.muted })
                        .hover(move |s| s.bg(c.hover))
                        .when(menu_open, |d| d.bg(c.hover))
                        .on_click(cx.listener(move |app, _, _, cx| app.toggle_project(&toggle, cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| app.project_menu(menu_path.clone(), e.position, cx)))
                        .child(div().flex_none().text_color(c.text).child(Icon::empty().path(assets::FOLDER).size(px(16.))))
                        .child(div().flex_1().min_w(px(0.)).truncate().child(project.name.clone()))
                        .child(
                            div().opacity(if menu_open { 1. } else { 0. }).group_hover(group.clone(), |s| s.opacity(1.)).child(
                                self.row_button(SharedString::from(format!("project-new-{key}")), IconName::Plus)
                                    .p(px(3.))
                                    .on_click(cx.listener(move |app, e: &gpui::ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        let items = app.new_session_entries(&add);
                                        app.open_menu(e.position(), items, cx);
                                        app.set_menu_owner(format!("project:{}", add.display()));
                                    })),
                            ),
                        )
                        .child(
                            div().opacity(if menu_open { 1. } else { 0. }).group_hover(group, |s| s.opacity(1.)).child(
                                self.row_button(SharedString::from(format!("project-more-{key}")), Icon::empty().path(assets::DOTS))
                                    .p(px(3.))
                                    .on_click(cx.listener(move |app, e: &gpui::ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        app.project_menu(more_path.clone(), e.position(), cx);
                                    })),
                            ),
                        )
                        .child(
                            div().flex_none().text_color(c.muted).child(
                                Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(14.)),
                            ),
                        ),
                )
                .when(!collapsed && !sessions.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .ml(px(18.))
                            .pl(px(6.))
                            .mb(px(6.))
                            .border_l_1()
                            .border_color(c.border)
                            .children(self.session_groups(&sessions, &key).into_iter().flat_map(|g| {
                                self.render_group(g, &project.path, !query.is_empty(), cx)
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let query = self.search.read(cx).value().trim().to_lowercase();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.sidebar_width))
            .h_full()
            .bg(c.side_bg)
            .px(px(8.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .gap(px(9.))
                    .h(px(44.))
                    .pl(px(12.))
                    .text_size(px(18.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c.text)
                    .child(img(self.logo.clone()).flex_none().size(px(24.)))
                    .child("Crew")
                    .child(div().pt(px(4.)).text_size(px(11.5)).font_weight(gpui::FontWeight::NORMAL).text_color(c.muted).child(format!("v{VERSION}"))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .gap(px(2.))
                    .pb(px(8.))
                    .child(
                        div().flex_1().min_w(px(0.)).child(
                            Input::new(&self.search)
                                .appearance(false)
                                .small()
                                .prefix(div().text_color(c.muted).child(Icon::new(IconName::Search).size(px(16.)))),
                        ),
                    )
                    .child(self.icon_button("add-project", Icon::empty().path(assets::FOLDER_PLUS)).on_click(cx.listener(|app, _, _, cx| app.add_project(cx))))
                    .child(self.icon_button("new-session", Icon::empty().path(assets::SQUARE_PEN)).on_mouse_down(MouseButton::Left, cx.listener(|app, e: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let items = app.new_session_entries(&app.default_folder());
                        app.open_menu(e.position, items, cx);
                    })))
                    .child(self.icon_button("open-settings", Icon::empty().path(assets::SETTINGS)).on_click(cx.listener(|app, _, window, cx| app.open_settings(window, cx)))),
            )
            .child(
                div()
                    .id("project-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .pb(px(12.))
                    .children(
                        self.projects
                            .iter()
                            // A host's agent sessions belong to the host, not to the Projects list.
                            .filter(|p| !self.is_host_workspace(&p.path))
                            .filter_map(|p| self.render_project(p, &query, cx)),
                    ),
            )
            .child(self.render_hosts(cx))
            .child(self.render_roster(cx))
            .into_any_element()
    }

    fn render_header(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        // The title bar's own wrapper won't shrink, so what's in it is given the room there is: the
        // window, less the sidebar and the window's three buttons.
        let sidebar = if self.sidebar_hidden { 0. } else { self.sidebar_width + 1. };
        let room = (f32::from(window.viewport_size().width) - sidebar - 12. - 3. * 46.).max(120.);
        let active = self.active();
        let showing_file = self.layout.focused_session() == Some(FILE_KEY);
        let title = if showing_file { self.file_title().unwrap_or_default() } else { active.map(|o| o.title.clone().unwrap_or_else(|| t("New session").into())).unwrap_or_default() };
        let path = if showing_file { self.file_path_text().unwrap_or_default() } else { active.map(|o| self.session_place(o)).unwrap_or_default() };
        let active_key = active.map(|o| o.key);
        TitleBar::new()
            .bg(c.main_bg)
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .w(px(room))
                    .gap(px(8.))
                    .child(
                        self.icon_button("toggle-sidebar", if self.sidebar_hidden { IconName::PanelLeftOpen } else { IconName::PanelLeft })
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.sidebar_hidden = !app.sidebar_hidden;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_none().max_w(px(420.)).truncate().text_size(px(13.5)).text_color(c.text).child(title))
                    .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(12.)).text_color(c.muted).child(path))
                    .when_some(active_key, |d, key| {
                        d.child(self.icon_button("restart", IconName::RefreshCw).on_click(cx.listener(|app, _, window, cx| app.restart_active(window, cx))))
                            .child(self.icon_button("close", IconName::Close).on_click(cx.listener(move |app, _, window, cx| app.close_session(key, window, cx))))
                    })
                    .child(self.icon_button("split-right", Icon::empty().path(assets::SPLIT_RIGHT)).on_click(cx.listener(|app, _, window, cx| app.split(Dir::Row, window, cx))))
                    .child(self.icon_button("split-down", Icon::empty().path(assets::SPLIT_DOWN)).on_click(cx.listener(|app, _, window, cx| app.split(Dir::Column, window, cx))))
                    .when(active_key.is_some() || self.files.is_some(), |d| {
                        let on = self.files.is_some();
                        d.child(self.icon_button("toggle-files", Icon::empty().path(assets::FILES)).when(on, |d| d.text_color(c.accent)).on_click(cx.listener(|app, _, window, cx| app.toggle_files(window, cx))))
                    })
                    .when(active_key.is_some() || self.git.is_some(), |d| {
                        let on = self.git.is_some();
                        d.child(self.icon_button("toggle-git", Icon::empty().path(assets::GIT_BRANCH)).when(on, |d| d.text_color(c.accent)).on_click(cx.listener(|app, _, window, cx| app.toggle_git(window, cx))))
                    })
                    .child(self.icon_button("toggle-browser", Icon::empty().path(assets::GLOBE)).when(self.browser.is_some(), |d| d.text_color(c.accent)).on_click(cx.listener(|app, _, window, cx| {
                        if app.browser.is_some() {
                            app.close_browser(cx);
                        } else {
                            app.open_browser(None, window, cx);
                        }
                    }))),
            )
            .into_any_element()
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(o) = self.active() else { return div().into_any_element() };
        let empty = self.composer.read(cx).value().trim().is_empty() && o.attachments.is_empty();
        let stop = self.can_stop(cx);
        let project_name = self.host_of_folder(&o.cwd).map(|h| h.name.clone()).or_else(|| o.cwd.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default();
        let chip = |content: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(8.))
                .py(px(3.))
                .rounded(px(7.))
                .text_size(px(12.))
                .text_color(c.muted)
                .child(content)
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(4.))
            .pt(px(6.))
            .px(px(20.))
            .pb(px(12.))
            .bg(c.main_bg)
            .child(
                div()
                    .capture_key_down(cx.listener(Self::composer_key))
                    .capture_action(cx.listener(|app, _: &gpui_component::input::MoveUp, window, cx| {
                        if app.suggest_key("up", window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|app, _: &gpui_component::input::MoveDown, window, cx| {
                        if app.suggest_key("down", window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|app, _: &gpui_component::input::IndentInline, window, cx| {
                        if app.suggest_key("tab", window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|app, _: &gpui_component::input::Escape, window, cx| {
                        if app.suggest_key("escape", window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|app, action: &gpui_component::input::Enter, window, cx| {
                        if !action.shift && app.suggest_key("enter", window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|app, _, _, cx| app.update_suggest(cx)))
                    .relative()
                    .children(self.render_suggest(cx))
                    .capture_action(cx.listener(|app, _: &gpui_component::input::Paste, window, cx| {
                        if app.paste_attachments(window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .on_drop(cx.listener(|app, paths: &gpui::ExternalPaths, window, cx| app.drop_attachments(paths, window, cx)))
                    .flex()
                    .flex_col()
                    .pt(px(12.))
                    .pr(px(12.))
                    .pb(px(8.))
                    .pl(px(6.))
                    .rounded(px(18.))
                    .on_mouse_down(MouseButton::Left, cx.listener(|app, _, window, cx| {
                        // Stopped here, or the window would take the keyboard back.
                        cx.stop_propagation();
                        app.composer.update(cx, |c, cx| c.focus(window, cx));
                    }))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.card)
                    .children(self.render_attachments(o, cx))
                    .child(div().when(o.agent == "ssh", |d| d.font_family("Cascadia Mono").text_size(px(13.))).child(Textarea::new(&self.composer).appearance(false)))
                    .child(div().flex().items_center().gap(px(8.)).mt(px(8.)).children(self.render_attach_button(o, cx)).child(div().flex_1().min_w(px(0.)).truncate().text_size(px(12.)).text_color(c.muted).children(self.composer_hint(o))).child(
                        div()
                            .id("composer-send")
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(30.))
                            .rounded_full()
                            .text_color(gpui::white())
                            .bg(if stop { gpui::rgb(0xe5484d).into() } else { c.accent })
                            .when(empty && !stop, |d| d.opacity(0.25))
                            .when(!empty || stop, |d| d.cursor_pointer())
                            .on_click(cx.listener(|app, _, window, cx| app.send_or_stop(window, cx)))
                            .map(|d| {
                                if stop {
                                    d.child(div().size(px(11.)).rounded(px(2.)).bg(gpui::white()))
                                } else {
                                    d.child(Icon::new(IconName::ArrowUp).size(px(17.)))
                                }
                            }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(self.render_project_chip(project_name, cx))
                    .when_some(self.branch.clone(), |d, branch| {
                        let folder = o.cwd.clone();
                        d.child(
                            div()
                                .id("chip-branch")
                                .cursor_pointer()
                                .rounded(px(7.))
                                .hover(move |s| s.bg(c.hover))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |app, e: &gpui::MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    app.open_branch_picker(e.position, folder.clone(), true, window, cx);
                                }))
                                .child(chip(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(Icon::empty().path(assets::GIT_BRANCH).size(px(12.)))
                                        .child(branch)
                                        .into_any_element(),
                                )),
                        )
                    })
                    .child(div().flex_1())
                    .children(self.render_chips(o, cx)),
            )
            .into_any_element()
    }

    /// An empty pane offers to start a session in it.
    fn render_empty(&self, leaf: u64, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let button = |label: &'static str, agent: &'static str| {
            div()
                .id(SharedString::from(format!("start-{agent}-{leaf}")))
                .px(px(16.))
                .py(px(8.))
                .rounded(px(10.))
                .border_1()
                .border_color(c.border)
                .text_size(px(14.))
                .text_color(c.text)
                .cursor_pointer()
                .hover(move |s| s.bg(c.hover))
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.layout.focus = leaf;
                    let folder = app.default_folder();
                    app.new_session(agent, folder, window, cx);
                }))
                .child(label)
        };
        let folder = self.default_folder().to_string_lossy().to_string();
        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .child(div().flex().gap(px(8.)).child(button(t("Start Claude"), "claude")).child(button(t("Start Codex"), "codex")))
            .child(div().text_size(px(12.)).text_color(c.muted).child(folder))
            .into_any_element()
    }

    fn render_leaf(&self, leaf: u64, session: Option<u64>, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let split = self.layout.is_split();
        let focused = self.layout.focus == leaf;
        let open = session.and_then(|k| self.by_key(k));
        let title = if session == Some(FILE_KEY) {
            self.file_title().unwrap_or_default()
        } else {
            open.map(|o| o.title.clone().unwrap_or_else(|| t("New session").into())).unwrap_or_else(|| t("Empty").into())
        };
        self.pane_drop_handlers(div().id(SharedString::from(format!("pane-{leaf}"))), leaf, cx)
            .flex()
            .flex_col()
            .size_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_hidden()
            .bg(c.main_bg)
            .capture_any_mouse_down(cx.listener(move |app, _, window, cx| {
                if app.layout.focus != leaf {
                    let previous = app.layout.focused_session();
                    app.layout.focus = leaf;
                    app.focus_changed(previous, window, cx);
                }
            }))
            .when(split, |d| {
                d.rounded(px(10.))
                    .border_1()
                    .border_color(if focused { c.accent } else { c.border })
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(8.))
                            .h(px(28.))
                            .pl(px(10.))
                            .pr(px(4.))
                            .text_size(px(12.))
                            .text_color(if focused { c.text } else { c.muted })
                            .when(focused, |d| d.bg(c.accent.opacity(0.12)))
                            .when_some(open, |d, o| d.child(self.open_session_icon(o)))
                            .child(div().flex_1().min_w(px(0.)).truncate().child(title))
                            .when_some(open.filter(|o| !o.exited), |d, o| {
                                d.child(if o.busy {
                                    Spinner::new().color(c.accent).xsmall().into_any_element()
                                } else {
                                    div().flex_none().size(px(7.)).rounded_full().bg(c.running).into_any_element()
                                })
                            })
                            .when(focused, |d| {
                                d.child(div().flex_none().px(px(8.)).rounded(px(9.)).bg(c.accent).text_color(gpui::white()).text_size(px(11.)).child(t("Active")))
                            })
                            .child(
                                self.icon_button(SharedString::from(format!("pane-close-{leaf}")), IconName::Close)
                                    .p(px(3.))
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        // The session keeps running; it can be reopened from the sidebar.
                                        if session == Some(FILE_KEY) {
                                            app.close_file_pane(cx);
                                            return;
                                        }
                                        let previous = app.layout.focused_session();
                                        app.layout.close(leaf);
                                        app.focus_changed(previous, window, cx);
                                    })),
                            ),
                    )
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .when(split && !focused, |d| d.opacity(0.62))
                    .map(|d| match open {
                        Some(o) => d.child(o.view.clone()),
                        None if session == Some(FILE_KEY) => d.child(self.render_file_view(cx)),
                        None => d.child(self.render_empty(leaf, cx)),
                    }),
            )
            .into_any_element()
    }

    fn render_node(&self, node: &Node, cx: &mut Context<Self>) -> AnyElement {
        match node {
            Node::Leaf { id, session } => self.render_leaf(*id, *session, cx),
            Node::Split { id, dir, ratio, a, b } => {
                let (id, dir) = (*id, *dir);
                let bounds = self.split_bounds.clone();
                let divider = div()
                    .id(SharedString::from(format!("divider-{id}")))
                    .flex_none()
                    .map(|d| match dir {
                        Dir::Row => d.w(px(6.)).h_full().cursor_col_resize(),
                        Dir::Column => d.h(px(6.)).w_full().cursor_row_resize(),
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(move |app, _, _, cx| {
                        app.drag = Some(Drag::Split { id, dir });
                        cx.stop_propagation();
                    }));
                let first = div().flex_none().min_w(px(0.)).min_h(px(0.)).child(self.render_node(a, cx));
                let first = match dir {
                    Dir::Row => first.w(relative(*ratio)).h_full(),
                    Dir::Column => first.h(relative(*ratio)).w_full(),
                };
                div()
                    .relative()
                    .flex()
                    .when(dir == Dir::Column, |d| d.flex_col())
                    .size_full()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .child(
                        canvas(move |b, _, _| { bounds.borrow_mut().insert(id, b); }, |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(first)
                    .child(divider)
                    .child(div().flex_1().min_w(px(0.)).min_h(px(0.)).child(self.render_node(b, cx)))
                    .into_any_element()
            }
        }
    }
}

impl Render for CrewApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_key_file(window, cx);
        self.run_deferred(window, cx);
        if self.drag.is_none() {
            self.save_layout_if_changed();
        }
        // GPUI's surface lies above child windows, so a web page only shows through a transparent hole.
        if self.see_through != self.browser.is_some() {
            self.see_through = self.browser.is_some();
            window.set_background_appearance(if self.see_through { gpui::WindowBackgroundAppearance::Transparent } else { gpui::WindowBackgroundAppearance::Opaque });
        }
        if window.focused(cx).is_none() {
            window.focus(&self.root_focus, cx);
        }
        // Closing a picker or a panel frees its input only after this render. Left alone the focus
        // would point at nothing and no key would reach the window until something else redrew it.
        cx.on_next_frame(window, |app, window, cx| {
            app.browser_sync(cx, window.scale_factor());
            if window.focused(cx).is_none() {
                match app.focus_return.take().and_then(|h| h.upgrade()) {
                    Some(handle) => window.focus(&handle, cx),
                    None if app.active().is_some() => app.composer.update(cx, |c, cx| c.focus(window, cx)),
                    None => window.focus(&app.root_focus, cx),
                }
            }
        });
        let c = self.colors;
        let split = self.layout.is_split();
        let has_composer = self.active().is_some();
        let show_usage = self.config.bool("showUsage");
        let body = div()
            .flex()
            .flex_1()
            .min_h(px(0.))
            .when(!self.sidebar_hidden, |d| {
                d.child(self.render_sidebar(cx)).child(div().flex_none().w(px(1.)).h_full().bg(c.border))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .child(self.render_header(window, cx))
                    .when_some(self.notice.clone(), |d, notice| {
                        d.child(
                            div()
                                .flex_none()
                                .px(px(16.))
                                .py(px(8.))
                                .text_size(px(13.))
                                .text_color(c.text)
                                .bg(c.side_bg)
                                .on_mouse_down(MouseButton::Left, cx.listener(|app, _, _, cx| {
                                    app.notice = None;
                                    cx.notify();
                                }))
                                .child(notice),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h_full()
                                    .bg(c.main_bg)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h(px(0.))
                                            .when(split, |d| d.p(px(6.)).bg(c.side_bg))
                                            .child(self.render_node(&self.layout.root, cx)),
                                    )
                                    .when(has_composer, |d| d.child(self.render_composer(cx))),
                            )
                            .children(self.render_files_panel(cx))
                            .children(self.render_git_panel(window, cx))
                            .children(self.render_browser(window, cx)),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .size_full()
            .font(gpui::Font {
                family: UI_FONT.into(),
                features: Default::default(),
                // As in the original's font list, for Chinese.
                fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["Microsoft YaHei UI".to_string()])),
                weight: Default::default(),
                style: Default::default(),
            })
            .when(self.browser.is_none(), |d| d.bg(c.main_bg))
            .text_color(c.text)
            .track_focus(&self.root_focus)
            .capture_key_down(cx.listener(Self::window_key))
            .capture_any_mouse_down(cx.listener(|app, _, _, _| app.browser_release_keyboard()))
            .relative()
            .child(body)
            .when(!self.sidebar_hidden, |d| d.child(self.resize_grip("sidebar-resizer", Drag::Sidebar, self.sidebar_width - 5., cx)))
            .when(show_usage, |d| d.child(self.render_status_bar(cx)))
            .children(self.render_run_cards(cx))
            .children(self.render_approvals(cx))
            .children(self.render_agent_editor(cx))
            .children(self.render_run_result(cx))
            .children(self.render_missing_agent(cx))
            .children(self.render_image_view(cx))
            .children(self.render_image_hover(window))
            .children(self.render_picker(window, cx))
            .children(self.render_form(cx))
            .children(self.render_save_changes(cx))
            .children(self.render_usage_page(cx))
            .children(self.render_settings(cx))
            .children(self.render_host_editor(cx))
            .children(self.render_provider_editor(cx))
            .children(self.render_price_editor(cx))
            .children(self.render_dialog(cx))
            .children(self.render_prompt(cx))
            .children(self.render_toast())
            .children(self.render_menu(cx))
            .when(self.drag.is_some(), |d| d.child(self.render_drag_layer(cx)))
    }
}
