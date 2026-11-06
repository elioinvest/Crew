//! Agents you can @-mention in a session to hand them work: the list in the sidebar and its editor,
//! and live cards for their runs. crew-service does the running.

use crew_core::env as crew_env;
use crew_service::runs::{CodexModel, PublicRun};
use gpui::MouseDownEvent;

use super::overlays::{MenuEntry, ToastKind, entry};
use super::*;
use crate::i18n::zh;

const CLAUDE_MODELS: [(&str, &str); 4] = [("fable", "Fable 5.1"), ("opus", "Opus 5.5"), ("sonnet", "Sonnet 5.5"), ("haiku", "Haiku 5.5")];
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const OTHER_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
/// Finished cards go away by themselves after this long.
const RUN_CARD_LIFE: Duration = Duration::from_secs(120);

struct Template {
    label: &'static str,
    name: &'static str,
    cli: &'static str,
    access: &'static str,
    instructions: &'static str,
}

const TEMPLATES: [Template; 4] = [
    Template { label: "Code reviewer", name: "CodeReviewerAgent", cli: "codex", access: "read", instructions: "Review the code for bugs, security problems, missing error handling and unclear code. Don't change files. Report findings by severity, each with the file, line and a suggested fix. Say clearly if you found nothing serious." },
    Template { label: "Tester", name: "TesterAgent", cli: "claude", access: "edit", instructions: "Run the project's tests and linters. Add tests for anything important that has none. Report what passed, what failed and why." },
    Template { label: "Publisher", name: "CodePublisherAgent", cli: "claude", access: "full", instructions: "Publish the project: build it, commit and push the changes, tag a release or deploy as the project does it (read its README and scripts first). Stop and report instead of guessing when credentials or a step are missing." },
    Template { label: "Blank agent", name: "", cli: "claude", access: "read", instructions: "" },
];

pub(super) struct AgentEditor {
    id: Option<String>,
    title: String,
    shown_name: String,
    name: Entity<InputState>,
    instructions: Entity<TextareaState>,
    cli: String,
    model: String,
    effort: String,
    access: String,
    /// CLI -> whether it's installed, once checked.
    installed: HashMap<String, bool>,
    /// Codex's models for this account: None while loading or when it can't say.
    codex_models: Option<Vec<CodexModel>>,
    codex_loading: bool,
    error: Option<String>,
}

pub(super) struct RunCard {
    run: PublicRun,
    finished: Option<Instant>,
}

pub(super) struct RunResult {
    run: PublicRun,
}

fn effort_label(e: &str) -> String {
    let en = match e {
        "xhigh" => "Extra high",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "max" => "Max",
        other => return other.to_string(),
    };
    t(en).to_string()
}

fn access_label(a: &str) -> &'static str {
    match a {
        "edit" => t("Edit files and run commands in the project"),
        "full" => t("Anything, without asking"),
        _ => t("Only read"),
    }
}

fn install_command(cli: &str) -> &'static str {
    if cli == "codex" { "npm install -g @openai/codex" } else { "irm https://claude.ai/install.ps1 | iex" }
}

fn elapsed_text(r: &PublicRun) -> String {
    let end = if r.ended_at > 0 { r.ended_at } else { usage::now_ms() };
    let s = ((end - r.started_at) / 1000).max(0);
    if s < 60 { format!("{s}s") } else { format!("{}m {}s", s / 60, s % 60) }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "done" => t("Done"),
        "failed" => t("Failed"),
        "stopped" => t("Stopped"),
        _ => t("Working"),
    }
}

/// Whether `text` mentions @name as a whole word, ignoring case.
fn mentions(text: &str, name: &str) -> bool {
    let lower = text.to_lowercase();
    let needle = format!("@{}", name.to_lowercase());
    lower.match_indices(&needle).any(|(at, m)| {
        !lower[at + m.len()..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// A message that @-mentions agents gets a short note for the main agent on how to run them.
pub(super) fn with_agent_note(text: &str, roster: &[RosterAgent]) -> String {
    let names: Vec<String> = roster.iter().filter(|a| mentions(text, &a.name)).map(|a| format!("@{}", a.name)).collect();
    if names.is_empty() {
        return text.to_string();
    }
    let verb = if names.len() > 1 { "are agents" } else { "is an agent" };
    format!(
        "{text}\n\n(Crew: {} {verb} you can hand work to, in this folder. Run one with crew ask <name> \"<complete task with the context it needs>\": it waits and prints the agent's answer. To run several at once, add --bg to each (it prints a run id), then crew wait <ids>. Follow my message for what runs when, one after another or together. Give these commands a 10-minute timeout; on exit status 3, run crew wait <id> again. See crew --help.)",
        names.join(", ")
    )
}

impl CrewApp {
    pub(super) fn runner_icon(&self, cli: &str) -> AnyElement {
        match cli.strip_prefix("provider:") {
            Some(id) => self.providers.iter().find(|p| p.id == id).map(|p| self.provider_icon(p)).unwrap_or_else(|| div().size(px(16.)).into_any_element()),
            None => self.agent_badge(cli, 14.),
        }
    }

    fn editor_provider(&self, cli: &str) -> Option<&Provider> {
        cli.strip_prefix("provider:").and_then(|id| self.providers.iter().find(|p| p.id == id))
    }

    fn reload_roster(&mut self, cx: &mut Context<Self>) {
        self.roster = roster::list(&self.paths);
        cx.notify();
    }

    fn add_agent_menu(&mut self, at: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let items: Vec<MenuEntry> = (0..TEMPLATES.len())
            .map(|i| {
                entry(t(TEMPLATES[i].label), move |app, window, cx| {
                    let tpl = &TEMPLATES[i];
                    let draft = RosterAgent {
                        name: tpl.name.into(),
                        cli: tpl.cli.into(),
                        access: tpl.access.into(),
                        instructions: t(tpl.instructions).into(),
                        ..RosterAgent::default()
                    };
                    app.edit_agent(draft, true, window, cx);
                })
            })
            .collect();
        self.open_menu(at, items, cx);
    }

    fn ask_delete_agent(&mut self, a: &RosterAgent, cx: &mut Context<Self>) {
        let id = a.id.clone();
        let title = if zh() { format!("删除 {}？", a.name) } else { format!("Delete {}?", a.name) };
        self.confirm(
            title,
            t("Sessions can no longer @-mention it. Its past runs are kept as sessions."),
            t("Delete"),
            move |app, _, cx| {
                if let Err(e) = roster::delete(&app.paths, &id) {
                    app.toast(format!("{e}"), ToastKind::Error, cx);
                }
                app.reload_roster(cx);
            },
            cx,
        );
    }

    fn agent_menu(&mut self, a: &RosterAgent, at: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let (edit, copy, delete) = (a.clone(), format!("@{}", a.name), a.clone());
        let label = if zh() { format!("复制 @{}", a.name) } else { format!("Copy @{}", a.name) };
        let items = vec![
            entry(t("Edit"), move |app, window, cx| app.edit_agent(edit.clone(), false, window, cx)),
            entry(label, move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
            entry(t("Delete"), move |app, _, cx| app.ask_delete_agent(&delete, cx)).danger(),
        ];
        self.open_menu(at, items, cx);
    }

    fn edit_agent(&mut self, a: RosterAgent, is_new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let name_value = a.name.clone();
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("e.g. CodeReviewerAgent (no spaces)")).default_value(name_value));
        let text = a.instructions.clone();
        let instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(6, 10)
                .placeholder(t("What this agent does and how, e.g. \"Review the code for bugs and security problems. Don't change files; report findings.\""))
                .default_value(text)
        });
        name.update(cx, |s, cx| s.focus(window, cx));
        let known_cli = a.cli == "claude" || a.cli == "codex" || self.editor_provider(&a.cli).is_some();
        self.agent_editor = Some(AgentEditor {
            id: (!is_new).then(|| a.id.clone()),
            title: match (is_new, zh()) {
                (true, _) => t("Add agent").to_string(),
                (false, true) => format!("编辑 {}", a.name),
                (false, false) => format!("Edit {}", a.name),
            },
            shown_name: if a.name.is_empty() { "ThisAgent".into() } else { a.name.clone() },
            name,
            instructions,
            cli: if known_cli { a.cli.clone() } else { "claude".into() }, // its provider was deleted
            model: a.model.clone(),
            effort: a.effort.clone(),
            access: if a.access.is_empty() { "read".into() } else { a.access.clone() },
            installed: HashMap::new(),
            codex_models: None,
            codex_loading: false,
            error: None,
        });
        // Which CLIs are installed: one that isn't says so, with how to install it.
        let check = cx.background_spawn(async move { ["claude", "codex"].map(|c| (c.to_string(), crew_env::command_exists(c))) });
        cx.spawn(async move |this, cx| {
            let results = check.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(e) = app.agent_editor.as_mut() {
                    e.installed.extend(results);
                }
                cx.notify();
            });
        })
        .detach();
        self.load_codex_models(cx);
        cx.notify();
    }

    fn load_codex_models(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.agent_editor.as_mut() else { return };
        if e.cli != "codex" || e.codex_models.is_some() || e.codex_loading {
            return;
        }
        e.codex_loading = true;
        let service = self.service.clone();
        let load = cx.background_spawn(async move { service.codex_models() });
        cx.spawn(async move |this, cx| {
            let list = load.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(e) = app.agent_editor.as_mut() {
                    e.codex_models = list;
                    e.codex_loading = false;
                    app.fit_effort();
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn effort_levels(&self) -> Vec<String> {
        let Some(e) = &self.agent_editor else { return Vec::new() };
        if self.editor_provider(&e.cli).is_some() {
            return OTHER_EFFORTS.map(String::from).to_vec();
        }
        if e.cli == "codex" {
            let list = e.codex_models.as_deref().unwrap_or(&[]);
            let m = list.iter().find(|m| m.value == e.model).or_else(|| list.iter().find(|m| m.is_default));
            return m.filter(|m| !m.efforts.is_empty()).map_or_else(|| OTHER_EFFORTS.map(String::from).to_vec(), |m| m.efforts.clone());
        }
        CLAUDE_EFFORTS.map(String::from).to_vec()
    }

    /// A model that doesn't have the chosen effort goes back to its default.
    fn fit_effort(&mut self) {
        let levels = self.effort_levels();
        if let Some(e) = self.agent_editor.as_mut() {
            if !e.effort.is_empty() && !levels.contains(&e.effort) {
                e.effort.clear();
            }
        }
    }

    fn model_entries(&self, e: &AgentEditor) -> (String, Vec<(String, String)>, Option<String>) {
        let list: Vec<(String, String)>;
        let default_label;
        let mut hint = None;
        if let Some(p) = self.editor_provider(&e.cli) {
            list = p.models.iter().map(|m| (m.id.clone(), if m.name.is_empty() { m.id.clone() } else { m.name.clone() })).collect();
            let current = p.models.iter().find(|m| m.id == p.default_model).map_or(p.default_model.as_str(), |m| m.name.as_str());
            default_label = format!("{} ({})", t("Default"), if current.is_empty() { t("none set") } else { current });
            if list.is_empty() {
                hint = Some(t("Add models to this provider in Settings → Model providers.").to_string());
            }
        } else if e.cli == "claude" {
            list = CLAUDE_MODELS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
            default_label = t("Default (your Claude Code model)").to_string();
            hint = Some(t("Each one is the latest version of that model.").to_string());
        } else if let Some(models) = &e.codex_models {
            list = models.iter().map(|m| (m.value.clone(), m.label.clone())).collect();
            default_label = match models.iter().find(|m| m.is_default) {
                Some(m) => format!("{} ({})", t("Default"), m.label),
                None => t("Default (your Codex model)").to_string(),
            };
        } else {
            list = Vec::new();
            default_label = if e.codex_loading { t("Loading models…") } else { t("Default (your Codex model)") }.to_string();
            if !e.codex_loading && e.installed.get("codex") != Some(&false) {
                hint = Some(t("Couldn't load Codex's models; it uses its default.").to_string());
            }
        }
        (default_label, list, hint)
    }

    fn save_agent_editor(&mut self, cx: &mut Context<Self>) {
        let Some(e) = &self.agent_editor else { return };
        let input = RosterAgent {
            id: e.id.clone().unwrap_or_default(),
            name: e.name.read(cx).value().to_string(),
            cli: e.cli.clone(),
            model: e.model.clone(),
            effort: e.effort.clone(),
            instructions: e.instructions.read(cx).value().to_string(),
            access: e.access.clone(),
        };
        let providers: Vec<String> = self.providers.iter().map(|p| p.id.clone()).collect();
        match roster::save(&self.paths, input, |id| providers.iter().any(|p| p == id)) {
            Ok(_) => {
                self.agent_editor = None;
                self.reload_roster(cx);
            }
            Err(error) => {
                if let Some(e) = self.agent_editor.as_mut() {
                    e.error = Some(error);
                }
                cx.notify();
            }
        }
    }

    pub(super) fn render_agent_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let e = self.agent_editor.as_ref()?;
        let danger: gpui::Hsla = gpui::rgb(0xd93d42).into();
        let provider = self.editor_provider(&e.cli);
        let row = || div().flex().gap(px(10.));
        let hint = |text: String| div().text_size(px(12.)).text_color(c.muted).child(text);

        let not_installed = provider.is_none() && e.installed.get(&e.cli) == Some(&false);
        let cli_label = |cli: &str, installed: &HashMap<String, bool>| {
            let name = if cli == "codex" { "Codex" } else { "Claude Code" };
            if installed.get(cli) == Some(&false) { format!("{name} ({})", t("not installed")) } else { name.to_string() }
        };
        let mut cli_entries: Vec<MenuEntry> = ["claude", "codex"]
            .into_iter()
            .map(|cli| {
                entry(cli_label(cli, &e.installed), move |app, _, cx| {
                    if let Some(e) = app.agent_editor.as_mut() {
                        e.cli = cli.to_string();
                        e.model.clear();
                    }
                    app.fit_effort();
                    app.load_codex_models(cx);
                })
            })
            .collect();
        cli_entries.extend(self.providers.iter().map(|p| {
            let value = format!("provider:{}", p.id);
            entry(p.name.clone(), move |app, _, _| {
                if let Some(e) = app.agent_editor.as_mut() {
                    e.cli = value.clone();
                    e.model.clear();
                }
                app.fit_effort();
            })
        }));
        let cli_text = match provider {
            Some(p) => p.name.clone(),
            None => cli_label(&e.cli, &e.installed),
        };
        let cli_hint: (String, bool) = match provider {
            Some(p) if p.protocol == "anthropic" => (t("A custom model provider, run on Claude Code").to_string(), false),
            Some(_) => (t("A custom model provider, run on Codex").to_string(), false),
            None => match e.installed.get(&e.cli) {
                None => (t("Checking…").to_string(), false),
                Some(true) => (t("Installed").to_string(), false),
                Some(false) => (format!("{} {}", t("Not installed. Install it with:"), install_command(&e.cli)), true),
            },
        };

        let (default_label, models, model_hint) = self.model_entries(e);
        let model_text = if e.model.is_empty() {
            default_label.clone()
        } else {
            models.iter().find(|(v, _)| *v == e.model).map_or_else(|| format!("{} ({})", e.model, t("saved")), |(_, l)| l.clone())
        };
        let mut model_entries = vec![entry(default_label, |app, _, _| {
            if let Some(e) = app.agent_editor.as_mut() {
                e.model.clear();
            }
            app.fit_effort();
        })];
        model_entries.extend(models.into_iter().map(|(value, label)| {
            entry(label, move |app, _, _| {
                if let Some(e) = app.agent_editor.as_mut() {
                    e.model = value.clone();
                }
                app.fit_effort();
            })
        }));

        let effort_default = if provider.is_some() {
            t("Default (the model's)").to_string()
        } else if e.cli == "codex" {
            let list = e.codex_models.as_deref().unwrap_or(&[]);
            let m = list.iter().find(|m| m.value == e.model).or_else(|| list.iter().find(|m| m.is_default));
            m.filter(|m| !m.default_effort.is_empty()).map_or_else(|| t("Default (the model's)").to_string(), |m| format!("{} ({})", t("Default"), effort_label(&m.default_effort)))
        } else {
            t("Default (your Claude Code setting)").to_string()
        };
        let effort_text = if e.effort.is_empty() { effort_default.clone() } else { effort_label(&e.effort) };
        let mut effort_entries = vec![entry(effort_default, |app, _, _| {
            if let Some(e) = app.agent_editor.as_mut() {
                e.effort.clear();
            }
        })];
        effort_entries.extend(self.effort_levels().into_iter().map(|level| {
            entry(effort_label(&level), move |app, _, _| {
                if let Some(e) = app.agent_editor.as_mut() {
                    e.effort = level.clone();
                }
            })
        }));

        let access_entries: Vec<MenuEntry> = ["read", "edit", "full"]
            .into_iter()
            .map(|a| {
                entry(access_label(a), move |app, _, _| {
                    if let Some(e) = app.agent_editor.as_mut() {
                        e.access = a.to_string();
                    }
                })
            })
            .collect();
        let on_codex = provider.map_or(e.cli == "codex", |p| p.protocol != "anthropic");
        let warning = match e.access.as_str() {
            "full" => t("It runs any command and changes anything without asking you, including outside the project. Give this only to agents you trust with that, like a publisher."),
            "edit" if on_codex => t("Codex's sandbox keeps it to the project folder."),
            "edit" => t("Claude's auto mode lets it act, and blocks risky actions."),
            _ => t("It can look but not change anything."),
        };
        let mention_hint = if zh() {
            format!("在会话中提及它，例如：“添加定价页，然后让 @{} 审查。”主智能体会在同一文件夹把工作交给它，可以依次或同时进行，并取回它的回答。", e.shown_name)
        } else {
            format!("Mention it in a session, e.g. \"Add a pricing page, then have @{} review it.\" The main agent hands it the work in the same folder, one after another or several at once, and gets its answer back.", e.shown_name)
        };
        let delete_target = e.id.clone().and_then(|id| self.roster.iter().find(|a| a.id == id).cloned());

        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(self.heading(e.title.clone()))
            .child(
                row()
                    .child(self.field(t("Name"), Input::new(&e.name).small(), Some(t("What you @-mention").into())))
                    .child(
                        self.field(t("Runs on"), self.select("agent-cli", cli_text, cli_entries, cx), None)
                            .child(div().text_size(px(12.)).text_color(if cli_hint.1 { danger } else { c.muted }).child(cli_hint.0)),
                    ),
            )
            .child(
                row()
                    .child(self.field(t("Model"), self.select("agent-model", model_text, model_entries, cx), model_hint.map(Into::into)))
                    .child(self.field(t("Reasoning effort"), self.select("agent-effort", effort_text, effort_entries, cx), None)),
            )
            .child(self.field(t("Can"), self.select("agent-access", access_label(&e.access), access_entries, cx), None))
            .child(div().text_size(px(12.)).text_color(if e.access == "full" { danger } else { c.muted }).child(warning))
            .child(self.field(t("Instructions"), Textarea::new(&e.instructions).small(), None))
            .child(hint(mention_hint))
            .when_some(e.error.clone(), |d, error| d.child(div().px(px(10.)).py(px(6.)).rounded(px(8.)).bg(c.hover).text_size(px(12.5)).text_color(danger).child(error)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(4.))
                    .when_some(delete_target, |d, a| {
                        d.child(
                            div()
                                .id("agent-delete")
                                .text_size(px(13.))
                                .text_color(danger)
                                .cursor_pointer()
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.agent_editor = None;
                                    app.ask_delete_agent(&a, cx);
                                }))
                                .child(t("Delete")),
                        )
                    })
                    .child(div().flex_1())
                    .child(self.button("agent-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                        app.agent_editor = None;
                        cx.notify();
                    })))
                    .child(self.button("agent-save", t("Save"), true).when(not_installed, |d| d.opacity(0.4)).on_click(cx.listener(move |app, _, _, cx| {
                        if !not_installed {
                            app.save_agent_editor(cx);
                        }
                    }))),
            );
        Some(self.modal("agent-editor", 620., body))
    }

    pub(super) fn render_roster(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let actions = self
            .icon_button("agents-add", IconName::Plus)
            .p(px(4.))
            .on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                app.add_agent_menu(e.position, cx);
            }))
            .into_any_element();
        let head = self.section_head("agents-toggle", "Agents", "agentsCollapsed", actions, cx);
        let mut sorted: Vec<&RosterAgent> = self.roster.iter().collect();
        sorted.sort_by_key(|a| a.name.to_lowercase());
        let mut rows: Vec<AnyElement> = sorted
            .into_iter()
            .map(|a| {
                let access = match a.access.as_str() {
                    "edit" => t("edits"),
                    "full" => t("full access"),
                    _ => t("reads"),
                };
                let (edit, menu) = (a.clone(), a.clone());
                div()
                    .id(SharedString::from(format!("agent-{}", a.id)))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(34.))
                    .py(px(4.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(14.))
                    .text_color(c.text)
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |app, _, window, cx| app.edit_agent(edit.clone(), false, window, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &MouseDownEvent, _, cx| app.agent_menu(&menu, e.position, cx)))
                    .child(self.runner_icon(&a.cli))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(a.name.clone()))
                    .child(div().flex_none().text_size(px(12.)).text_color(c.muted).child(access))
                    .into_any_element()
            })
            .collect();
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
                    .child(t("Add agents, then @-mention them in a session: \"build this, then have @CodeReviewerAgent check it\"."))
                    .child(
                        div()
                            .id("agents-empty-add")
                            .text_color(c.accent)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                app.add_agent_menu(e.position, cx);
                            }))
                            .child(t("Add an agent")),
                    )
                    .into_any_element(),
            );
        }
        let collapsed = self.config.bool("agentsCollapsed");
        div()
            .flex()
            .flex_col()
            .flex_none()
            .max_h(relative(0.24))
            .child(head)
            .when(!collapsed, |d| d.child(div().id("agent-list").flex_1().min_h(px(0.)).overflow_y_scroll().pb(px(6.)).children(rows)))
            .into_any_element()
    }

    // ---------- live cards for runs ----------

    pub(super) fn on_run(&mut self, run: PublicRun, cx: &mut Context<Self>) {
        let finished = (run.status != "running").then(Instant::now);
        match self.run_cards.iter_mut().find(|c| c.run.id == run.id) {
            Some(card) => {
                card.run = run;
                card.finished = finished;
            }
            None => self.run_cards.insert(0, RunCard { run, finished }),
        }
        if self.run_cards.iter().any(|c| c.run.status == "running") && !self.run_clock {
            self.run_clock = true;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let more = this.update(cx, |app, cx| {
                        app.run_cards.retain(|c| c.finished.is_none_or(|t| t.elapsed() < RUN_CARD_LIFE));
                        cx.notify();
                        let more = !app.run_cards.is_empty();
                        app.run_clock = more;
                        more
                    });
                    if !more.unwrap_or(false) {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn open_run_session(&mut self, r: &PublicRun, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = r.session_id.clone() else { return };
        let session = Session {
            id,
            agent: "claude".into(),
            file: PathBuf::new(),
            cwd: Some(PathBuf::from(&r.cwd)),
            branch: None,
            title: Some(r.agent.clone()),
            mtime: 0.,
            size: 0,
            provider: None,
            worktree: None,
        };
        self.open_session(&session, Path::new(&r.cwd), window, cx);
    }

    pub(super) fn render_run_cards(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        if self.run_cards.is_empty() {
            return None;
        }
        let danger: gpui::Hsla = gpui::rgb(0xd93d42).into();
        let cards = self.run_cards.iter().map(|card| {
            let r = &card.run;
            let running = r.status == "running";
            let edge = match r.status.as_str() {
                "done" => c.running,
                "failed" => danger,
                "stopped" => c.muted,
                _ => c.accent,
            };
            let caller = self.open.iter().find(|o| o.id == r.caller).and_then(|o| o.title.clone());
            let id = SharedString::from(r.id.clone());
            let (dismiss_id, stop_id, shown) = (r.id.clone(), r.id.clone(), r.clone());
            let open_run = r.clone();
            let can_open = r.cli == "claude" && r.session_id.is_some() && !running;
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(10.))
                .rounded(px(10.))
                .bg(c.main_bg)
                .border_1()
                .border_l_3()
                .border_color(edge)
                .shadow_md()
                .text_size(px(12.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(self.runner_icon(&r.cli))
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(r.agent.clone()))
                        .when(running, |d| d.child(Spinner::new().color(c.accent).xsmall()))
                        .child(div().flex_1().min_w(px(0.)).text_color(c.muted).child(format!("{} · {}", status_label(&r.status), elapsed_text(r))))
                        .child(self.icon_button(SharedString::from(format!("run-x-{id}")), IconName::Close).p(px(2.)).on_click(cx.listener(move |app, _, _, cx| {
                            app.run_cards.retain(|c| c.run.id != dismiss_id);
                            cx.notify();
                        }))),
                )
                .child(div().truncate().child(r.task.split_whitespace().collect::<Vec<_>>().join(" ")))
                .when(!r.activity.is_empty(), |d| d.child(div().truncate().text_color(c.muted).child(r.activity.clone())))
                .when_some(caller, |d, title| d.child(div().truncate().text_color(c.muted).child(if zh() { format!("来自 {title}") } else { format!("for {title}") })))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .pt(px(2.))
                        .map(|d| {
                            if running {
                                d.child(self.button(SharedString::from(format!("run-stop-{id}")), t("Stop"), false).on_click(cx.listener(move |app, _, _, _| app.service.stop_run(&stop_id))))
                            } else {
                                let label = if shown.status == "done" { t("Result") } else { t("Details") };
                                d.child(self.button(SharedString::from(format!("run-result-{id}")), label, false).on_click(cx.listener(move |app, _, _, cx| {
                                    app.run_result = Some(RunResult { run: shown.clone() });
                                    cx.notify();
                                })))
                            }
                        })
                        .when(can_open, |d| {
                            d.child(self.button(SharedString::from(format!("run-open-{id}")), t("Open"), false).on_click(cx.listener(move |app, _, window, cx| app.open_run_session(&open_run, window, cx))))
                        }),
                )
        });
        Some(
            div()
                .id("run-cards")
                .absolute()
                .top(px(60.))
                .right(px(16.))
                .w(px(340.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .occlude()
                .children(cards)
                .into_any_element(),
        )
    }

    pub(super) fn render_run_result(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let r = &self.run_result.as_ref()?.run;
        let task: String = if r.task.chars().count() > 300 { format!("{}…", r.task.chars().take(300).collect::<String>()) } else { r.task.clone() };
        let text = if r.status == "done" {
            r.result.clone()
        } else {
            [r.error.as_str(), r.result.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n")
        };
        let text = if text.is_empty() { t("(no answer)").to_string() } else { text };
        let copy = if r.result.is_empty() { r.error.clone() } else { r.result.clone() };
        let status = if r.status == "done" { t("result") } else { status_label(&r.status) };
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(self.heading(format!("{}: {status}", r.agent)))
            .child(div().text_size(px(12.5)).text_color(c.muted).child(task))
            .child(
                div()
                    .id("run-result-text")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(c.hover)
                    .font_family("Cascadia Mono")
                    .text_size(px(12.5))
                    .child(text),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(self.button("run-copy", t("Copy"), false).on_click(cx.listener(move |_, _, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone())))))
                    .child(self.button("run-close", t("Close"), true).on_click(cx.listener(|app, _, _, cx| {
                        app.run_result = None;
                        cx.notify();
                    }))),
            );
        Some(self.modal("run-result", 640., body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str) -> RosterAgent {
        RosterAgent { name: name.into(), ..RosterAgent::default() }
    }

    #[test]
    fn the_note_goes_only_on_whole_mentions() {
        let roster = [agent("Reviewer"), agent("Tester")];
        assert_eq!(with_agent_note("no mention", &roster), "no mention");
        assert_eq!(with_agent_note("ask @Reviewer2", &roster), "ask @Reviewer2", "a longer name isn't this agent");
        let one = with_agent_note("have @reviewer check it", &roster);
        assert!(one.contains("(Crew: @Reviewer is an agent"), "{one}");
        let two = with_agent_note("@Reviewer then @Tester.", &roster);
        assert!(two.contains("@Reviewer, @Tester are agents"), "{two}");
    }

    #[test]
    fn elapsed_time_reads_like_the_other_app() {
        let run = PublicRun { id: String::new(), agent: String::new(), cli: String::new(), task: String::new(), cwd: String::new(), caller: String::new(), status: "done".into(), activity: String::new(), started_at: 1000, ended_at: 96_000, result: String::new(), error: String::new(), session_id: None };
        assert_eq!(elapsed_text(&run), "1m 35s");
    }
}
