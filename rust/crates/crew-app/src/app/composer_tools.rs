//! What sits around the prompt box: the model, effort and permission chips, whose picks run as the
//! agents' own commands in the session's terminal, and the suggestions after @, / and $.

use std::sync::atomic::{AtomicU64, Ordering};

use crew_core::composer::{self, CodexModel, Items, Settings, TriggerKind};
use crew_core::files;
use crew_service::remote::RemoteAsk;
use futures::lock::Mutex as AsyncMutex;
use gpui::{BackgroundExecutor, MouseDownEvent, Point, ScrollHandle, deferred};

use super::overlays::{MenuEntry, entry};
use super::*;

const CLAUDE_MODES: [(&str, &str); 4] = [("manual", "Manual"), ("acceptEdits", "Accept edits"), ("plan", "Plan"), ("auto", "Auto")];
const PROVIDER_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
// Codex's permission levels, as its /permissions picker names them. Crew shows them as a menu and
// makes the pick in that picker itself, confirming Full Access's "Enable full access?" step.
const CODEX_PERMISSIONS: [(&str, &str); 4] = [
    ("Ask for approval", "Read and edit workspace files and run commands; asks for internet access or edits outside the workspace"),
    ("Approve for me", "Only asks for actions detected as potentially unsafe"),
    ("Full Access", "Use with caution: edits files outside this workspace and uses the internet without asking"),
    ("Read Only", "Reads workspace files; asks for edits or internet access"),
];
const CLAUDE_ALIASES: [(&str, &str); 4] = [("fable", "Fable 5.1"), ("opus", "Opus 5.5"), ("sonnet", "Sonnet 5.5"), ("haiku", "Haiku 5.5")];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Chip {
    Model,
    Effort,
    Mode,
}

/// A session's chips and the commands they send; those run one at a time.
#[derive(Default)]
pub(super) struct SessionChips {
    pub(super) file: Settings,
    pub(super) live: Settings,
    pub(super) picked: Settings,
    pub(super) provider_model: Option<String>,
    pub(super) provider_effort: Option<String>,
    pub(super) codex_permission: Option<String>,
    pub(super) queue: Arc<AsyncMutex<()>>,
}

impl SessionChips {
    fn current(&self) -> Settings {
        self.picked.over(&self.live.over(&self.file))
    }
}

fn effort_label(e: &str) -> SharedString {
    if e == "xhigh" {
        return t("Extra high").into();
    }
    let mut c = e.chars();
    let capital = c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect());
    crate::i18n::lookup(&capital).map_or_else(|| capital.clone().into(), Into::into)
}

fn mode_label(mode: &str) -> Option<&'static str> {
    Some(match mode {
        "manual" | "default" => "Manual",
        "acceptEdits" => "Accept edits",
        "plan" => "Plan",
        "auto" => "Auto",
        "bypassPermissions" => "Bypass",
        "dontAsk" => "Don't ask",
        "untrusted" => "Untrusted",
        "on-request" => "On request",
        "on-failure" => "On failure",
        "never" => "Never ask",
        _ => return None,
    })
}

async fn sleep(ex: &BackgroundExecutor, ms: u64) {
    ex.timer(Duration::from_millis(ms)).await;
}

/// Bracketed paste, so newlines stay inside the message and "/" doesn't open the command menu
/// mid-typing; then Enter submits it.
async fn send_text(term: &Terminal, ex: &BackgroundExecutor, text: &str) {
    term.write(format!("\x1b[200~{}\x1b[201~", text.replace("\r\n", "\r").replace('\n', "\r")).as_bytes());
    sleep(ex, 150).await;
    term.write(b"\r");
}

/// A command pasted while Claude is still finishing the previous one can be dropped, so it waits
/// for an empty prompt, and is sent again once if Claude never echoes it.
async fn send_command(term: &Terminal, ex: &BackgroundExecutor, command: &str) {
    for _ in 0..2 {
        if term.exited() {
            return;
        }
        for _ in 0..40 {
            if composer::prompt_text(&term.screen_text()).as_deref() == Some("") {
                break;
            }
            sleep(ex, 100).await;
        }
        send_text(term, ex, command).await;
        // Echoed above the prompt box = submitted (the box itself would show it unsent).
        let echoed = format!("❯ {command}");
        for _ in 0..20 {
            sleep(ex, 100).await;
            let lines = term.screen_text();
            let the_box = lines.iter().rposition(|l| l.trim_start().starts_with('❯')).unwrap_or(0);
            if lines[..the_box].iter().any(|l| l.contains(&echoed)) {
                return;
            }
        }
    }
}

/// Shift+Tab cycles Claude's permission modes: press it until the status line says so.
async fn set_claude_mode(term: &Terminal, ex: &BackgroundExecutor, target: &str) {
    for _ in 0..6 {
        if composer::read_screen("claude", &term.screen_text()).mode.as_deref() == Some(target) {
            return;
        }
        term.write(b"\x1b[Z");
        sleep(ex, 400).await;
    }
}

async fn wait_for_codex_step(term: &Terminal, ex: &BackgroundExecutor, heading: &str) -> Option<composer::Picker> {
    for _ in 0..25 {
        if let Some(p) = composer::read_codex_picker(&term.screen_text()) {
            if p.heading.contains(heading) && !p.rows.is_empty() && p.highlighted.is_some() {
                return Some(p);
            }
        }
        sleep(ex, 200).await;
    }
    None
}

/// Moves the highlight to a row of the step showing now and presses Enter.
async fn choose_codex_row(term: &Terminal, ex: &BackgroundExecutor, index: usize) -> bool {
    for _ in 0..3 {
        let Some(at) = composer::read_codex_picker(&term.screen_text()).and_then(|p| p.highlighted) else { return false };
        if at == index {
            term.write(b"\r");
            return true;
        }
        let key: &[u8] = if index > at { b"\x1b[B" } else { b"\x1b[A" };
        for _ in 0..index.abs_diff(at) {
            term.write(key);
            sleep(ex, 250).await; // keys sent faster get merged
        }
        sleep(ex, 200).await;
    }
    false
}

async fn select_codex_permission(term: &Terminal, ex: &BackgroundExecutor, label: &str) -> Result<(), String> {
    send_text(term, ex, "/permissions").await;
    let Some(p) = wait_for_codex_step(term, ex, "Update Model Permissions").await else {
        return Err(t("Codex didn't open its permissions — try again when it's idle").to_string());
    };
    let Some(row) = p.rows.iter().find(|r| r.label.eq_ignore_ascii_case(label)) else {
        term.write(b"\x1b");
        return Err(format!("Codex doesn't offer \"{label}\" here"));
    };
    if !choose_codex_row(term, ex, row.index).await {
        return Err(t("Codex didn't open its permissions — try again when it's idle").to_string());
    }
    if label == "Full Access" {
        // Codex asks once more, unless the user turned that warning off.
        for _ in 0..8 {
            let shown = composer::read_codex_picker(&term.screen_text()).filter(|c| c.heading.contains("Enable full access") && c.highlighted.is_some());
            if let Some(yes) = shown.and_then(|c| c.rows.into_iter().find(|r| r.label.to_lowercase().starts_with("yes, continue anyway"))) {
                choose_codex_row(term, ex, yes.index).await;
                break;
            }
            sleep(ex, 200).await;
        }
    }
    Ok(())
}

impl CrewApp {
    /// The current picture of the agents' models, loaded at startup.
    pub(super) fn load_model_lists(&mut self, cx: &mut Context<Self>) {
        self.load_claude_catalog(false, cx);
    }

    fn load_claude_catalog(&mut self, force: bool, cx: &mut Context<Self>) {
        let (paths, command) = (self.paths.clone(), self.config.str("claudeCommand").to_string());
        let work = cx.background_spawn(async move { composer::claude_catalog(&paths, &command, force) });
        cx.spawn(async move |this, cx| {
            let catalog = work.await;
            let _ = this.update(cx, |app, cx| {
                app.claude_catalog = Some(Arc::new(catalog));
                cx.notify();
            });
        })
        .detach();
    }

    /// Codex's models by name ("GPT-5.6-Sol", not "gpt-5.6-sol") need its list: fetched on demand.
    fn fetch_codex_models(&mut self, then: Option<(Chip, Point<Pixels>)>, cx: &mut Context<Self>) {
        if self.codex_list_loading {
            return;
        }
        self.codex_list_loading = true;
        let command = self.config.str("codexCommand").to_string();
        let work = cx.background_spawn(async move { composer::codex_models(&command) });
        cx.spawn(async move |this, cx| {
            let list = work.await;
            let _ = this.update(cx, |app, cx| {
                app.codex_list_loading = false;
                if let Some(list) = list {
                    app.codex_list = Some((Instant::now(), Arc::new(list)));
                } else if then.is_some() {
                    app.hint = Some(t("Couldn't read Codex's model list — try again when Codex is idle").to_string());
                }
                if let Some((chip, at)) = then {
                    if app.codex_list.is_some() {
                        app.open_chip(chip, at, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// What a session's file saved. Called when it's shown, and now and then while it's in front,
    /// as a reply reports the model it really ran on.
    pub(super) fn load_session_settings(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(o) = self.by_key(key).filter(|o| o.agent != "ssh") else { return };
        let (paths, id, agent) = (self.paths.clone(), o.id.clone(), o.agent.clone());
        let work = cx.background_spawn(async move {
            let homes = providers::homes(&paths);
            composer::session_state(&paths, &homes, &id, &agent)
        });
        cx.spawn(async move |this, cx| {
            let state = work.await;
            let _ = this.update(cx, |app, cx| app.settings_loaded(key, state, cx));
        })
        .detach();
    }

    fn settings_loaded(&mut self, key: u64, state: Settings, cx: &mut Context<Self>) {
        let Some(o) = self.open.iter_mut().find(|o| o.key == key) else { return };
        if o.chips.file == state {
            return;
        }
        // Once the file reports a different model (e.g. after the next reply), it supersedes the pick.
        if o.chips.file.model.is_some() && o.chips.file.model != state.model {
            o.chips.picked.model = None;
        }
        o.chips.file = state;
        cx.notify();
    }

    /// Reads mode, effort and model off the status line of the session in front.
    pub(super) fn read_active_screen(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.layout.focused_session() else { return };
        let Some(o) = self.by_key(key).filter(|o| !o.exited && o.agent != "ssh") else { return };
        let live = composer::read_screen(&o.agent, &o.view.read(cx).term.screen_text());
        let codex = o.agent == "codex";
        let Some(o) = self.open.iter_mut().find(|o| o.key == key) else { return };
        let merged = live.over(&o.chips.live);
        let changed = merged != o.chips.live;
        o.chips.live = merged;
        // A pick stays shown until the status line confirms it: Codex briefly redraws the old
        // values right after a change.
        if codex {
            let same = |a: &Option<String>, b: &Option<String>| a.as_deref().zip(b.as_deref()).is_some_and(|(x, y)| x.eq_ignore_ascii_case(y));
            if same(&live.model, &o.chips.picked.model) {
                o.chips.picked.model = None;
            }
            if same(&live.effort, &o.chips.picked.effort) {
                o.chips.picked.effort = None;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn model_label(&self, agent: &str, model: Option<&str>) -> SharedString {
        let Some(m) = model else { return t("Model").into() };
        if agent == "codex" {
            // Session files say "gpt-6-luna"; show it the way Codex spells it ("GPT-6-Luna").
            if let Some((_, list)) = &self.codex_list {
                if let Some(x) = list.iter().find(|x| x.value.eq_ignore_ascii_case(m) || x.label.eq_ignore_ascii_case(m)) {
                    return x.label.clone().into();
                }
            }
            if m.to_lowercase().starts_with("gpt-") {
                let mut out = String::from("GPT");
                for part in m[3..].split('-').filter(|p| !p.is_empty()) {
                    out.push('-');
                    let mut c = part.chars();
                    if let Some(f) = c.next() {
                        out.extend(f.to_uppercase());
                        out.push_str(c.as_str());
                    }
                }
                return out.into();
            }
            return m.to_string().into();
        }
        if let Some(known) = self.claude_catalog.as_ref().and_then(|c| c.find(m)) {
            return known.label.clone().into();
        }
        if let Some((_, label)) = CLAUDE_ALIASES.iter().find(|(a, _)| *a == m) {
            return (*label).into();
        }
        m.to_string().into()
    }

    /// The model, effort and mode chips' text for a session.
    fn chip_labels(&self, o: &OpenSession) -> (SharedString, SharedString, SharedString) {
        self.chip_labels_of(&o.agent, &o.chips, o.provider.as_ref())
    }

    fn chip_labels_of(&self, agent: &str, chips: &SessionChips, provider: Option<&String>) -> (SharedString, SharedString, SharedString) {
        let s = chips.current();
        let codex = agent == "codex";
        let provider = provider.and_then(|id| self.providers.iter().find(|p| p.id == *id));
        let (model, effort): (SharedString, SharedString) = if let Some(p) = provider {
            // A custom provider's session shows its own models and effort, not Claude's or Codex's.
            let id = chips.provider_model.clone().unwrap_or_else(|| p.default_model.clone());
            let label: SharedString = match p.models.iter().find(|m| m.id == id) {
                Some(m) if !m.name.is_empty() => m.name.clone().into(),
                _ if id.is_empty() => t("Model").into(),
                _ => id.into(),
            };
            (label, chips.provider_effort.as_deref().map_or_else(|| t("Effort").into(), effort_label))
        } else {
            (self.model_label(agent, s.model.as_deref()), s.effort.as_deref().map_or(SharedString::default(), effort_label))
        };
        let mode: SharedString = if let Some(p) = chips.codex_permission.as_ref().filter(|_| codex) {
            crate::i18n::lookup(p).map_or_else(|| p.clone().into(), Into::into)
        } else if let Some(label) = s.mode.as_deref().and_then(mode_label) {
            t(label).into()
        } else if codex {
            t("Permissions").into()
        } else {
            t("Manual").into()
        };
        (model, effort, mode)
    }

    /// Sends a command through the session's queue.
    fn queued(&self, key: u64, cx: &mut Context<Self>, command: String, paste_once: bool) {
        let Some(o) = self.by_key(key) else { return };
        let (term, queue) = (o.view.read(cx).term.clone(), o.chips.queue.clone());
        cx.spawn(async move |_, cx| {
            let _turn = queue.lock().await;
            let ex = cx.background_executor().clone();
            if paste_once {
                // Codex's prompt isn't Claude's: send once.
                send_text(&term, &ex, &command).await;
                sleep(&ex, 800).await;
            } else {
                send_command(&term, &ex, &command).await;
            }
        })
        .detach();
    }

    fn chips_of(&mut self, key: u64) -> Option<&mut SessionChips> {
        self.open.iter_mut().find(|o| o.key == key).map(|o| &mut o.chips)
    }

    fn claude_model_items(&self, key: u64, o: &OpenSession) -> Vec<MenuEntry> {
        let current = o.chips.current().model;
        let mut items = Vec::new();
        let catalog = self.claude_catalog.clone();
        let on_default = current.as_deref().is_some_and(|m| m.to_lowercase().starts_with("default"));
        let now = catalog.as_ref().filter(|_| !on_default).and_then(|c| c.find(current.as_deref()?)).map(|m| m.slug.clone());
        let default_label = catalog.as_ref().and_then(|c| c.find("default")).map_or(String::new(), |m| m.label.clone());
        items.push(
            entry("Default (recommended)", move |app, _, cx| {
                app.queued(key, cx, "/model default".to_string(), false);
                if let Some(c) = app.chips_of(key) {
                    c.picked.model = Some("Default".to_string());
                }
            })
            .hint(default_label)
            .checked(on_default),
        );
        let mut older_seen = false;
        for m in catalog.iter().flat_map(|c| c.models.iter()) {
            // By its full id: a short name like "fable" means whichever model the installed Claude
            // Code was built with, which can be older than the one the list shows.
            let slug = m.slug.clone();
            let mut row = entry(m.label.clone(), move |app, _, cx| {
                app.queued(key, cx, format!("/model {slug}"), false);
                if let Some(c) = app.chips_of(key) {
                    c.picked.model = Some(slug.clone());
                }
            })
            .checked(now.as_deref() == Some(m.slug.as_str()));
            if !m.current && !older_seen {
                older_seen = true;
                row = row.separated().hint(t("Older models"));
            }
            items.push(row);
        }
        items.push(entry("Refresh model list", |app, _, cx| app.load_claude_catalog(true, cx)).separated());
        items
    }

    fn claude_effort_items(&mut self, key: u64, o_effort: Option<String>, model: Option<String>) -> Option<Vec<MenuEntry>> {
        let found = self.claude_catalog.as_ref().and_then(|c| c.find(model.as_deref()?)).cloned();
        let levels: Vec<(String, bool)> = match &found {
            Some(m) => m.efforts.iter().map(|e| (e.id.clone(), e.is_default)).collect(),
            None => CLAUDE_EFFORTS.iter().map(|e| ((*e).to_string(), false)).collect(),
        };
        if levels.is_empty() {
            self.hint = found.map(|m| format!("{} has no reasoning effort levels", m.label));
            return None;
        }
        Some(
            levels
                .into_iter()
                .map(|(id, is_default)| {
                    let on = o_effort.as_deref() == Some(id.as_str());
                    let label = effort_label(&id);
                    entry(label, move |app, _, cx| {
                        if let Some(c) = app.chips_of(key) {
                            c.live.effort = Some(id.clone());
                        }
                        app.queued(key, cx, format!("/effort {id}"), false);
                    })
                    .hint(if is_default { t("Default") } else { "" })
                    .checked(on)
                })
                .collect(),
        )
    }

    fn switch_codex(&mut self, key: u64, model: &CodexModel, effort: &str, cx: &mut Context<Self>) {
        let command = if effort.is_empty() { format!("/model {}", model.value) } else { format!("/model {} --reasoning={effort}", model.value) };
        self.queued(key, cx, command, true);
        if let Some(c) = self.chips_of(key) {
            c.picked.model = Some(model.value.clone());
            if !effort.is_empty() {
                c.picked.effort = Some(effort.to_string());
            }
        }
    }

    fn codex_model_items(&self, key: u64, o: &OpenSession, list: &Arc<Vec<CodexModel>>) -> Vec<MenuEntry> {
        let now = o.chips.current();
        let model_now = now.model.as_deref().unwrap_or("").to_lowercase();
        let mut items: Vec<MenuEntry> = list
            .iter()
            .map(|m| {
                // Keep the current effort if the new model has it, otherwise that model's default.
                let keep = now.effort.clone().filter(|e| m.efforts.contains(e));
                let effort = keep.unwrap_or_else(|| if m.default_effort.is_empty() { m.efforts.first().cloned().unwrap_or_default() } else { m.default_effort.clone() });
                let on = m.value.to_lowercase() == model_now || m.label.to_lowercase() == model_now;
                let chosen = m.clone();
                entry(m.label.clone(), move |app, _, cx| app.switch_codex(key, &chosen, &effort, cx)).checked(on)
            })
            .collect();
        items.push(
            entry("Refresh model list", |app, _, cx| {
                app.codex_list = None;
                app.fetch_codex_models(None, cx);
            })
            .separated(),
        );
        items
    }

    fn codex_effort_items(&mut self, key: u64, o: &OpenSession, list: &Arc<Vec<CodexModel>>) -> Option<Vec<MenuEntry>> {
        let now = o.chips.current();
        let model_now = now.model.as_deref().unwrap_or("").to_lowercase();
        let Some(model) = list.iter().find(|m| m.value.to_lowercase() == model_now || m.label.to_lowercase() == model_now) else {
            self.hint = Some(t("Current Codex model not found — use Refresh model list").to_string());
            return None;
        };
        Some(
            model
                .efforts
                .iter()
                .map(|e| {
                    let (chosen, level) = (model.clone(), e.clone());
                    entry(effort_label(e), move |app, _, cx| app.switch_codex(key, &chosen, &level, cx)).checked(now.effort.as_deref() == Some(e.as_str()))
                })
                .collect(),
        )
    }

    fn provider_items(&mut self, key: u64, chip: Chip, o: &OpenSession, provider: &Provider) -> Option<Vec<MenuEntry>> {
        let codex = o.agent == "codex";
        let default_model = provider.default_model.clone();
        if chip == Chip::Model {
            if provider.models.is_empty() {
                self.hint = Some(format!("No models are listed for {}: add them in Settings → Model providers", provider.name));
                return None;
            }
            let current = o.chips.provider_model.clone().unwrap_or_else(|| default_model.clone());
            return Some(
                provider
                    .models
                    .iter()
                    .map(|m| {
                        let id = m.id.clone();
                        let on = id == current;
                        entry(m.id.clone(), move |app, _, cx| {
                            let effort = app.chips_of(key).and_then(|c| {
                                c.provider_model = Some(id.clone());
                                c.provider_effort.clone()
                            });
                            let command = match effort.filter(|_| codex) {
                                Some(e) => format!("/model {id} --reasoning={e}"),
                                None => format!("/model {id}"),
                            };
                            app.queued(key, cx, command, codex);
                            app.remember_provider_picks(key);
                        })
                        .checked(on)
                    })
                    .collect(),
            );
        }
        let now = o.chips.provider_effort.clone();
        let pick = move |level: Option<&'static str>, default_model: String| {
            move |app: &mut CrewApp, _: &mut Window, cx: &mut Context<CrewApp>| {
                let model = app.chips_of(key).map(|c| {
                    c.provider_effort = level.map(String::from);
                    c.provider_model.clone().unwrap_or_else(|| default_model.clone())
                });
                if let Some(level) = level {
                    if codex {
                        app.queued(key, cx, format!("/model {} --reasoning={level}", model.unwrap_or_default()), true);
                    } else {
                        app.queued(key, cx, format!("/effort {level}"), false);
                    }
                }
                app.remember_provider_picks(key);
            }
        };
        let mut items = vec![entry("Default (the model's)", pick(None, default_model.clone())).checked(now.is_none())];
        for level in PROVIDER_EFFORTS {
            items.push(entry(effort_label(level), pick(Some(level), default_model.clone())).checked(now.as_deref() == Some(level)));
        }
        Some(items)
    }

    /// Keeps a provider session's picks for its next resume.
    fn remember_provider_picks(&mut self, key: u64) {
        let Some(o) = self.by_key(key) else { return };
        let Some(provider) = o.provider.clone() else { return };
        providers::bind(&self.paths, &o.id, &providers::Binding { provider, model: o.chips.provider_model.clone(), effort: o.chips.provider_effort.clone() });
    }

    fn mode_items(&self, key: u64, o: &OpenSession) -> Vec<MenuEntry> {
        if o.agent == "codex" {
            return CODEX_PERMISSIONS
                .iter()
                .map(|(label, hint)| {
                    let on = o.chips.codex_permission.as_deref() == Some(*label);
                    entry(*label, move |app, _, cx| app.pick_codex_permission(key, label, cx)).hint(t(hint)).checked(on)
                })
                .collect();
        }
        let current = o.chips.current().mode;
        CLAUDE_MODES
            .iter()
            .map(|(mode, label)| {
                let on = current.as_deref() == Some(*mode) || (*mode == "manual" && current.as_deref() == Some("default"));
                entry(*label, move |app, _, cx| {
                    let Some(o) = app.by_key(key) else { return };
                    let (term, queue) = (o.view.read(cx).term.clone(), o.chips.queue.clone());
                    cx.spawn(async move |_, cx| {
                        let _turn = queue.lock().await;
                        let ex = cx.background_executor().clone();
                        set_claude_mode(&term, &ex, mode).await;
                    })
                    .detach();
                })
                .checked(on)
            })
            .collect()
    }

    fn pick_codex_permission(&mut self, key: u64, label: &'static str, cx: &mut Context<Self>) {
        let Some(o) = self.by_key(key) else { return };
        let (term, queue) = (o.view.read(cx).term.clone(), o.chips.queue.clone());
        cx.spawn(async move |this, cx| {
            let _turn = queue.lock().await;
            let ex = cx.background_executor().clone();
            let result = select_codex_permission(&term, &ex, label).await;
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(()) => {
                        if let Some(c) = app.chips_of(key) {
                            c.codex_permission = Some(label.to_string());
                        }
                    }
                    Err(why) => app.hint = Some(why),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_chip(&mut self, chip: Chip, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.hint = None;
        let Some(key) = self.layout.focused_session() else { return };
        let Some(i) = self.open.iter().position(|o| o.key == key) else { return };
        if self.open[i].exited || self.open[i].agent == "ssh" {
            return;
        }
        let o = self.open.remove(i);
        let items = self.chip_items(key, chip, &o, at, cx);
        self.open.insert(i, o);
        if let Some(items) = items.filter(|i| !i.is_empty()) {
            self.open_menu_above(at, items, cx);
        }
        cx.notify();
    }

    fn chip_items(&mut self, key: u64, chip: Chip, o: &OpenSession, at: Point<Pixels>, cx: &mut Context<Self>) -> Option<Vec<MenuEntry>> {
        if chip == Chip::Mode {
            return Some(self.mode_items(key, o));
        }
        if let Some(provider) = o.provider.as_ref().and_then(|id| self.providers.iter().find(|p| p.id == *id)).cloned() {
            return self.provider_items(key, chip, o, &provider);
        }
        if o.agent != "codex" {
            return match chip {
                Chip::Model => Some(self.claude_model_items(key, o)),
                _ => {
                    let now = o.chips.current();
                    self.claude_effort_items(key, now.effort, now.model)
                }
            };
        }
        let list = self.codex_list.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(600)).map(|x| x.1.clone());
        let Some(list) = list else {
            // The menu opens when the list arrives.
            self.fetch_codex_models(Some((chip, at)), cx);
            return None;
        };
        match chip {
            Chip::Model => Some(self.codex_model_items(key, o, &list)),
            _ => self.codex_effort_items(key, o, &list),
        }
    }

    /// A phone shows a session's chips and opens their menus; the picks run here, as if made in this
    /// composer. This window loads a session's saved settings when it's switched to; one the phone
    /// asks about may never have been (a background pane, a session the phone opened), so they're
    /// loaded first. A session that isn't open has its chips from what its file saved (and a custom
    /// provider's picks), marked closed, since switching needs the running session.
    pub(super) fn phone_chips(&mut self, ask: RemoteAsk, cx: &mut Context<Self>) {
        let running = self.open.iter().find(|o| o.id == ask.session_id && !o.exited && (o.agent == "claude" || o.agent == "codex"));
        let key = running.map(|o| o.key);
        let saved = self.projects.iter().flat_map(|p| p.sessions.iter()).find(|s| s.id == ask.session_id);
        let agent = running.map(|o| o.agent.clone()).or_else(|| self.open.iter().find(|o| o.id == ask.session_id).map(|o| o.agent.clone())).or_else(|| saved.map(|s| s.agent.clone())).unwrap_or_default();
        let provider = running.and_then(|o| o.provider.clone()).or_else(|| saved.and_then(|s| s.provider.clone()));
        let usable = agent == "claude" || agent == "codex";
        let is_menu = ask.what == "menu";
        let needs_list = agent == "codex" && provider.is_none() && if is_menu { !self.codex_list_fresh() } else { self.codex_list.is_none() };
        if needs_list {
            self.fetch_codex_models(None, cx);
        }
        let (paths, id) = (self.paths.clone(), ask.session_id.clone());
        let state_agent = agent.clone();
        cx.spawn(async move |this, cx| {
            if needs_list {
                // A Codex session's model is named from Codex's list: give it a moment to load the first time.
                let ex = cx.background_executor().clone();
                for _ in 0..if is_menu { 150 } else { 35 } {
                    sleep(&ex, 100).await;
                    if !this.update(cx, |app, _| app.codex_list_loading).unwrap_or(false) {
                        break;
                    }
                }
            }
            let state = if usable {
                cx.background_spawn(async move {
                    let homes = providers::homes(&paths);
                    composer::session_state(&paths, &homes, &id, &state_agent)
                })
                .await
            } else {
                Settings::default()
            };
            let _ = this.update(cx, |app, cx| {
                if let Some(key) = key {
                    app.settings_loaded(key, state.clone(), cx);
                }
                let answer = if is_menu { app.phone_menu(key, &ask.kind, cx) } else { app.phone_chip_labels(key, &ask.session_id, &agent, state) };
                app.service.remote_answer(ask.id, answer);
            });
        })
        .detach();
    }

    fn codex_list_fresh(&self) -> bool {
        self.codex_list.as_ref().is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(600))
    }

    fn phone_chip_labels(&self, key: Option<u64>, session_id: &str, agent: &str, state: Settings) -> Value {
        if agent != "claude" && agent != "codex" {
            return Value::Null;
        }
        let labels = |(model, effort, mode): (SharedString, SharedString, SharedString)| json!({ "model": model.as_ref(), "effort": effort.as_ref(), "mode": mode.as_ref() });
        if let Some(o) = key.and_then(|k| self.by_key(k)) {
            return labels(self.chip_labels(o));
        }
        let binding = providers::binding(&self.paths, session_id);
        let provider = binding.as_ref().map(|b| b.provider.clone());
        let chips = SessionChips { file: state, provider_model: binding.as_ref().and_then(|b| b.model.clone()), provider_effort: binding.and_then(|b| b.effort), ..SessionChips::default() };
        let mut out = labels(self.chip_labels_of(agent, &chips, provider.as_ref()));
        out["closed"] = json!(true);
        out
    }

    fn phone_menu(&mut self, key: Option<u64>, kind: &str, cx: &mut Context<Self>) -> Value {
        self.hint = None;
        let chip = match kind {
            "model" => Some(Chip::Model),
            "effort" => Some(Chip::Effort),
            "mode" => Some(Chip::Mode),
            _ => None,
        };
        let i = key.and_then(|k| self.open.iter().position(|o| o.key == k));
        let mut items = None;
        if let (Some(key), Some(i), Some(chip)) = (key, i, chip) {
            let waits_for_list = chip != Chip::Mode && self.open[i].agent == "codex" && self.open[i].provider.is_none() && !self.codex_list_fresh();
            if waits_for_list {
                self.hint = Some(t("Couldn't read Codex's model list — try again when Codex is idle").to_string());
            } else {
                let o = self.open.remove(i);
                items = self.chip_items(key, chip, &o, Point::default(), cx);
                self.open.insert(i, o);
            }
        }
        let Some(items) = items.filter(|i| !i.is_empty()) else {
            return json!({ "id": 0, "items": [], "hint": self.hint.clone().unwrap_or_default() });
        };
        self.phone.menu_seq += 1;
        let id = self.phone.menu_seq;
        let rows: Vec<Value> = items.iter().map(|it| json!({ "label": it.label.as_ref(), "hint": it.hint.as_ref(), "checked": it.checked, "separator": it.separator })).collect();
        self.phone.menus.push_back((id, items));
        if self.phone.menus.len() > 20 {
            self.phone.menus.pop_front();
        }
        json!({ "id": id, "items": rows })
    }

    fn project_chip_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(o) = self.active() else { return };
        let cwd = o.cwd.clone();
        let (a, b, c, d) = (cwd.clone(), cwd.clone(), cwd.clone(), cwd);
        self.open_menu_above(
            at,
            vec![
                entry("New Claude session here", move |app, window, cx| app.new_session("claude", a.clone(), window, cx)),
                entry("New Codex session here", move |app, window, cx| app.new_session("codex", b.clone(), window, cx)),
                entry("Open folder", move |_, _, cx| cx.open_with_system(&c)),
                entry("Copy path", move |app, _, cx| app.copy_text(d.to_string_lossy(), cx)),
            ],
            cx,
        );
    }

    /// The chips under the prompt box. Sessions on a host's terminal have only its name.
    pub(super) fn render_chips(&self, o: &OpenSession, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = self.colors;
        if o.agent == "ssh" || o.agent == "install" {
            return Vec::new();
        }
        let (model, effort, mode) = self.chip_labels(o);
        let provider = o.provider.as_ref().and_then(|id| self.providers.iter().find(|p| p.id == *id));
        let badge: AnyElement = match provider {
            Some(p) => self.provider_icon(p),
            None => self.agent_badge(&o.agent, 12.),
        };
        let button = |id: &'static str, chip: Chip, strong: bool, content: AnyElement| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.))
                .px(px(8.))
                .py(px(3.))
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(if strong { c.text } else { c.muted })
                .hover(move |s| s.bg(c.hover))
                .on_mouse_down(MouseButton::Left, cx.listener(move |app, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    app.open_chip(chip, e.position, cx);
                }))
                .child(content)
                .into_any_element()
        };
        let mut chips = vec![button("chip-model", Chip::Model, true, div().flex().items_center().gap(px(5.)).child(badge).child(model).into_any_element())];
        if !effort.is_empty() {
            chips.push(button("chip-effort", Chip::Effort, false, div().child(effort).into_any_element()));
        }
        chips.push(button("chip-mode", Chip::Mode, true, div().child(mode).into_any_element()));
        chips
    }

    pub(super) fn render_project_chip(&self, name: String, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        div()
            .id("chip-project")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(7.))
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(c.muted)
            .hover(move |s| s.bg(c.hover))
            .on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                app.project_chip_menu(e.position, cx);
            }))
            .child(format!("{name} ▾"))
            .into_any_element()
    }

    /// Why the box can't send right now, or what a chip just couldn't do.
    pub(super) fn composer_hint(&self, o: &OpenSession) -> Option<SharedString> {
        if o.exited {
            return Some(if o.agent == "ssh" { t("Disconnected — press Enter to reconnect") } else { t("Session stopped — press Enter to resume it") }.into());
        }
        self.hint.clone().map(Into::into)
    }
}

// ---------- the suggestions after @, / and $ ----------

#[derive(Clone)]
pub(super) enum SuggestIcon {
    None,
    File,
    Folder,
    Runner(String),
}

#[derive(Clone)]
pub(super) struct SuggestItem {
    icon: SuggestIcon,
    label: String,
    description: String,
    badge: &'static str,
    insert: String,
    /// What's searched: the name without its @, / or $.
    name: String,
    rel: String,
}

pub(super) struct Suggest {
    kind: TriggerKind,
    start: usize,
    caret: usize,
    items: Vec<SuggestItem>,
    index: usize,
    loading: bool,
    scroll: ScrollHandle,
}

static FILE_SEARCH: AtomicU64 = AtomicU64::new(0);

/// Nothing typed yet keeps the list's own order; among equal matches, shorter paths come first.
fn ranked(list: Vec<SuggestItem>, query: &str, limit: usize) -> Vec<SuggestItem> {
    if query.is_empty() {
        return list.into_iter().take(limit).collect();
    }
    let mut scored: Vec<(u8, SuggestItem)> = list.into_iter().filter_map(|it| Some((composer::match_score(&it.name, &it.description, query)?, it))).collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.rel.len().cmp(&b.1.rel.len())).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.into_iter().take(limit).map(|x| x.1).collect()
}

fn skill_row(s: &composer::Skill, codex: bool, as_command: bool) -> SuggestItem {
    let slash = as_command && !codex;
    SuggestItem {
        icon: SuggestIcon::None,
        label: if slash { format!("/{}", s.name) } else { format!("${}", s.name) },
        description: s.description.clone(),
        badge: composer::scope_label(&s.scope),
        insert: if slash { format!("/{} ", s.name) } else { format!("${} ", s.name) },
        name: s.name.clone(),
        rel: String::new(),
    }
}

/// Files and folders for an @: the folder's own when nothing's typed, otherwise a search.
fn file_rows(root: &Path, query: &str) -> Vec<SuggestItem> {
    let rows: Vec<(String, bool)> = if query.is_empty() {
        files::list(root, "").unwrap_or_default().into_iter().filter(|i| !i.ignored).take(30).map(|i| (i.rel, i.dir)).collect()
    } else {
        let found: Vec<String> = files::search_names(root, query).into_iter().map(|h| h.rel).collect();
        let needle = query.to_lowercase();
        let mut folders: Vec<String> = Vec::new();
        for f in &found {
            let parts: Vec<&str> = f.split('/').collect();
            for i in 1..parts.len() {
                if parts[i - 1].to_lowercase().contains(&needle) {
                    let folder = parts[..i].join("/");
                    if !folders.contains(&folder) {
                        folders.push(folder);
                    }
                }
            }
        }
        folders.into_iter().map(|f| (f, true)).chain(found.into_iter().map(|f| (f, false))).collect()
    };
    let items: Vec<SuggestItem> = rows
        .into_iter()
        .map(|(rel, dir)| {
            let (parent, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
            let slash = if dir { "/" } else { "" };
            let reference = if rel.contains(char::is_whitespace) { format!("\"{rel}{slash}\"") } else { format!("{rel}{slash}") };
            SuggestItem {
                icon: if dir { SuggestIcon::Folder } else { SuggestIcon::File },
                label: name.to_string(),
                description: parent.to_string(),
                badge: "",
                insert: format!("@{reference} "),
                name: name.to_string(),
                rel: rel.clone(),
            }
        })
        .collect();
    if query.is_empty() { items } else { ranked(items, &query.to_lowercase(), 40) }
}

impl CrewApp {
    fn items_key(&self, o: &OpenSession) -> (String, String) {
        let agent = if o.agent == "codex" { "codex" } else { "claude" }.to_string();
        let key = format!("{agent}|{}|{}", o.cwd.display(), o.provider.as_deref().unwrap_or(""));
        (agent, key)
    }

    /// What the agent offers in this folder, read again after a minute. None while it's loading;
    /// the suggestions update when it arrives.
    fn agent_items(&mut self, o_key: u64, cx: &mut Context<Self>) -> Option<Rc<Items>> {
        let o = self.by_key(o_key)?;
        let (agent, key) = self.items_key(o);
        let (cwd, provider) = (o.cwd.clone(), o.provider.clone());
        let cached = self.items_cache.get(&key);
        let expired = cached.is_none_or(|c| c.0.elapsed() > Duration::from_secs(60));
        let data = cached.and_then(|c| c.1.clone());
        if expired {
            self.items_cache.insert(key.clone(), (Instant::now(), data.clone()));
            let paths = self.paths.clone();
            let command = self.config.str("codexCommand");
            let command = if command.is_empty() { "codex".to_string() } else { command.to_string() };
            let work = cx.background_spawn(async move {
                let home = provider.and_then(|id| providers::homes(&paths).into_iter().find(|h| h.id == id).map(|h| h.home));
                if agent == "codex" { composer::codex_items(&paths, &command, &cwd, home.as_deref()) } else { composer::claude_items(&paths, &cwd, home.as_deref()) }
            });
            cx.spawn(async move |this, cx| {
                let items = work.await;
                let _ = this.update(cx, |app, cx| {
                    app.items_cache.insert(key, (Instant::now(), Some(Rc::new(items))));
                    app.update_suggest(cx);
                });
            })
            .detach();
        }
        data
    }

    pub(super) fn update_suggest(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.layout.focused_session() else { return self.hide_suggest(cx) };
        let Some(o) = self.by_key(key).filter(|o| o.agent != "ssh") else { return self.hide_suggest(cx) };
        let (codex, cwd) = (o.agent == "codex", o.cwd.clone());
        let (value, range) = {
            let c = self.composer.read(cx);
            (c.value().to_string(), c.selected_range())
        };
        if !range.is_empty() {
            return self.hide_suggest(cx);
        }
        let caret = range.end;
        let Some(trigger) = composer::trigger_at(&value, caret) else { return self.hide_suggest(cx) };
        let query = trigger.query.to_lowercase();
        let loading;
        let items = match trigger.kind {
            TriggerKind::Path => {
                let agents: Vec<SuggestItem> = self
                    .roster
                    .iter()
                    .map(|a| SuggestItem {
                        icon: SuggestIcon::Runner(a.cli.clone()),
                        label: format!("@{}", a.name),
                        description: format!("{} · {}", self.runner_label(&a.cli), t(match a.access.as_str() {
                            "read" => "reads",
                            "edit" => "edits",
                            _ => "full access",
                        })),
                        badge: "",
                        insert: format!("@{} ", a.name),
                        name: a.name.clone(),
                        rel: String::new(),
                    })
                    .collect();
                let agents = ranked(agents, &query, 8);
                let raw_query = trigger.query.clone();
                let seq = FILE_SEARCH.fetch_add(1, Ordering::SeqCst) + 1;
                let work = cx.background_spawn(async move { file_rows(&cwd, &raw_query) });
                let shown = agents.clone();
                cx.spawn(async move |this, cx| {
                    let found = work.await;
                    if FILE_SEARCH.load(Ordering::SeqCst) != seq {
                        return; // a newer search is on its way
                    }
                    let _ = this.update(cx, |app, cx| {
                        if let Some(s) = app.suggest.as_mut().filter(|s| s.kind == TriggerKind::Path) {
                            s.items = shown.iter().cloned().chain(found).collect();
                            s.loading = false;
                            s.index = s.index.min(s.items.len().saturating_sub(1));
                            cx.notify();
                        }
                    });
                })
                .detach();
                loading = true;
                agents
            }
            kind => {
                let data = self.agent_items(key, cx);
                loading = data.is_none();
                let data = data.unwrap_or_default();
                if kind == TriggerKind::Command {
                    let agent = if codex { "codex" } else { "claude" };
                    let builtin = composer::builtin_commands(agent).iter().map(|(name, description)| SuggestItem {
                        icon: SuggestIcon::None,
                        label: format!("/{name}"),
                        description: t(description).to_string(),
                        badge: "Built-in",
                        insert: format!("/{name} "),
                        name: (*name).to_string(),
                        rel: String::new(),
                    });
                    let custom = data.commands.iter().map(|m| SuggestItem {
                        icon: SuggestIcon::None,
                        label: format!("/{}", m.name),
                        description: m.description.clone(),
                        badge: composer::scope_label(&m.scope),
                        insert: format!("/{} ", m.name),
                        name: m.name.clone(),
                        rel: String::new(),
                    });
                    let skills = data.skills.iter().map(|s| skill_row(s, codex, true));
                    ranked(builtin.chain(custom).chain(skills).collect(), query.trim_start_matches("skill:"), 50)
                } else {
                    ranked(data.skills.iter().map(|s| skill_row(s, codex, false)).collect(), &query, 50)
                }
            }
        };
        let same = self.suggest.as_ref().filter(|s| s.kind == trigger.kind && s.start == trigger.start);
        let index = same.map_or(0, |s| s.index.min(items.len().saturating_sub(1)));
        let scroll = same.map_or_else(ScrollHandle::new, |s| s.scroll.clone());
        self.suggest = Some(Suggest { kind: trigger.kind, start: trigger.start, caret, items, index, loading, scroll });
        cx.notify();
    }

    pub(super) fn hide_suggest(&mut self, cx: &mut Context<Self>) {
        if self.suggest.take().is_some() {
            cx.notify();
        }
    }

    fn pick_suggest(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.suggest.take() else { return };
        let Some(it) = s.items.get(i) else { return };
        let value = self.composer.read(cx).value().to_string();
        let before = format!("{}{}", &value[..s.start.min(value.len())], it.insert);
        // The rest of the token being replaced goes too.
        let rest = value.get(s.caret..).unwrap_or("");
        let rest = rest.trim_start_matches(|c: char| !c.is_whitespace());
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let caret = before.len();
        let text = format!("{before}{rest}");
        self.composer.update(cx, |c, cx| {
            c.set_value(text, window, cx);
            c.set_selected_range(caret..caret, cx);
            c.focus(window, cx);
        });
        cx.notify();
    }

    /// Up, Down, Enter, Tab and Esc go to the list while it's open.
    pub(super) fn suggest_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.suggest.as_mut() else { return false };
        let n = s.items.len();
        match key {
            "up" | "down" if n > 0 => {
                s.index = if key == "down" { (s.index + 1) % n } else { (s.index + n - 1) % n };
                s.scroll.scroll_to_item(s.index);
            }
            "enter" | "tab" if n > 0 => {
                let i = s.index;
                self.pick_suggest(i, window, cx);
            }
            "escape" => self.suggest = None,
            _ => return false,
        }
        cx.notify();
        true
    }

    /// A Claude message's $skills, moved to where Claude Code runs them.
    pub(super) fn with_skills(&mut self, key: u64, text: &str) -> String {
        let Some(o) = self.by_key(key).filter(|o| o.agent != "codex" && text.contains('$')) else { return text.to_string() };
        let (_, cache_key) = self.items_key(o);
        if let Some(items) = self.items_cache.get(&cache_key).and_then(|c| c.1.clone()) {
            return composer::with_skills(text, &items.skills);
        }
        // The menu never opened, e.g. pasted text. Claude's are plain files, so this is quick.
        let home = o.provider.as_ref().and_then(|id| providers::homes(&self.paths).into_iter().find(|h| h.id == *id).map(|h| h.home));
        composer::with_skills(text, &composer::claude_items(&self.paths, &o.cwd, home.as_deref()).skills)
    }

    pub(super) fn render_suggest(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let s = self.suggest.as_ref()?;
        let head = match s.kind {
            TriggerKind::Path => "Agents, files and folders",
            TriggerKind::Command => "Commands",
            TriggerKind::Skill => "Skills",
        };
        let empty = match (s.kind, s.loading) {
            (TriggerKind::Path, true) => "Searching files…",
            (_, true) => "Loading skills…",
            (TriggerKind::Path, false) => "No matching files or folders.",
            (TriggerKind::Command, false) => "No matching command.",
            (TriggerKind::Skill, false) => "No skills found. Try / to browse commands.",
        };
        let rows: Vec<AnyElement> = s
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let icon: Option<AnyElement> = match &it.icon {
                    SuggestIcon::None => None,
                    SuggestIcon::File => Some(div().flex_none().text_color(c.muted).child(Icon::new(IconName::File).size(px(14.))).into_any_element()),
                    SuggestIcon::Folder => Some(div().flex_none().text_color(c.muted).child(Icon::empty().path(assets::FOLDER).size(px(14.))).into_any_element()),
                    SuggestIcon::Runner(cli) => Some(self.runner_icon(cli)),
                };
                div()
                    .id(("suggest-row", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_h(px(28.))
                    .px(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(i == s.index, |d| d.bg(c.hover))
                    .on_mouse_move(cx.listener(move |app, _, _, cx| {
                        if let Some(s) = app.suggest.as_mut().filter(|s| s.index != i) {
                            s.index = i;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        app.pick_suggest(i, window, cx);
                    }))
                    .children(icon)
                    .child(div().flex_none().max_w(relative(0.5)).truncate().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c.text).child(it.label.clone()))
                    .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(12.)).text_color(c.muted).child(it.description.clone()))
                    .when(!it.badge.is_empty(), |d| d.child(div().flex_none().px(px(5.)).rounded(px(4.)).bg(c.hover).text_size(px(11.)).text_color(c.muted).child(t(it.badge))))
                    .into_any_element()
            })
            .collect();
        Some(
            deferred(
                div()
                    .id("suggest")
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_full()
                    .mb(px(6.))
                    .occlude()
                    .flex()
                    .flex_col()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.card)
                    .shadow_md()
                    .child(div().px(px(14.)).pt(px(8.)).pb(px(4.)).text_size(px(12.)).text_color(c.muted).child(t(head)))
                    .child(
                        div()
                            .id("suggest-list")
                            .flex()
                            .flex_col()
                            .max_h(px(300.))
                            .overflow_y_scroll()
                            .track_scroll(&s.scroll)
                            .px(px(6.))
                            .pb(px(6.))
                            .when(rows.is_empty(), |d| d.child(div().px(px(8.)).py(px(6.)).text_size(px(13.)).text_color(c.muted).child(t(empty))))
                            .children(rows),
                    ),
            )
            .into_any_element(),
        )
    }

    fn runner_label(&self, cli: &str) -> String {
        match cli.strip_prefix("provider:") {
            Some(id) => self.providers.iter().find(|p| p.id == id).map_or_else(|| id.to_string(), |p| p.name.clone()),
            None if cli == "codex" => "Codex".to_string(),
            None => "Claude".to_string(),
        }
    }
}
