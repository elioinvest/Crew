// Status line for the Claude sessions Crew starts (passed with `claude --settings`). Claude Code sends
// the session's data on stdin, including the plan's rate limits; this saves those for Crew's usage
// bar, then runs the status line the user configured, so theirs still shows.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const OWN_LINE_TIMEOUT: Duration = Duration::from_secs(5);

fn is_set(v: Option<&Value>) -> bool {
    v.is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)))
}

/// The bar never reads a half-written file, and keeps its previous numbers if this fails.
fn save_usage(file: &Path, data: &Value) {
    let Some(limits) = data.get("rate_limits") else { return };
    if !is_set(limits.get("five_hour")) && !is_set(limits.get("seven_day")) {
        return;
    }
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let tmp = file.with_extension(format!("json.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, json!({ "at": at, "rate_limits": limits }).to_string()).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn session_folder(data: &Value) -> PathBuf {
    let text = |v: Option<&Value>| v.and_then(Value::as_str).filter(|s| !s.is_empty()).map(PathBuf::from);
    text(data.pointer("/workspace/project_dir"))
        .or_else(|| text(data.pointer("/workspace/current_dir")))
        .or_else(|| text(data.get("cwd")))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// The user's own status line, from the same places Claude Code reads it (project first).
fn own_command(cwd: &Path, home: &Path) -> Option<String> {
    let files = [cwd.join(".claude").join("settings.local.json"), cwd.join(".claude").join("settings.json"), home.join(".claude").join("settings.json")];
    let found = files.iter().find_map(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        let settings: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        settings.pointer("/statusLine/command")?.as_str().filter(|c| !c.is_empty()).map(str::to_string)
    })?;
    // This hook itself must never run itself.
    (!found.contains("crew-statusline")).then_some(found)
}

/// Claude Code runs commands with Git Bash on Windows; the same here, so bash-style lines work.
fn git_bash() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .into_iter()
        .chain([PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"), PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe")])
        .find(|p| p.exists())
}

fn shell_command(command: &str, bash: Option<&Path>) -> Command {
    let mut c;
    if let Some(bash) = bash {
        c = Command::new(bash);
        c.args(["-c", command]);
    } else {
        c = Command::new("cmd");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.raw_arg(format!("/d /s /c \"{command}\""));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// What the user's own status line prints, up to the point it's given up on.
fn run_own(mut command: Command, cwd: &Path, input: &[u8], timeout: Duration) -> Vec<u8> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    if cwd.is_dir() {
        command.current_dir(cwd);
    }
    let Ok(mut child) = command.spawn() else { return Vec::new() };
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else { return Vec::new() };
    let input = input.to_vec();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = Arc::new(Mutex::new(Vec::new()));
    let (done, finished) = mpsc::channel();
    let collected = output.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut chunk) {
            if n == 0 {
                break;
            }
            collected.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
        let _ = done.send(());
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    // Something it left running may hold the pipe open: what's arrived by now is what shows.
    let _ = finished.recv_timeout(Duration::from_millis(200));
    std::mem::take(&mut *output.lock().unwrap())
}

/// crew-cli statusline <usage file>
pub fn run(args: &[String]) {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    // Not JSON: nothing to save, and the user's line still gets what came.
    let data: Value = serde_json::from_slice(&input).unwrap_or_else(|_| json!({}));
    if let Some(file) = args.first() {
        save_usage(Path::new(file), &data);
    }
    let cwd = session_folder(&data);
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    let Some(command) = own_command(&cwd, &home) else { return };
    let out = run_own(shell_command(&command, git_bash().as_deref()), &cwd, &input, OWN_LINE_TIMEOUT);
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(&out);
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("crew statusline {}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_line(folder: &Path, file: &str, command: &str) {
        let dir = folder.join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), json!({ "statusLine": { "type": "command", "command": command } }).to_string()).unwrap();
    }

    #[test]
    fn saves_only_real_limits_and_leaves_no_temp_file() {
        let dir = scratch();
        let file = dir.join("usage-claude.json");
        save_usage(&file, &json!({ "model": "x" }));
        save_usage(&file, &json!({ "rate_limits": {} }));
        assert!(!file.exists());
        save_usage(&file, &json!({ "rate_limits": { "five_hour": { "used_percentage": 16, "resets_at": 100 } } }));
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["rate_limits"]["five_hour"]["used_percentage"], 16);
        assert!(saved["at"].as_u64().unwrap() > 1_000_000_000_000, "milliseconds, as the bar reads them");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // Nothing new: the numbers stay.
        save_usage(&file, &json!({}));
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_the_session_folder_in_order() {
        assert_eq!(session_folder(&json!({ "workspace": { "project_dir": "a", "current_dir": "b" }, "cwd": "c" })), PathBuf::from("a"));
        assert_eq!(session_folder(&json!({ "workspace": { "project_dir": "", "current_dir": "b" }, "cwd": "c" })), PathBuf::from("b"));
        assert_eq!(session_folder(&json!({ "cwd": "c" })), PathBuf::from("c"));
    }

    #[test]
    fn the_project_line_wins_and_this_hook_never_runs_itself() {
        let (project, home) = (scratch(), scratch());
        assert_eq!(own_command(&project, &home), None);
        set_line(&home, "settings.json", "home line");
        assert_eq!(own_command(&project, &home).as_deref(), Some("home line"));
        set_line(&project, "settings.json", "project line");
        assert_eq!(own_command(&project, &home).as_deref(), Some("project line"));
        set_line(&project, "settings.local.json", "local line");
        assert_eq!(own_command(&project, &home).as_deref(), Some("local line"));
        set_line(&project, "settings.local.json", "\"C:/x/crew-statusline.cmd\"");
        assert_eq!(own_command(&project, &home), None);
        let _ = std::fs::remove_dir_all(&project);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn runs_the_users_line_with_the_sessions_data() {
        let dir = scratch();
        // Reads stdin and prints something from it, as real status lines do.
        let out = run_own(shell_command("more", None), &dir, b"from claude\n", OWN_LINE_TIMEOUT);
        assert!(String::from_utf8_lossy(&out).contains("from claude"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gives_up_on_a_line_that_hangs() {
        let started = Instant::now();
        let out = run_own(shell_command("echo early& ping -n 30 127.0.0.1 >nul", None), &std::env::temp_dir(), b"", Duration::from_millis(600));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(String::from_utf8_lossy(&out).contains("early"), "what it printed before still shows");
    }
}
