//! Claude Code has no command that prints plan usage, so this opens it in a hidden terminal, runs
//! /usage and reads the bars off the screen. The marker variable keeps Claude from saving a
//! transcript, so the check never shows up as a session.

use std::time::{Duration, Instant};

use crew_core::usage::{PlanUsage, parse_claude_usage_screen};
use crew_core::{Config, Paths, agents, env};
use crew_term::{Command, Palette, Terminal};

const TICK: Duration = Duration::from_millis(400);
const GIVE_UP: Duration = Duration::from_secs(35);

/// Blocks for up to [`GIVE_UP`], so it belongs off the window's thread.
pub fn claude(config: &Config, paths: &Paths) -> Option<PlanUsage> {
    let cwd = paths.data_dir.join("usage-probe");
    std::fs::create_dir_all(&cwd).ok()?;
    let command = env::split_args(config.str("claudeCommand")).into_iter().next().unwrap_or_else(|| "claude".into());
    let launch = agents::shell_command(&[command]);
    let mut environment: Vec<(String, String)> = env::clean_env().into_iter().collect();
    environment.extend(launch.env);
    environment.push(("CLAUDE_CODE_CHILD_SESSION".into(), "1".into()));
    let term = Terminal::spawn(
        &Command { program: launch.program, args: launch.args, cwd: Some(cwd), env: environment, replace_env: true },
        120,
        50,
        Palette::light(),
    )
    .ok()?;

    let started = Instant::now();
    let mut ready_at: Option<Instant> = None;
    let mut last_key = started;
    let mut step = 0; // 0 waiting for the prompt, 1 typed, 2 sent
    let mut result = None;
    while started.elapsed() < GIVE_UP && !term.exited() {
        std::thread::sleep(TICK);
        let text = term.screen_text().join("\n");
        if text.contains("trust this folder") || text.contains("Quick safety check") {
            // Only ever Crew's own empty folder. The prompt starts on "No, exit": move to "Yes" first.
            if last_key.elapsed() < Duration::from_millis(600) {
                continue;
            }
            last_key = Instant::now();
            let selected = text.split('❯').nth(1).and_then(|s| s.lines().next()).unwrap_or("").trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').trim_start();
            if selected.starts_with("Yes") {
                term.write(b"\r");
                step = 0;
                ready_at = None;
            } else {
                term.write(b"\x1b[B");
            }
        } else if step == 0 && text.contains("Claude Code v") && text.contains('❯') {
            let since = *ready_at.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_millis(1500) {
                term.write(b"/usage");
                step = 1;
                ready_at = Some(Instant::now());
            }
        } else if step == 1 && ready_at.is_some_and(|t| t.elapsed() > Duration::from_millis(700)) {
            term.write(b"\r");
            step = 2;
        } else if step == 2 {
            // Both bars, or just one once the screen has had time to settle (plans without a weekly bar).
            let from = text.rfind("Current session").unwrap_or(0);
            if let Some(u) = parse_claude_usage_screen(&text[from..]) {
                if u.windows.len() >= 2 || ready_at.is_some_and(|t| t.elapsed() > Duration::from_secs(8)) {
                    result = Some(u);
                    break;
                }
            }
        }
    }
    term.write(b"\x1b");
    std::thread::sleep(Duration::from_millis(300));
    term.kill();
    result
}
