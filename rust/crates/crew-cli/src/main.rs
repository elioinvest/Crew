// crew-cli: the `crew-ssh` and `crew` commands in a Crew session. They forward each request to the
// running Crew over a private pipe; Crew connects, asks the user where needed, and streams the output
// back. A separate console program because the app's own window program has no console.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::exit;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

const SSH_HELP: &str = r#"crew-ssh: run commands on the SSH hosts saved in Crew.

Usage:
  crew-ssh hosts [--json]                         list the saved hosts (no credentials)
  crew-ssh exec <host> [--cwd DIR] "<command>"    run a command with the host's login shell
  crew-ssh cp <local> <host>:<remote>             upload one file
  crew-ssh cp <host>:<remote> <local>             download one file
  crew-ssh screen <host> [--lines N] [--wait S]   what's on the user's terminal on the host in Crew;
                                                  --wait first waits up to S seconds (max 120) for
                                                  new output to come and go quiet
  crew-ssh send <host> [--keys K1,K2] [--key K] [--wait S] ["<text>"]
                                                  type into that terminal, then press the keys, wait
                                                  until the output is quiet for 1s (or S seconds,
                                                  default 10) and print what appeared

Keys: one printable character, or enter, tab, shift+tab, esc, backspace, space, up, down, left,
right, home, end, page_up, page_down, insert, delete, f1-f12, ctrl+a-ctrl+z, alt+<char>. Use
--key for a literal comma. Example: crew-ssh send web1 --keys enter "sudo apt upgrade"

<host> is a saved host's name (or id). Crew holds the credentials, so never pass passwords
or keys. Commands that only read run at once; others wait until the user approves them in
Crew, so allow a long timeout. exec has no terminal and no input: use non-interactive flags,
or send for interactive programs.

Exit status: the remote command's, or 255 when Crew couldn't run it (the reason is printed
on stderr, starting with "crew-ssh:")."#;

const CREW_HELP: &str = r#"crew: hand tasks to the agents the user saved in Crew.

Usage:
  crew agents [--json]                 list the agents (name, Claude or Codex, what each may do)
  crew ask <agent> "<task>"            run an agent on a task and wait for its answer
  crew ask <agent> "<task>" --bg       start it and print its run id at once (to run several together)
  crew wait <id> [<id>...]             wait for runs and print their answers
  crew result <id>                     print a run's answer, or that it's still running
  crew runs                            list this session's runs
  crew stop <id>                       stop a run

Agents work in this folder (the current directory) and can't see this conversation: give each
a complete task, with the files and context it needs. The task can also come on stdin
(crew ask <agent> - < task.md).

One after another: crew ask Reviewer "..."   (waits)
At once:           crew ask Tester "..." --bg; crew ask Reviewer "..." --bg; crew wait r1 r2

Waiting stops after 9 minutes (exit status 3) while the run goes on: call crew wait again.
Give these commands a long timeout. Exit status: 0 done, 1 failed or stopped, 3 still running,
255 Crew couldn't do it (the reason starts with "crew:")."#;

fn die(tool: &str, message: &str) -> ! {
    eprintln!("{tool}: {message}");
    exit(255)
}

fn bridge() -> Option<(String, String)> {
    if let (Ok(pipe), Ok(token)) = (std::env::var("CREW_SSH_PIPE"), std::env::var("CREW_SSH_TOKEN")) {
        if !pipe.is_empty() && !token.is_empty() {
            return Some((pipe, token));
        }
    }
    let file = std::env::var_os("CREW_BRIDGE_FILE")?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    Some((v.get("pipe")?.as_str()?.to_string(), v.get("token")?.as_str()?.to_string()))
}

/// The pipe may be between instances for a moment when several commands start together.
fn open_pipe(pipe: &str) -> Option<std::fs::File> {
    let start = Instant::now();
    loop {
        match OpenOptions::new().read(true).write(true).open(pipe) {
            Ok(f) => return Some(f),
            Err(e) if e.raw_os_error() == Some(231) && start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
}

fn request(tool: &str, mut msg: Value, on_result: impl FnOnce(&Value)) -> ! {
    let Some((pipe, token)) = bridge() else { die(tool, "can't find Crew. Start this from a Crew session, or open Crew first.") };
    let unreachable = "can't reach Crew. Is it running? (Restart the session if Crew was restarted.)";
    let Some(mut sock) = open_pipe(&pipe) else { die(tool, unreachable) };
    msg["token"] = json!(token);
    msg["session"] = json!(std::env::var("CREW_SESSION_ID").unwrap_or_default());
    if tool == "crew" {
        msg["depth"] = json!(std::env::var("CREW_DEPTH").ok().and_then(|d| d.parse::<i64>().ok()).unwrap_or(0));
    } else {
        msg["agent"] = json!(std::env::var("CREW_AGENT").unwrap_or_default());
    }
    if writeln!(sock, "{msg}").is_err() {
        die(tool, unreachable);
    }
    let mut on_result = Some(on_result);
    let mut code = None;
    let b64 = base64::engine::general_purpose::STANDARD;
    for line in BufReader::new(&sock).lines() {
        let Ok(line) = line else { break };
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
        match text("type") {
            "out" => {
                let mut o = std::io::stdout();
                let _ = o.write_all(&b64.decode(text("d")).unwrap_or_default());
                let _ = o.flush();
            }
            "err" => {
                let _ = std::io::stderr().write_all(&b64.decode(text("d")).unwrap_or_default());
            }
            "status" => eprintln!("{tool}: {}…", text("message")),
            "result" => {
                if let Some(f) = on_result.take() {
                    f(&m);
                }
                code = Some(0);
            }
            "exit" => code = Some(m.get("code").and_then(Value::as_i64).unwrap_or(0) as i32),
            "fail" => {
                eprintln!("{tool}: {}", text("message"));
                code = Some(255);
            }
            _ => {}
        }
        // That's the last message: the pipe stays open on Crew's side until this program leaves.
        if code.is_some() {
            break;
        }
    }
    match code {
        Some(c) => exit(c),
        None => die(tool, "Crew closed the connection before the command finished."),
    }
}

/// Git Bash passes local paths as /c/Users/...; Crew needs C:\Users\...
fn local_path(p: &str) -> String {
    let b = p.as_bytes();
    let win = if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') {
        let rest = if b.len() > 2 { &p[2..] } else { "/" };
        format!("{}:{}", (b[1] as char).to_ascii_uppercase(), rest.replace('/', "\\"))
    } else {
        p.to_string()
    };
    let path = PathBuf::from(&win);
    if path.is_absolute() {
        return win;
    }
    std::env::current_dir().map(|d| d.join(&path)).unwrap_or(path).to_string_lossy().to_string()
}

/// host:path, but not a Windows drive letter (C:\...).
fn remote_side(s: &str) -> Option<(String, String)> {
    let (host, path) = s.split_once(':')?;
    if path.is_empty() || host.is_empty() || host.contains(['\\', '/']) || (host.chars().count() == 1 && host.chars().all(|c| c.is_ascii_alphabetic())) {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

fn ssh(mut args: Vec<String>) -> ! {
    const T: &str = "crew-ssh";
    let cmd = if args.is_empty() { String::new() } else { args.remove(0) };
    match cmd.as_str() {
        "" | "help" | "--help" | "-h" => {
            println!("{SSH_HELP}");
            exit(0)
        }
        "hosts" | "ls" => {
            let as_json = args.iter().any(|a| a == "--json");
            request(T, json!({"op": "hosts"}), |m| {
                let empty = Vec::new();
                let hosts = m.get("hosts").and_then(Value::as_array).unwrap_or(&empty);
                if as_json {
                    println!("{}", serde_json::to_string_pretty(hosts).unwrap_or_default());
                    return;
                }
                if hosts.is_empty() {
                    println!("No hosts are saved in Crew yet.");
                }
                for h in hosts {
                    let f = |k: &str| h.get(k).and_then(Value::as_str).unwrap_or("");
                    let user = if f("user").is_empty() { String::new() } else { format!("{}@", f("user")) };
                    let port = h.get("port").and_then(Value::as_i64).filter(|p| *p != 22).map(|p| format!(":{p}")).unwrap_or_default();
                    let group = if f("group").is_empty() { String::new() } else { format!("\t[{}]", f("group")) };
                    let notes = if f("notes").is_empty() { String::new() } else { format!("\n    notes: {}", f("notes").replace('\n', "\n           ")) };
                    println!("{}\t{user}{}{port}{group}{notes}", f("name"), f("host"));
                }
            })
        }
        "exec" | "run" => {
            if args.is_empty() {
                die(T, "which host? Usage: crew-ssh exec <host> \"<command>\"");
            }
            let host = args.remove(0);
            let mut cwd = String::new();
            while args.first().is_some_and(|a| a != "--" && a.starts_with("--")) {
                let opt = args.remove(0);
                if opt == "--cwd" {
                    if !args.is_empty() {
                        cwd = args.remove(0);
                    }
                } else if let Some(v) = opt.strip_prefix("--cwd=") {
                    cwd = v.to_string();
                } else {
                    die(T, &format!("unknown option {opt}"));
                }
            }
            if args.first().is_some_and(|a| a == "--") {
                args.remove(0);
            }
            let command = args.join(" ");
            if command.trim().is_empty() {
                die(T, "no command given. Usage: crew-ssh exec <host> \"<command>\"");
            }
            request(T, json!({"op": "exec", "host": host, "command": command, "cwd": cwd}), |_| {})
        }
        "screen" => {
            if args.is_empty() {
                die(T, "which host? Usage: crew-ssh screen <host> [--lines N] [--wait S]");
            }
            let host = args.remove(0);
            let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.);
            let lines = if opt("--lines") > 0. { opt("--lines") } else { 60. };
            request(T, json!({"op": "screen", "host": host, "lines": lines, "wait": opt("--wait")}), |_| {})
        }
        "send" | "type" => {
            if args.is_empty() {
                die(T, "which host? Usage: crew-ssh send <host> [--keys K1,K2] [--wait S] [\"<text>\"]");
            }
            let host = args.remove(0);
            let mut keys: Vec<String> = Vec::new();
            let mut wait = 0.;
            // Options first; the text is the rest. `--` before the text is accepted but not needed.
            while args.first().is_some_and(|a| ["--keys", "--key", "--wait"].contains(&a.as_str())) {
                let opt = args.remove(0);
                if args.is_empty() {
                    die(T, &format!("{opt} needs a value"));
                }
                let value = args.remove(0);
                match opt.as_str() {
                    "--keys" => keys.extend(value.split(',').filter(|k| !k.is_empty()).map(str::to_string)),
                    "--key" => keys.push(value),
                    _ => wait = value.parse().unwrap_or(0.),
                }
            }
            if args.first().is_some_and(|a| a == "--") {
                args.remove(0);
            }
            request(T, json!({"op": "send", "host": host, "text": args.join(" "), "keys": keys, "wait": wait}), |_| {})
        }
        "cp" | "copy" => {
            if args.len() != 2 {
                die(T, "usage: crew-ssh cp <local> <host>:<remote>  or  crew-ssh cp <host>:<remote> <local>");
            }
            match (remote_side(&args[0]), remote_side(&args[1])) {
                (Some(_), Some(_)) => die(T, "copy between two hosts isn't supported: copy to this machine first."),
                (None, None) => die(T, "one side must be <host>:<path>."),
                (None, Some((host, path))) => request(T, json!({"op": "cp", "direction": "up", "host": host, "remote": path, "local": local_path(&args[0])}), |_| {}),
                (Some((host, path)), None) => request(T, json!({"op": "cp", "direction": "down", "host": host, "remote": path, "local": local_path(&args[1])}), |_| {}),
            }
        }
        other => die(T, &format!("unknown command \"{other}\". Run crew-ssh --help.")),
    }
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn crew(mut args: Vec<String>) -> ! {
    const T: &str = "crew";
    let cmd = if args.is_empty() { String::new() } else { args.remove(0) };
    match cmd.as_str() {
        "" | "help" | "--help" | "-h" => {
            println!("{CREW_HELP}");
            exit(0)
        }
        "agents" => {
            let as_json = take_flag(&mut args, "--json");
            request(T, json!({"op": "agents"}), |m| {
                let empty = Vec::new();
                let agents = m.get("agents").and_then(Value::as_array).unwrap_or(&empty);
                if as_json {
                    println!("{}", serde_json::to_string_pretty(agents).unwrap_or_default());
                    return;
                }
                if agents.is_empty() {
                    println!("No agents are saved in Crew yet (Agents in the sidebar).");
                }
                for a in agents {
                    let f = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("");
                    let model = if f("model").is_empty() { String::new() } else { format!(" ({})", f("model")) };
                    let can = match f("access") {
                        "read" => "only reads",
                        "edit" => "edits and runs commands in the folder",
                        "full" => "may do anything",
                        other => other,
                    };
                    let first: String = f("instructions").lines().next().unwrap_or("").chars().take(120).collect();
                    let first = if first.is_empty() { String::new() } else { format!("\n    {first}") };
                    println!("{}\t{}{model}\t{can}{first}", f("name"), f("cli"));
                }
            })
        }
        "ask" => {
            let bg = take_flag(&mut args, "--bg") | take_flag(&mut args, "--background");
            if args.is_empty() {
                die(T, "which agent? Usage: crew ask <agent> \"<task>\"");
            }
            let agent = args.remove(0);
            let mut task = args.join(" ");
            if task == "-" || (task.is_empty() && !std::io::stdin().is_terminal()) {
                task.clear();
                let _ = std::io::stdin().read_to_string(&mut task);
            }
            if task.trim().is_empty() {
                die(T, "no task given. Usage: crew ask <agent> \"<task>\"");
            }
            let cwd = std::env::current_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
            request(T, json!({"op": "ask", "agent": agent, "task": task, "bg": bg, "cwd": cwd}), |_| {})
        }
        "wait" | "result" => {
            if args.is_empty() {
                die(T, &format!("usage: crew {cmd} <id> [<id>...]"));
            }
            request(T, json!({"op": cmd, "ids": args}), |_| {})
        }
        "stop" => {
            if args.is_empty() {
                die(T, "usage: crew stop <id>");
            }
            request(T, json!({"op": "stop", "id": args[0]}), |_| {})
        }
        "runs" => request(T, json!({"op": "runs"}), |_| {}),
        other => die(T, &format!("unknown command \"{other}\". Run crew --help.")),
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let which = if args.is_empty() { String::new() } else { args.remove(0) };
    match which.as_str() {
        "ssh" => ssh(args),
        "crew" => crew(args),
        _ => die("crew-cli", "run this through the crew-ssh or crew command."),
    }
}
