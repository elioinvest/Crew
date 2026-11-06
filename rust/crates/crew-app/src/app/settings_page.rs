//! Settings: the sections on the left, their controls on the right, saved together. Providers are
//! edited in their own dialog and saved straight away, as in the Electron app.

use base64::Engine;
use crew_core::env as crew_env;
use crew_core::providers::{self as provider_store, ProviderInput, ProviderModel};
use crew_core::secrets;

use super::overlays::{ToastKind, entry};
use super::*;
use crate::i18n::{self, zh};

/// ego lite's README, "Let your agent set it up": paste this into your agent.
const EGO_SETUP_PROMPT: &str = "Set up ego lite for me: https://github.com/citrolabs/ego-lite\n\nRead `skills/ego-browser/references/install.md` and follow the steps to install ego lite.";
const MAX_ICON: u64 = 512 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    General,
    Appearance,
    Agents,
    Providers,
    Phone,
    Git,
    Browser,
}

const SECTIONS: [Section; 7] = [Section::General, Section::Appearance, Section::Agents, Section::Providers, Section::Phone, Section::Git, Section::Browser];

impl Section {
    fn title(self) -> &'static str {
        t(match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Agents => "Agents",
            Section::Providers => "Providers",
            Section::Phone => "Phone access",
            Section::Git => "Git",
            Section::Browser => "Browser",
        })
    }

    fn icon(self) -> Icon {
        match self {
            Section::General => Icon::new(IconName::Settings2),
            Section::Appearance => Icon::new(IconName::Palette),
            Section::Agents => Icon::new(IconName::Bot),
            Section::Providers => Icon::empty().path(assets::PLUG),
            Section::Phone => Icon::empty().path(assets::PHONE),
            Section::Git => Icon::empty().path(assets::GIT_BRANCH),
            Section::Browser => Icon::new(IconName::Globe),
        }
    }

    /// The text the search box looks through.
    fn searchable(self) -> &'static [&'static str] {
        match self {
            Section::General => &["Window behavior and how Crew runs in the background.", "Keep running in the tray when the window is closed", "Sessions keep running after you close the window.", "Plan usage", "Show plan usage at the bottom. Claude sessions Crew starts get a status line hook that records it."],
            Section::Appearance => &["Theme, text size and the interface language.", "Theme", "Light or dark interface.", "Font size", "Terminal and editor text size.", "Language", "Interface language. Automatic follows Windows."],
            Section::Agents => &["Commands and arguments for the Claude Code and Codex CLIs.", "Claude command", "Claude arguments", "Codex command", "Codex arguments"],
            Section::Providers => &["Your own APIs: relays, self-hosted services and other OpenAI- or Anthropic-compatible endpoints. Each one appears in the New session menus.", "Model providers"],
            Section::Phone => &["Use Crew from the Crew app on your phone: see the open sessions, type into them, start sessions and answer approvals. On the same Wi-Fi, or from anywhere through Tailscale.", "Let paired phones connect"],
            Section::Git => &["Worktree directory", "Worktree setup command"],
            Section::Browser => &["ego lite is the browser agents use for anything that needs a browser.", "ego lite: the browser for agents"],
        }
    }

    fn matches(self, query: &str) -> bool {
        query.is_empty()
            || self.title().to_lowercase().contains(query)
            || self.searchable().iter().copied().any(|s| s.to_lowercase().contains(query) || t(s).to_lowercase().contains(query))
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct EgoStatus {
    installed: bool,
    skill_claude: bool,
    skill_codex: bool,
}

pub(super) struct SettingsPage {
    section: Section,
    search: Entity<InputState>,
    claude_command: Entity<InputState>,
    claude_args: Entity<InputState>,
    codex_command: Entity<InputState>,
    codex_args: Entity<InputState>,
    worktree_dir: Entity<InputState>,
    worktree_setup: Entity<InputState>,
    font_size: Entity<InputState>,
    theme: String,
    language: String,
    close_to_tray: bool,
    show_usage: bool,
    ego: Option<EgoStatus>,
}

struct ModelRow {
    id: Entity<InputState>,
    name: Entity<InputState>,
}

pub(super) struct ProviderEditor {
    original_id: String,
    is_new: bool,
    title: String,
    id: Entity<InputState>,
    name: Entity<InputState>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    emoji: Entity<InputState>,
    image: Option<String>,
    protocol: String,
    models: Vec<ModelRow>,
    /// Which row is the default for new sessions.
    default_row: usize,
    /// Whether the ID was typed: until then it follows the display name.
    id_typed: bool,
    status: Option<String>,
    fetching: bool,
    _subscriptions: Vec<gpui::Subscription>,
}

fn ego_status(paths: &Paths) -> EgoStatus {
    let skill = |p: PathBuf| p.join("skills").join("ego-browser").join("SKILL.md").exists();
    EgoStatus {
        installed: crew_env::command_exists("ego-browser"),
        skill_claude: skill(paths.home.join(".claude")),
        skill_codex: skill(paths.home.join(".agents")) || skill(paths.codex_home.clone()),
    }
}

impl CrewApp {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = |config: &Config, key: &str| config.str(key).to_string();
        let plain = |value: String, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(t(placeholder)).default_value(value))
        };
        self.refresh_phone(cx);
        let config = self.config.clone();
        let search = plain(String::new(), "Search", window, cx);
        search.update(cx, |s, cx| s.focus(window, cx));
        self._subscriptions.push(cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        }));
        self.settings = Some(SettingsPage {
            section: Section::General,
            search,
            claude_command: plain(text(&config, "claudeCommand"), "", window, cx),
            claude_args: plain(text(&config, "extraArgs"), "e.g. --model opus", window, cx),
            codex_command: plain(text(&config, "codexCommand"), "", window, cx),
            codex_args: plain(text(&config, "codexArgs"), "e.g. -m gpt-5.5", window, cx),
            worktree_dir: plain(text(&config, "worktreeDirectory"), "../worktrees", window, cx),
            worktree_setup: plain(text(&config, "worktreeSetup"), "e.g. copy %CREW_MAIN_GIT_WORKTREE%\\.env .", window, cx),
            font_size: plain((config.f32("fontSize", 14.) as i64).to_string(), "", window, cx),
            theme: if config.str("theme") == "dark" { "dark".into() } else { "light".into() },
            language: if config.str("language").is_empty() { "auto".into() } else { config.str("language").to_string() },
            close_to_tray: config.bool("closeToTray"),
            show_usage: config.bool("showUsage"),
            ego: None,
        });
        self.refresh_ego(cx);
        cx.notify();
    }

    fn refresh_ego(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let check = cx.background_spawn(async move { ego_status(&paths) });
        cx.spawn(async move |this, cx| {
            let status = check.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(s) = app.settings.as_mut() {
                    s.ego = Some(status);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Once, at startup: if ego lite isn't installed, offer to set it up. Nothing runs without a click.
    pub(super) fn offer_ego_setup(&mut self, cx: &mut Context<Self>) {
        if self.config.bool("egoSetupOffered") {
            return;
        }
        let paths = self.paths.clone();
        let check = cx.background_spawn(async move { ego_status(&paths) });
        cx.spawn(async move |this, cx| {
            let status = check.await;
            let _ = this.update(cx, |app, cx| {
                if status.installed {
                    return;
                }
                app.save_setting("egoSetupOffered", json!(true));
                app.offer(
                    t("Set up ego lite?"),
                    format!(
                        "{}\n\n{}",
                        t("Agents use ego lite, a free browser built for agents, for anything that needs a browser. It isn't installed yet."),
                        t("Set up opens an agent session with ego's official setup prompt. The agent guides you through installing ego lite and its skill. You can also do this later in Settings."),
                    ),
                    t("Set up"),
                    |app, window, cx| app.set_up_ego(window, cx),
                    cx,
                );
            });
        })
        .detach();
    }

    /// ego's README sets ego lite up by giving an agent its setup prompt: a new session in the home
    /// folder with that prompt, on Claude Code, or Codex when only Codex is installed.
    fn set_up_ego(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let claude = self.config.str("claudeCommand").to_string();
        let codex = self.config.str("codexCommand").to_string();
        let agent = if crew_env::command_exists(&claude) || !crew_env::command_exists(&codex) { "claude" } else { "codex" };
        let home = self.paths.home.clone();
        self.start_new_with(agent, home, None, Some(EGO_SETUP_PROMPT), window, cx);
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.settings else { return };
        let value = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let or = |v: String, default: &str| if v.is_empty() { default.to_string() } else { v };
        let font: i64 = value(&s.font_size).parse::<i64>().unwrap_or(14).clamp(9, 28);
        let changes = [
            ("claudeCommand", json!(or(value(&s.claude_command), "claude"))),
            ("extraArgs", json!(value(&s.claude_args))),
            ("codexCommand", json!(or(value(&s.codex_command), "codex"))),
            ("codexArgs", json!(value(&s.codex_args))),
            ("worktreeDirectory", json!(value(&s.worktree_dir))),
            ("worktreeSetup", json!(value(&s.worktree_setup))),
            ("theme", json!(s.theme)),
            ("fontSize", json!(font)),
            ("closeToTray", json!(s.close_to_tray)),
            ("showUsage", json!(s.show_usage)),
            ("language", json!(s.language)),
        ];
        let language = s.language.clone();
        for (key, value) in changes {
            self.save_setting(key, value);
        }
        i18n::init(&language);
        self.apply_appearance(window, cx);
        self.settings = None;
        cx.notify();
    }

    fn toggle(&self, id: &'static str, on: bool, flip: fn(&mut SettingsPage), cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .w(px(36.))
            .h(px(20.))
            .p(px(2.))
            .rounded_full()
            .cursor_pointer()
            .bg(if on { c.accent } else { c.border })
            .when(on, |d| d.justify_end())
            .on_click(cx.listener(move |app, _, _, cx| {
                if let Some(s) = app.settings.as_mut() {
                    flip(s);
                }
                cx.notify();
            }))
            .child(div().size(px(16.)).rounded_full().bg(gpui::white()))
            .into_any_element()
    }

    pub(super) fn settings_row(&self, title: &'static str, description: &'static str, control: impl IntoElement) -> gpui::Div {
        let c = self.colors;
        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(16.))
            .py(px(12.))
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(div().text_size(px(13.5)).text_color(c.text).child(t(title)))
                    .when(!description.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(c.muted).child(t(description)))),
            )
            .child(control)
    }

    pub(super) fn settings_card(&self) -> gpui::Div {
        let c = self.colors;
        div().flex().flex_col().rounded(px(10.)).border_1().border_color(c.border).bg(c.card).overflow_hidden()
    }

    fn panel_head(&self, title: &'static str, description: &'static str) -> gpui::Div {
        let c = self.colors;
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().text_size(px(20.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c.text).child(t(title)))
            .when(!description.is_empty(), |d| d.child(div().text_size(px(13.)).text_color(c.muted).child(t(description))))
    }

    fn render_settings_panel(&self, s: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let wide = |input: &Entity<InputState>| div().w(px(320.)).flex_none().child(Input::new(input).small());
        let body = div().flex().flex_col().gap(px(14.));
        match s.section {
            Section::General => body
                .child(self.panel_head("General", "Window behavior and how Crew runs in the background."))
                .child(
                    self.settings_card()
                        .child(self.settings_row("Keep running in the tray when the window is closed", "Sessions keep running after you close the window.", self.toggle("cfg-tray", s.close_to_tray, |s| s.close_to_tray = !s.close_to_tray, cx)))
                        .child(self.settings_row("Plan usage", "Show plan usage at the bottom. Claude sessions Crew starts get a status line hook that records it.", self.toggle("cfg-usage", s.show_usage, |s| s.show_usage = !s.show_usage, cx))),
                ),
            Section::Appearance => {
                let themes = vec![
                    entry(t("Light"), |app, _, _| { if let Some(s) = app.settings.as_mut() { s.theme = "light".into(); } }),
                    entry(t("Dark"), |app, _, _| { if let Some(s) = app.settings.as_mut() { s.theme = "dark".into(); } }),
                ];
                let languages = vec![
                    entry(t("Automatic"), |app, _, _| { if let Some(s) = app.settings.as_mut() { s.language = "auto".into(); } }),
                    entry("English", |app, _, _| { if let Some(s) = app.settings.as_mut() { s.language = "en".into(); } }),
                    entry("中文", |app, _, _| { if let Some(s) = app.settings.as_mut() { s.language = "zh".into(); } }),
                ];
                let theme = if s.theme == "dark" { t("Dark") } else { t("Light") };
                let language = match s.language.as_str() {
                    "en" => "English",
                    "zh" => "中文",
                    _ => t("Automatic"),
                };
                body.child(self.panel_head("Appearance", "Theme, text size and the interface language.")).child(
                    self.settings_card()
                        .child(self.settings_row("Theme", "Light or dark interface.", div().w(px(180.)).flex_none().child(self.select("cfg-theme", theme, themes, cx))))
                        .child(self.settings_row("Font size", "Terminal and editor text size.", div().w(px(80.)).flex_none().child(Input::new(&s.font_size).small())))
                        .child(self.settings_row("Language", "Interface language. Automatic follows Windows.", div().w(px(180.)).flex_none().child(self.select("cfg-language", language, languages, cx)))),
                )
            }
            Section::Agents => body
                .child(self.panel_head("Agents", "Commands and arguments for the Claude Code and Codex CLIs."))
                .child(
                    self.settings_card()
                        .child(self.settings_row("Claude command", "The Claude Code executable Crew starts.", wide(&s.claude_command)))
                        .child(self.settings_row("Claude arguments", "Extra arguments passed on every Claude session.", wide(&s.claude_args))),
                )
                .child(
                    self.settings_card()
                        .child(self.settings_row("Codex command", "The Codex executable Crew starts.", wide(&s.codex_command)))
                        .child(self.settings_row("Codex arguments", "Extra arguments passed on every Codex session.", wide(&s.codex_args))),
                ),
            Section::Git => body.child(self.panel_head("Git", "")).child(
                self.settings_card()
                    .child(self.settings_row("Worktree directory", "Where new worktrees go, relative to the repository. Outside the repository, the project's name is added.", wide(&s.worktree_dir)))
                    .child(self.settings_row("Worktree setup command", "Runs in each new worktree Crew creates, with CREW_WORKTREE_ROOT and CREW_MAIN_GIT_WORKTREE set. For example, copy .env or install dependencies.", wide(&s.worktree_setup))),
            ),
            Section::Providers => {
                let rows: Vec<AnyElement> = self
                    .providers
                    .iter()
                    .map(|p| {
                        let count = p.models.len();
                        let meta = format!(
                            "{} · {}",
                            if p.protocol == "anthropic" { "Anthropic Messages" } else { "OpenAI Responses" },
                            match (count, zh()) {
                                (0, true) => "没有模型".to_string(),
                                (0, false) => "no models".to_string(),
                                (n, true) => format!("{n} 个模型"),
                                (1, false) => "1 model".to_string(),
                                (n, false) => format!("{n} models"),
                            }
                        );
                        let (edit, delete) = (p.clone(), p.clone());
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(16.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(c.border)
                            .child(self.provider_icon(p))
                            .child(div().text_size(px(13.5)).child(p.name.clone()))
                            .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(12.)).text_color(c.muted).child(meta))
                            .child(self.button(SharedString::from(format!("provider-edit-{}", p.id)), t("Edit"), false).on_click(cx.listener(move |app, _, window, cx| app.edit_provider(Some(edit.clone()), window, cx))))
                            .child(self.button(SharedString::from(format!("provider-delete-{}", p.id)), t("Delete"), false).text_color(gpui::rgb(0xd93d42)).on_click(cx.listener(move |app, _, _, cx| app.ask_delete_provider(&delete, cx))))
                            .into_any_element()
                    })
                    .collect();
                body.child(self.panel_head("Providers", "Your own APIs: relays, self-hosted services and other OpenAI- or Anthropic-compatible endpoints. Each one appears in the New session menus.")).child(
                    self.settings_card()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px(px(16.))
                                .py(px(10.))
                                .child(div().text_size(px(13.5)).child(t("Model providers")))
                                .child(self.button("provider-add", t("+ Add provider"), false).on_click(cx.listener(|app, _, window, cx| app.edit_provider(None, window, cx)))),
                        )
                        .map(|d| {
                            if rows.is_empty() {
                                d.child(div().px(px(16.)).pb(px(12.)).text_size(px(12.)).text_color(c.muted).child(t("None yet. Add a relay, a self-hosted service or another API compatible with OpenAI or Anthropic.")))
                            } else {
                                d.children(rows)
                            }
                        }),
                )
            }
            Section::Phone => body
                .child(self.panel_head("Phone access", "Use Crew from the Crew app on your phone: see the open sessions, type into them, start sessions and answer approvals. On the same Wi-Fi, or from anywhere through Tailscale."))
                .child(self.render_phone_card(cx)),
            Section::Browser => {
                let danger: gpui::Hsla = gpui::rgb(0xd93d42).into();
                let line = |ok: bool, text: String| div().text_size(px(12.5)).text_color(if ok { c.text } else { danger }).child(text);
                let (app_line, skill_line, needs_setup) = match s.ego {
                    None => (None, None, false),
                    Some(e) => {
                        let both = e.skill_claude && e.skill_codex;
                        let app_text = if e.installed { t("✓ ego lite is installed") } else { t("✗ ego lite isn't installed") };
                        let skill_text = if both {
                            t("✓ Official skill installed for Claude Code and Codex").to_string()
                        } else if e.skill_claude {
                            t("✓ Official skill installed for Claude Code only").to_string()
                        } else if e.skill_codex {
                            t("✓ Official skill installed for Codex only").to_string()
                        } else {
                            t("✗ Official skill not installed").to_string()
                        };
                        (Some(line(e.installed, app_text.to_string())), Some(line(e.skill_claude || e.skill_codex, skill_text)), !(e.installed && both))
                    }
                };
                body.child(self.panel_head("Browser", "ego lite is the browser agents use for anything that needs a browser.")).child(
                    self.settings_card().child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .px(px(16.))
                            .py(px(12.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .gap(px(4.))
                                    .child(div().text_size(px(13.5)).child(t("ego lite: the browser for agents")))
                                    .children(app_line)
                                    .children(skill_line)
                                    .child(div().text_size(px(12.)).text_color(c.muted).child(t("With ego's official skill, agents use ego lite for anything that needs a browser, each in its own Space. When a step needs you (a sign-in, a code), the agent pauses and tells you which Space to open."))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .gap(px(6.))
                                    .when(needs_setup, |d| {
                                        d.child(self.button("ego-setup", t("Set up ego lite"), false).on_click(cx.listener(|app, _, window, cx| {
                                            app.settings = None;
                                            app.set_up_ego(window, cx);
                                        })))
                                    })
                                    .child(self.button("ego-check", t("Check again"), false).on_click(cx.listener(|app, _, _, cx| app.refresh_ego(cx)))),
                            ),
                    ),
                )
            }
        }
        .into_any_element()
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let s = self.settings.as_ref()?;
        let query = s.search.read(cx).value().trim().to_lowercase();
        let mut nav: Vec<AnyElement> = Vec::new();
        let mut any = false;
        for section in SECTIONS {
            if !section.matches(&query) {
                continue;
            }
            any = true;
            if section == Section::Agents {
                nav.push(div().pt(px(12.)).pb(px(4.)).px(px(10.)).text_size(px(12.)).text_color(c.muted).child(t("Agents")).into_any_element());
            }
            let on = section == s.section;
            nav.push(
                div()
                    .id(SharedString::from(format!("settings-nav-{}", section.title())))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(13.5))
                    .text_color(if on { c.text } else { c.muted })
                    .when(on, |d| d.bg(c.card).border_1().border_color(c.card_edge))
                    .when(!on, |d| d.hover(move |h| h.bg(c.hover).text_color(c.text)))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if let Some(s) = app.settings.as_mut() {
                            s.section = section;
                        }
                        if section == Section::Phone {
                            app.refresh_phone(cx);
                        }
                        cx.notify();
                    }))
                    .child(section.icon().size(px(16.)))
                    .child(section.title())
                    .into_any_element(),
            );
        }
        let content = self.render_settings_panel(s, cx);
        Some(
            div()
                .id("settings")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.35))
                .occlude()
                .child(
                    div()
                        .flex()
                        .w(relative(0.86))
                        .max_w(px(1040.))
                        .h(relative(0.86))
                        .max_h(px(780.))
                        .rounded(px(12.))
                        .overflow_hidden()
                        .bg(c.main_bg)
                        .border_1()
                        .border_color(c.border)
                        .shadow_md()
                        .child(
                            div()
                                .id("settings-nav")
                                .flex()
                                .flex_col()
                                .flex_none()
                                .w(px(230.))
                                .h_full()
                                .gap(px(2.))
                                .p(px(10.))
                                .bg(c.side_bg)
                                .border_r_1()
                                .border_color(c.border)
                                .overflow_y_scroll()
                                .child(div().pb(px(8.)).child(Input::new(&s.search).small().prefix(div().text_color(c.muted).child(Icon::new(IconName::Search).size(px(15.))))))
                                .children(nav)
                                .when(!any, |d| d.child(div().px(px(10.)).py(px(8.)).text_size(px(12.5)).text_color(c.muted).child(t("No matches")))),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_between()
                                        .px(px(20.))
                                        .py(px(10.))
                                        .border_b_1()
                                        .border_color(c.border)
                                        .text_size(px(13.))
                                        .text_color(c.muted)
                                        .child(div().flex().gap(px(6.)).child(t("Settings")).child("/").child(div().text_color(c.text).font_weight(gpui::FontWeight::SEMIBOLD).child(s.section.title())))
                                        .child(self.icon_button("settings-close", IconName::Close).on_click(cx.listener(|app, _, _, cx| {
                                            app.settings = None;
                                            cx.notify();
                                        }))),
                                )
                                .child(div().id("settings-content").flex_1().min_h(px(0.)).overflow_y_scroll().p(px(20.)).child(content))
                                .child(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .gap(px(8.))
                                        .px(px(20.))
                                        .py(px(10.))
                                        .border_t_1()
                                        .border_color(c.border)
                                        .child(div().flex_1().min_w(px(0.)).text_size(px(12.)).text_color(c.muted).child(t("Command and arguments apply to sessions started after saving.")))
                                        .child(self.button("cfg-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                                            app.settings = None;
                                            cx.notify();
                                        })))
                                        .child(self.button("cfg-save", t("Save"), true).on_click(cx.listener(|app, _, window, cx| app.save_settings(window, cx)))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    // ---------- providers ----------

    fn reload_providers(&mut self, cx: &mut Context<Self>) {
        self.providers = providers::list(&self.paths);
        self.rescan(cx);
        cx.notify();
    }

    fn ask_delete_provider(&mut self, p: &Provider, cx: &mut Context<Self>) {
        let id = p.id.clone();
        let title = if zh() { format!("删除 {}？", p.name) } else { format!("Delete {}?", p.name) };
        self.confirm(
            title,
            t("Sessions started on it are kept, but resume on the agent's own account."),
            t("Delete"),
            move |app, _, cx| {
                if let Err(e) = provider_store::delete(&app.paths, &id) {
                    app.toast(e, ToastKind::Error, cx);
                }
                app.reload_providers(cx);
            },
            cx,
        );
    }

    fn model_row(&self, id: String, name: String, window: &mut Window, cx: &mut Context<Self>) -> ModelRow {
        ModelRow {
            id: cx.new(|cx| InputState::new(window, cx).placeholder(t("Model ID")).default_value(id)),
            name: cx.new(|cx| InputState::new(window, cx).placeholder(t("Display name")).default_value(name)),
        }
    }

    pub(super) fn edit_provider(&mut self, provider: Option<Provider>, window: &mut Window, cx: &mut Context<Self>) {
        let is_new = provider.is_none();
        let p = provider.as_ref();
        let text = |f: fn(&Provider) -> &String| p.map_or(String::new(), |p| f(p).clone());
        let (id_text, name_text, url_text) = (text(|p| &p.id), text(|p| &p.name), text(|p| &p.base_url));
        let has_key = p.is_some_and(|p| p.has_key());
        let id = cx.new(|cx| InputState::new(window, cx).placeholder("acme-gateway").default_value(id_text.clone()));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("Display name")).default_value(name_text));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://gateway.example/v1").default_value(url_text));
        let key = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(if has_key { t("Saved. Leave empty to keep it") } else { t("Enter the API key") }));
        let emoji_text = p.and_then(|p| p.emoji.clone()).unwrap_or_default();
        let emoji = cx.new(|cx| InputState::new(window, cx).placeholder(t("Emoji")).default_value(emoji_text));
        let models: Vec<ModelRow> = p.map_or(Vec::new(), |p| p.models.iter().map(|m| self.model_row(m.id.clone(), m.name.clone(), window, cx)).collect());
        let default_row = p.and_then(|p| p.models.iter().position(|m| m.id == p.default_model)).unwrap_or(0);
        let subscriptions = vec![
            // The ID is made valid as it's typed, and follows the display name until it's typed in itself.
            cx.subscribe_in(&id, window, |app, state, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let value = state.read(cx).value().to_string();
                let valid = provider_store::to_id(&value);
                if let Some(e) = app.provider_editor.as_mut() {
                    e.id_typed = !valid.is_empty();
                }
                if valid != value {
                    state.update(cx, |s, cx| s.set_value(valid, window, cx));
                }
            }),
            cx.subscribe_in(&name, window, |app, state, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let Some(e) = app.provider_editor.as_ref().filter(|e| !e.id_typed) else { return };
                let id = e.id.clone();
                let value = provider_store::to_id(&state.read(cx).value()).trim_end_matches('-').to_string();
                id.update(cx, |s, cx| s.set_value(value, window, cx));
                if let Some(e) = app.provider_editor.as_mut() {
                    e.id_typed = false;
                }
            }),
        ];
        let focus = if is_new { id.clone() } else { name.clone() };
        focus.update(cx, |s, cx| s.focus(window, cx));
        self.provider_editor = Some(ProviderEditor {
            original_id: id_text.clone(),
            is_new,
            title: match (p, zh()) {
                (None, _) => t("Add a model provider").to_string(),
                (Some(p), true) => format!("编辑 {}", p.name),
                (Some(p), false) => format!("Edit {}", p.name),
            },
            id,
            name,
            url,
            key,
            emoji,
            image: p.and_then(|p| p.image.clone()),
            protocol: p.map_or("responses".to_string(), |p| p.protocol.clone()),
            models,
            default_row,
            id_typed: !id_text.is_empty(),
            status: None,
            fetching: false,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn pick_provider_icon(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(file) = paths.into_iter().next() else { return };
            let result: Result<String, &'static str> = (|| {
                if std::fs::metadata(&file).map_or(true, |m| m.len() > MAX_ICON) {
                    return Err("Choose an image under 512 KB.");
                }
                let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let kind = match ext.as_str() {
                    "svg" => "image/svg+xml".to_string(),
                    "jpg" => "image/jpeg".to_string(),
                    "ico" => "image/x-icon".to_string(),
                    other => format!("image/{other}"),
                };
                let bytes = std::fs::read(&file).map_err(|_| "Choose an image under 512 KB.")?;
                Ok(format!("data:{kind};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
            })();
            let _ = this.update(cx, |app, cx| {
                if let Some(e) = app.provider_editor.as_mut() {
                    match result {
                        Ok(image) => e.image = Some(image),
                        Err(why) => e.status = Some(t(why).to_string()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_provider_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.provider_editor.as_mut() else { return };
        e.fetching = true;
        e.status = Some(t("Fetching the models…").to_string());
        let saved = if e.is_new { String::new() } else { e.original_id.clone() };
        let (url, protocol, key) = (e.url.read(cx).value().to_string(), e.protocol.clone(), e.key.read(cx).value().to_string());
        let paths = self.paths.clone();
        let fetch = cx.background_spawn(async move { provider_store::fetch_models(&paths, &saved, &url, &protocol, &key) });
        cx.spawn_in(window, async move |this, cx| {
            let result = fetch.await;
            let _ = this.update_in(cx, |app, window, cx| {
                match result {
                    Err(why) => {
                        if let Some(e) = app.provider_editor.as_mut() {
                            e.status = Some(if zh() { format!("无法获取模型：{why}") } else { format!("Couldn't fetch the models: {why}") });
                        }
                    }
                    Ok(found) => {
                        let have: Vec<String> = app.provider_editor.as_ref().map_or(Vec::new(), |e| e.models.iter().map(|r| r.id.read(cx).value().trim().to_string()).collect());
                        let fresh: Vec<String> = found.iter().filter(|m| !have.contains(m)).cloned().collect();
                        let rows: Vec<ModelRow> = fresh.iter().map(|m| app.model_row(m.clone(), provider_store::pretty_name(m), window, cx)).collect();
                        let (total, added) = (found.len(), fresh.len());
                        if let Some(e) = app.provider_editor.as_mut() {
                            // Rows left completely empty go.
                            e.models.retain(|r| !(r.id.read(cx).value().trim().is_empty() && r.name.read(cx).value().trim().is_empty()));
                            e.models.extend(rows);
                            e.status = Some(match (total, zh()) {
                                (0, _) => t("The provider listed no models. Add them by hand.").to_string(),
                                (n, true) => format!("找到 {n} 个模型（{added} 个新的）。"),
                                (n, false) => format!("Found {n} models ({added} new)."),
                            });
                        }
                    }
                }
                if let Some(e) = app.provider_editor.as_mut() {
                    e.fetching = false;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_provider_editor(&mut self, cx: &mut Context<Self>) {
        let Some(e) = &self.provider_editor else { return };
        let value = |s: &Entity<InputState>| s.read(cx).value().trim().to_string();
        let rows: Vec<(String, String)> = e.models.iter().map(|r| (value(&r.id), value(&r.name))).collect();
        let default_id = rows.get(e.default_row).map_or(String::new(), |r| r.0.clone());
        // Rows left completely empty are dropped; a named row needs its ID.
        let kept: Vec<&(String, String)> = rows.iter().filter(|r| !r.0.is_empty() || !r.1.is_empty()).collect();
        let error = if let Some(i) = kept.iter().position(|r| r.0.is_empty()) {
            Some(if zh() { format!("模型 {}：模型 ID 不能为空。", i + 1) } else { format!("Model {}: the model ID can't be empty.", i + 1) })
        } else {
            kept.iter().enumerate().find(|(i, r)| kept[..*i].iter().any(|o| o.0 == r.0)).map(|(_, r)| if zh() { format!("{} 重复了。", r.0) } else { format!("{} is in the list twice.", r.0) })
        };
        if let Some(error) = error {
            if let Some(e) = self.provider_editor.as_mut() {
                e.status = Some(error);
            }
            cx.notify();
            return;
        }
        let id_source = if value(&e.id).is_empty() { value(&e.name) } else { value(&e.id) };
        let input = ProviderInput {
            original_id: if e.is_new { String::new() } else { e.original_id.clone() },
            id: provider_store::to_id(&id_source).trim_end_matches('-').to_string(),
            name: value(&e.name),
            base_url: value(&e.url),
            protocol: e.protocol.clone(),
            key: e.key.read(cx).value().to_string(),
            models: kept.iter().map(|r| ProviderModel { id: r.0.clone(), name: if r.1.is_empty() { provider_store::pretty_name(&r.0) } else { r.1.clone() } }).collect(),
            default_model: default_id,
            emoji: Some(value(&e.emoji)).filter(|v| !v.is_empty()),
            image: e.image.clone(),
        };
        match provider_store::save(&self.paths, &input) {
            Ok(_) => {
                self.provider_editor = None;
                self.reload_providers(cx);
            }
            Err(why) => {
                if let Some(e) = self.provider_editor.as_mut() {
                    e.status = Some(why);
                }
                cx.notify();
            }
        }
    }

    pub(super) fn render_provider_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let e = self.provider_editor.as_ref()?;
        let hint = |text: &'static str| div().text_size(px(12.)).text_color(c.muted).child(t(text));
        let protocol_entries = vec![
            entry("OpenAI Responses", |app, _, _| { if let Some(e) = app.provider_editor.as_mut() { e.protocol = "responses".into(); } }),
            entry("Anthropic Messages", |app, _, _| { if let Some(e) = app.provider_editor.as_mut() { e.protocol = "anthropic".into(); } }),
        ];
        let anthropic = e.protocol == "anthropic";
        let rows: Vec<AnyElement> = e
            .models
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let is_default = i == e.default_row;
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_1().min_w(px(0.)).child(Input::new(&r.id).small()))
                    .child(div().flex_1().min_w(px(0.)).child(Input::new(&r.name).small()))
                    .child(
                        self.icon_button(("model-star", i), IconName::Star)
                            .p(px(4.))
                            .when(is_default, |d| d.text_color(c.accent))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if let Some(e) = app.provider_editor.as_mut() {
                                    e.default_row = i;
                                }
                                cx.notify();
                            })),
                    )
                    .child(self.icon_button(("model-remove", i), IconName::Delete).p(px(4.)).on_click(cx.listener(move |app, _, _, cx| {
                        if let Some(e) = app.provider_editor.as_mut() {
                            e.models.remove(i);
                            if e.default_row >= e.models.len() {
                                e.default_row = 0;
                            }
                        }
                        cx.notify();
                    })))
                    .into_any_element()
            })
            .collect();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(self.heading(e.title.clone()))
            .child(hint("Connect a relay, a self-hosted service or another API compatible with OpenAI or Anthropic. Crew runs it with Codex or Claude Code, set up through their official options."))
            .child(self.field(t("Provider ID"), Input::new(&e.id).small(), Some(t("Starts with a lowercase letter; a-z, 0-9 and -. It identifies the provider in Crew.").into())))
            .child(self.field(t("Display name"), Input::new(&e.name).small(), None))
            .child(
                self.field(
                    t("Icon"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when_some(e.image.as_ref(), |d, _| d.child(div().text_size(px(12.)).text_color(c.muted).child(t("Image chosen"))))
                        .child(self.button("provider-icon-pick", t("Choose image…"), false).on_click(cx.listener(|app, _, _, cx| app.pick_provider_icon(cx))))
                        .child(div().w(px(90.)).child(Input::new(&e.emoji).small()))
                        .child(self.button("provider-icon-reset", t("Reset"), false).on_click(cx.listener(|app, _, window, cx| {
                            if let Some(e) = app.provider_editor.as_mut() {
                                e.image = None;
                                let emoji = e.emoji.clone();
                                emoji.update(cx, |s, cx| s.set_value("", window, cx));
                            }
                            cx.notify();
                        }))),
                    Some(t("An image (PNG, SVG, ICO… under 512 KB) or an emoji. Without one, the first letter of the name is shown.").into()),
                ),
            )
            .child(self.field(t("API URL"), Input::new(&e.url).small(), None))
            .child(self.field(
                t("API protocol"),
                self.select("provider-protocol", if anthropic { "Anthropic Messages" } else { "OpenAI Responses" }, protocol_entries, cx),
                Some(if anthropic { t("Runs on Claude Code. For APIs that serve /v1/messages.") } else { t("Runs on Codex. For APIs that serve /v1/responses.") }.into()),
            ))
            .child(self.field(t("API key"), Input::new(&e.key).small(), Some(if secrets::can_seal() { t("Saved encrypted with Windows data protection. Agents get it only in the sessions you start on this provider.") } else { t("This system can't encrypt it, so it isn't saved.") }.into())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).child(t("Models")))
                    .child(self.button("provider-fetch", t("Fetch available models"), false).when(e.fetching, |d| d.opacity(0.5)).on_click(cx.listener(|app, _, window, cx| {
                        if !app.provider_editor.as_ref().is_some_and(|e| e.fetching) {
                            app.fetch_provider_models(window, cx);
                        }
                    }))),
            )
            .child(hint("Each model has its ID (sent to the API) and the name shown in the session's model menu. ★ marks the default for new sessions."))
            .map(|d| if rows.is_empty() { d.child(hint("No models yet. Fetch the provider's list, or add a model and type its ID.")) } else { d.children(rows) })
            .child(div().child(self.button("provider-add-model", t("+ Add model"), false).on_click(cx.listener(|app, _, window, cx| {
                let row = app.model_row(String::new(), String::new(), window, cx);
                row.id.update(cx, |s, cx| s.focus(window, cx));
                if let Some(e) = app.provider_editor.as_mut() {
                    e.models.push(row);
                }
                cx.notify();
            }))))
            .when_some(e.status.clone(), |d, status| d.child(div().text_size(px(12.5)).text_color(c.muted).child(status)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .pt(px(4.))
                    .child(self.button("provider-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                        app.provider_editor = None;
                        cx.notify();
                    })))
                    .child(self.button("provider-save", if e.is_new { t("Create provider") } else { t("Save") }, true).on_click(cx.listener(|app, _, _, cx| app.save_provider_editor(cx)))),
            );
        Some(self.modal("provider-editor", 640., body))
    }
}
