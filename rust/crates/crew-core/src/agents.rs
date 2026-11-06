//! How the agents' CLIs start. Crew runs them and never bundles them.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::config::{Config, Paths};
use crate::env::split_args;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// What a session needs started. A new Claude session is told its id; Codex picks its own, so a new
/// Codex session is matched to its file later by folder and start time.
pub fn agent_argv(config: &Config, paths: &Paths, agent: &str, id: &str, resume: bool, prompt: Option<&str>) -> Vec<String> {
    agent_argv_on(config, paths, agent, id, resume, prompt, &[])
}

/// The settings Claude sessions start with, for the status line that reports plan usage.
pub fn statusline_settings(paths: &Paths) -> PathBuf {
    paths.data_dir.join("crew-claude-settings-rs.json")
}

/// `pre`: arguments a custom provider adds, right after the command.
pub fn agent_argv_on(config: &Config, paths: &Paths, agent: &str, id: &str, resume: bool, prompt: Option<&str>, pre: &[String]) -> Vec<String> {
    // One line for cmd.exe.
    static LINE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\r?\n\s*").unwrap());
    let first: Vec<String> = match prompt {
        Some(p) if !resume && !p.is_empty() => vec![LINE_BREAK.replace_all(p, " ").into_owned()],
        _ => vec![],
    };
    let command = |key: &str, default: &str| {
        let c = config.str(key);
        if c.is_empty() { default.to_string() } else { c.to_string() }
    };
    if agent == "codex" {
        let extra = split_args(config.str("codexArgs"));
        let mut v = vec![command("codexCommand", "codex")];
        v.extend_from_slice(pre);
        if resume {
            v.push("resume".into());
            v.extend(extra);
            v.push(id.into());
        } else {
            v.extend(extra);
            v.extend(first);
        }
        return v;
    }
    let mut v = vec![command("claudeCommand", "claude")];
    v.extend_from_slice(pre);
    if config.bool("showUsage") {
        // The status line that reports plan usage, which crew-service sets up at startup.
        let settings = statusline_settings(paths);
        if settings.exists() {
            v.push("--settings".into());
            v.push(settings.to_string_lossy().to_string());
        }
    }
    v.extend(split_args(config.str("extraArgs")));
    if resume {
        v.push("--resume".into());
        v.push(id.into());
    } else {
        v.push("--session-id".into());
        v.push(id.into());
        v.extend(first);
    }
    v
}

const CMDLINE_VAR: &str = "CREW_CMDLINE";

/// The CLIs are usually .cmd shims on Windows, so they go through cmd.exe, which resolves PATHEXT.
/// cmd.exe needs the line exactly as written, but the pty quotes what it's given, so the line goes
/// in a variable that cmd.exe expands: a quoted program path survives, and a % in a prompt isn't
/// expanded a second time.
pub fn shell_command(argv: &[String]) -> Launch {
    let quote = |a: &String| {
        if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "&|<>^()".contains(c)) { format!("\"{a}\"") } else { a.clone() }
    };
    let line = argv.iter().map(quote).collect::<Vec<_>>().join(" ");
    Launch {
        program: std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
        args: vec!["/d".into(), "/c".into(), format!("%{CMDLINE_VAR}%")],
        env: vec![(CMDLINE_VAR.into(), line)],
    }
}

/// While working, both agents keep a status line above the prompt: Claude "✻ Cogitating… (12s ·
/// esc to interrupt)", Codex "• Working (5s • esc to interrupt)". It's gone once the turn ends.
pub fn is_busy(screen: &[String]) -> bool {
    static STATUS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^\s*[^\sA-Za-z0-9_]\s+[A-Za-z0-9_][A-Za-z0-9_-]*…\s*\(\d").unwrap());
    let text = screen[screen.len().saturating_sub(14)..].join("
");
    text.to_lowercase().contains("esc to interrupt") || STATUS.is_match(&text)
}

/// Where a session starts: its folder if that's still there, else the home folder.
pub fn work_dir(cwd: Option<&Path>, paths: &Paths) -> std::path::PathBuf {
    match cwd {
        Some(c) if c.exists() => c.to_path_buf(),
        _ => paths.home.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(json: serde_json::Value) -> (Config, Paths) {
        let d = std::env::temp_dir().join(format!("crew-agents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let paths = Paths { home: d.clone(), data_dir: d.clone(), codex_home: d.join(".codex") };
        std::fs::write(paths.config_file(), json.to_string()).unwrap();
        (Config::load(&paths), paths)
    }

    #[test]
    fn claude_new_and_resume() {
        let (c, p) = cfg(serde_json::json!({"extraArgs":"--model opus"}));
        assert_eq!(agent_argv(&c, &p, "claude", "id1", false, Some("do\n  it")), ["claude", "--model", "opus", "--session-id", "id1", "do it"]);
        assert_eq!(agent_argv(&c, &p, "claude", "id1", true, Some("ignored")), ["claude", "--model", "opus", "--resume", "id1"]);
    }

    #[test]
    fn codex_new_and_resume() {
        let (c, p) = cfg(serde_json::json!({"codexArgs":"-m gpt-5.5"}));
        assert_eq!(agent_argv(&c, &p, "codex", "id2", false, None), ["codex", "-m", "gpt-5.5"]);
        assert_eq!(agent_argv(&c, &p, "codex", "id2", true, None), ["codex", "resume", "-m", "gpt-5.5", "id2"]);
    }

    #[test]
    fn a_program_in_a_folder_with_spaces_gets_its_arguments_unchanged() {
        let d = std::env::temp_dir().join(format!("crew agents {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let shim = d.join("fake agent.cmd");
        std::fs::write(&shim, "@echo off\r\necho [%1] [%2] [%3]\r\n").unwrap();
        let l = shell_command(&[shim.to_string_lossy().to_string(), "plain".into(), "two words & 100%PATH%".into(), "a&b".into()]);
        let out = std::process::Command::new(&l.program).args(&l.args).envs(l.env.iter().map(|(k, v)| (k, v))).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(text.trim(), r#"[plain] ["two words & 100%PATH%"] ["a&b"]"#, "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn busy_comes_from_the_status_line_near_the_bottom() {
        let at_bottom = |line: &str| {
            let mut v = vec![String::new(); 30];
            v[28] = line.to_string();
            v
        };
        assert!(is_busy(&at_bottom("✻ Cogitating… (12s · esc to interrupt)")));
        assert!(is_busy(&at_bottom("• Working (5s • ESC to interrupt)")));
        assert!(is_busy(&at_bottom("  ✶ Thinking… (3s)")));
        assert!(!is_busy(&at_bottom("> ready for your prompt")));
        let mut high = vec![String::new(); 30];
        high[2] = "esc to interrupt".into();
        assert!(!is_busy(&high), "old text higher up doesn't count");
    }

    #[test]
    fn custom_command() {
        let (c, p) = cfg(serde_json::json!({"claudeCommand":"my-claude"}));
        assert_eq!(agent_argv(&c, &p, "claude", "x", true, None)[0], "my-claude");
    }
}
