//! Crew runs the agents' own `claude` / `codex` commands and never bundles them. If one isn't
//! installed, this says what to install and can run the official installer, then starts the session.

use crew_core::env as crew_env;

use super::*;

struct AgentInstall {
    name: &'static str,
    page: &'static str,
    /// What the installer's terminal runs, and how it's shown.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    argv: &'static [&'static str],
    display: &'static str,
    needs_npm: bool,
}

const CLAUDE: AgentInstall = AgentInstall {
    name: "Claude Code",
    page: "https://docs.anthropic.com/en/docs/claude-code/setup",
    argv: &["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "irm https://claude.ai/install.ps1 | iex"],
    display: "irm https://claude.ai/install.ps1 | iex",
    needs_npm: false,
};

const CODEX: AgentInstall = AgentInstall {
    name: "Codex",
    page: "https://github.com/openai/codex",
    argv: &["npm", "install", "-g", "@openai/codex"],
    display: "npm install -g @openai/codex",
    needs_npm: true,
};

fn install_of(agent: &str) -> &'static AgentInstall {
    if agent == "codex" { &CODEX } else { &CLAUDE }
}

/// A session that couldn't start for lack of its agent, with what it takes to start it again.
#[derive(Clone)]
pub(super) struct PendingStart {
    pub(super) agent: String,
    pub(super) id: String,
    pub(super) cwd: PathBuf,
    pub(super) title: Option<String>,
    pub(super) resume: bool,
    pub(super) provider: Option<String>,
    pub(super) prompt: Option<String>,
}

pub(super) struct MissingAgent {
    start: PendingStart,
    command: String,
    npm_missing: bool,
    note: Option<String>,
}

impl CrewApp {
    /// The command as it's set, which runs as one path even with spaces in it.
    fn agent_command(&self, agent: &str) -> String {
        let (key, plain) = if agent == "codex" { ("codexCommand", "codex") } else { ("claudeCommand", "claude") };
        Some(self.config.str(key).trim().trim_matches('"').to_string()).filter(|c| !c.is_empty()).unwrap_or_else(|| plain.to_string())
    }

    /// Whether the agent can run; if not, the screen for it comes up. Asked once per agent while it's
    /// found, as looking takes a moment.
    pub(super) fn agent_ready(&mut self, start: PendingStart, cx: &mut Context<Self>) -> bool {
        if self.agents_found.contains(&start.agent) {
            return true;
        }
        let command = self.agent_command(&start.agent);
        // It may have just been installed.
        crew_env::refresh_path_now();
        if crew_env::command_exists(&command) {
            self.agents_found.insert(start.agent);
            return true;
        }
        self.show_missing(start, None, cx);
        false
    }

    fn show_missing(&mut self, start: PendingStart, note: Option<String>, cx: &mut Context<Self>) {
        let command = self.agent_command(&start.agent);
        let npm_missing = install_of(&start.agent).needs_npm && !crew_env::command_exists("npm");
        self.missing_agent = Some(MissingAgent { start, command, npm_missing, note });
        cx.notify();
    }

    fn start_pending(&mut self, s: PendingStart, window: &mut Window, cx: &mut Context<Self>) {
        self.start(&s.agent, s.id, s.cwd, s.title, s.resume, s.provider, s.prompt.as_deref(), window, cx);
    }

    fn check_agent_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(missing) = self.missing_agent.take() else { return };
        self.start_pending(missing.start, window, cx);
        cx.notify();
    }

    fn install_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(missing) = self.missing_agent.take() else { return };
        let info = install_of(&missing.start.agent);
        #[cfg(not(debug_assertions))]
        let argv: Vec<String> = info.argv.iter().map(|a| (*a).to_string()).collect();
        // The real installer changes this computer, so debug builds, which is what tests drive, only
        // ever run a stand-in, and do nothing without one.
        #[cfg(debug_assertions)]
        let argv: Vec<String> = match std::env::var("TEST_AGENT_INSTALLER") {
            Ok(stand_in) => vec![stand_in],
            Err(_) => {
                self.show_missing(missing.start, Some("Debug builds don't run the real installer.".to_string()), cx);
                return;
            }
        };
        let launch = agents::shell_command(&argv);
        let mut environment: Vec<(String, String)> = env::clean_env().into_iter().collect();
        environment.extend(launch.env);
        let command = Command { program: launch.program, args: launch.args, cwd: Some(agents::work_dir(Some(&missing.start.cwd), &self.paths)), env: environment, replace_env: true };
        let term = match Terminal::spawn(&command, 80, 24, self.palette.clone()) {
            Ok(t) => Arc::new(t),
            Err(why) => {
                let note = format!("Couldn't run the installer: {why}");
                self.show_missing(missing.start, Some(note), cx);
                return;
            }
        };
        term.note(&format!("\x1b[90mInstalling {}: {}\x1b[0m\r\n\r\n", info.name, info.display));
        let (cwd, title) = (missing.start.cwd.clone(), format!("Installing {}", info.name));
        let key = self.attach("install", format!("install:{}", missing.start.agent), cwd, Some(title), true, term, window, cx);
        self.installing = Some((key, missing.start));
    }

    /// The installer's terminal ended: start the session, or say what happened.
    pub(super) fn installer_exited(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, start)) = self.installing.take_if(|(k, _)| *k == key) else { return };
        let command = self.agent_command(&start.agent);
        crew_env::refresh_path_now();
        if crew_env::command_exists(&command) {
            self.agents_found.insert(start.agent.clone());
            self.close_installer(key, cx);
            self.start_pending(start, window, cx);
        } else {
            if let Some(o) = self.open.iter_mut().find(|o| o.key == key) {
                o.exited = true;
            }
            let note = format!("The installer finished, but the {command} command still can't be found. Restarting Crew may help, or set the command in Settings.");
            self.show_missing(start, Some(note), cx);
        }
    }

    fn close_installer(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(i) = self.open.iter().position(|o| o.key == key) {
            self.open.remove(i).view.read(cx).term.kill();
        }
        self.layout.forget(key);
    }

    pub(super) fn render_missing_agent(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.missing_agent.as_ref()?;
        let c = self.colors;
        let info = install_of(&m.start.agent);
        let page = info.page;
        let line = |text: String| div().text_size(px(13.)).text_color(c.muted).child(text);
        let main = if m.npm_missing {
            self.button("agent-get-node", t("Get Node.js"), true).on_click(cx.listener(|_, _, _, cx| cx.open_url("https://nodejs.org/"))).into_any_element()
        } else {
            self.button("agent-install", format!("Install {}", info.name), true).on_click(cx.listener(|app, _, window, cx| app.install_agent(window, cx))).into_any_element()
        };
        Some(self.modal(
            "agent-missing",
            520.,
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).child(format!("{} isn't installed", info.name)))
                .child(line(format!("Crew runs the official {} command, and it wasn't found on this computer.", m.command)))
                .child(line(if m.npm_missing { format!("{} installs with npm, which comes with Node.js. Install Node.js, then click Check again.", info.name) } else { t("It can be installed with:").to_string() }))
                .child(div().px(px(10.)).py(px(8.)).rounded(px(6.)).bg(c.hover).text_size(px(12.5)).font_family(crate::terminal_view::FONT_FAMILY).child(info.display))
                .when_some(m.note.clone(), |d, note| d.child(div().text_size(px(13.)).text_color(gpui::rgb(0xd93d42)).child(note)))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .mt(px(4.))
                        .child(main)
                        .child(self.button("agent-guide", t("Install guide"), false).on_click(cx.listener(move |_, _, _, cx| cx.open_url(page))))
                        .child(self.button("agent-recheck", t("Check again"), false).on_click(cx.listener(|app, _, window, cx| app.check_agent_again(window, cx))))
                        .child(self.button("agent-cancel", t("Close"), false).on_click(cx.listener(|app, _, _, cx| {
                            app.missing_agent = None;
                            cx.notify();
                        }))),
                )
                .child(div().text_size(px(12.)).text_color(c.muted).child(t("You sign in once, the first time the session starts. Installed somewhere else? Set the command in Settings."))),
        ))
    }
}
