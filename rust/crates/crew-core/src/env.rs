//! The environment agents run in.

use std::collections::HashMap;
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;

const PATH_KEYS: [&str; 2] = [
    r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    r"HKCU\Environment",
];
const PATH_FRESH: Duration = Duration::from_secs(30);

static PATH_CACHE: Mutex<Option<(Instant, String)>> = Mutex::new(None);

fn hide(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn expand_percent(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn parse_registry_path(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let t = line.trim_start();
        let Some(rest) = t.get(..4).filter(|p| p.eq_ignore_ascii_case("path")).map(|_| &t[4..]) else { continue };
        let rest = rest.trim_start();
        for kind in ["REG_EXPAND_SZ", "REG_SZ"] {
            if let Some(v) = rest.strip_prefix(kind) {
                return Some(expand_percent(v.trim()));
            }
        }
    }
    None
}

fn registry_path(key: &str) -> String {
    let out = hide(Command::new("reg").args(["query", key, "/v", "Path"])).output();
    out.ok().and_then(|o| parse_registry_path(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

fn join_paths(from_registry: &[String]) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in from_registry.iter().map(String::as_str).chain([current.as_str()]).flat_map(|s| s.split(';')) {
        if part.is_empty() {
            continue;
        }
        if seen.insert(part.trim_end_matches(['\\', '/']).to_lowercase()) {
            out.push(part.to_string());
        }
    }
    out.join(";")
}

/// PATH as Windows knows it now: it only updates an app's PATH when it starts, so a tool installed
/// while Crew runs wouldn't be found. Reading the registry starts processes, so only the first read
/// waits for them; after that the last answer is used, and refreshed in the background when old.
pub fn fresh_path() -> String {
    let mut cache = PATH_CACHE.lock().unwrap();
    match cache.as_mut() {
        None => {
            let value = join_paths(&PATH_KEYS.map(registry_path));
            *cache = Some((Instant::now(), value.clone()));
            value
        }
        Some((at, value)) => {
            if at.elapsed() > PATH_FRESH {
                *at = Instant::now();
                std::thread::spawn(|| {
                    let value = join_paths(&PATH_KEYS.map(registry_path));
                    if let Some(c) = PATH_CACHE.lock().unwrap().as_mut() {
                        c.1 = value;
                    }
                });
            }
            value.clone()
        }
    }
}

/// Reads PATH again now and waits; after an install, say.
pub fn refresh_path_now() {
    let value = join_paths(&PATH_KEYS.map(registry_path));
    *PATH_CACHE.lock().unwrap() = Some((Instant::now(), value));
}

/// What agents and git run with: this process's environment, without what a parent Claude Code
/// or Electron would leak into them, and with the current PATH.
pub fn clean_env() -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.retain(|k, _| k != "CLAUDECODE" && !k.starts_with("CLAUDE_CODE_") && !k.starts_with("ELECTRON_"));
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("FORCE_COLOR".into(), "3".into());
    let key = env.keys().find(|k| k.eq_ignore_ascii_case("PATH")).cloned().unwrap_or_else(|| "PATH".into());
    env.insert(key, fresh_path());
    env
}

pub fn command_exists(cmd: &str) -> bool {
    if cmd.contains(['/', '\\']) {
        let p = std::path::Path::new(cmd);
        return p.exists() || ["exe", "cmd", "bat"].iter().any(|x| p.with_extension(x).exists());
    }
    let mut c = Command::new("where");
    c.arg(cmd).envs(clean_env());
    hide(&mut c).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Splits a line of arguments: spaces separate, and a whole "quoted" word keeps its spaces. The same
/// rule as the Electron app, so a saved setting means the same in both.
pub fn split_args(line: &str) -> Vec<String> {
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]*)"|(\S+)"#).unwrap());
    WORD.captures_iter(line)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_arguments() {
        assert_eq!(split_args(r#"--model opus --settings "C:\a b\s.json"  x"#), ["--model", "opus", "--settings", r"C:\a b\s.json", "x"]);
        assert_eq!(split_args(r#"--x="a b" 'c d'"#), [r#"--x="a"#, r#"b""#, "'c", "d'"], "as the Electron app reads it");
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn parses_registry_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    C:\\x;C:\\y\r\n\r\n";
        assert_eq!(parse_registry_path(out).as_deref(), Some(r"C:\x;C:\y"));
        assert_eq!(parse_registry_path("nothing"), None);
    }

    #[test]
    fn clean_env_drops_what_a_parent_leaks() {
        // Safe here: this test is the only one touching these variables.
        unsafe {
            std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1");
            std::env::set_var("ELECTRON_RUN_AS_NODE", "1");
        }
        let e = clean_env();
        assert!(!e.contains_key("CLAUDE_CODE_CHILD_SESSION"));
        assert!(!e.contains_key("ELECTRON_RUN_AS_NODE"));
        assert_eq!(e.get("COLORTERM").map(String::as_str), Some("truecolor"));
        assert!(e.iter().any(|(k, v)| k.eq_ignore_ascii_case("PATH") && !v.is_empty()));
    }

    #[test]
    fn the_second_path_read_does_not_wait() {
        fresh_path();
        let t = Instant::now();
        for _ in 0..50 {
            fresh_path();
        }
        assert!(t.elapsed() < Duration::from_millis(50), "{:?}", t.elapsed());
    }
}
