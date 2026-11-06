//! Agents the main session hands work to by @-mention, with `crew ask`. Each run is the agent's CLI in
//! its non-interactive mode in the caller's folder; its events become a live card in the window, and
//! its final answer goes back to the caller.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crew_core::roster::{self, RosterAgent};
use crew_core::{Config, agents, env, providers};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::bridge::Reply;
use crate::ssh::now;
use crate::{Event, Service, install};

const MAX_DEPTH: i64 = 2; // an agent's agent may delegate once more, no further
const WAIT_MS: u64 = 9 * 60_000; // how long ask / wait block before saying "still running"

/// What the window shows of a run.
#[derive(Clone, Debug, PartialEq)]
pub struct PublicRun {
    pub id: String,
    pub agent: String,
    pub cli: String,
    pub task: String,
    pub cwd: String,
    pub caller: String,
    pub status: String,
    pub activity: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub result: String,
    pub error: String,
    pub session_id: Option<String>,
}

struct RunState {
    public: PublicRun,
    engine: String,
    last_text: String,
    pid: Option<u32>,
}

pub(crate) struct Run {
    state: Mutex<RunState>,
    done: watch::Sender<bool>,
}

#[derive(Default)]
pub struct Runs {
    list: Mutex<HashMap<String, Arc<Run>>>,
    order: Mutex<Vec<String>>,
    seq: AtomicU64,
}

fn short(s: &str, n: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n { format!("{}…", t.chars().take(n - 1).collect::<String>()) } else { t }
}

fn base(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// What the agent is doing now, from one of its tool calls.
fn describe_tool(name: &str, input: &Value) -> String {
    let text = |k: &str| input.get(k).and_then(Value::as_str);
    let f = text("file_path").or_else(|| text("path")).or_else(|| text("notebook_path"));
    match (name, f) {
        ("Read" | "View", Some(f)) => format!("Reading {}", base(f)),
        ("Edit" | "MultiEdit" | "Write" | "NotebookEdit", Some(f)) => format!("Editing {}", base(f)),
        ("Grep" | "Glob", _) => format!("Searching {}", short(text("pattern").unwrap_or(""), 40)),
        ("Task" | "Agent", _) => "Delegating a subtask".into(),
        ("Bash", _) if text("command").is_some() => format!("Running {}", short(text("command").unwrap_or(""), 60)),
        (n, _) if n.starts_with("Web") => format!("Looking up {}", short(text("query").or_else(|| text("url")).unwrap_or(""), 50)),
        (n, _) => format!("Using {n}"),
    }
}

/// The access setting, as each CLI's own permission flags.
pub(crate) fn claude_args(a: &RosterAgent, prompt_file: Option<&str>) -> Vec<String> {
    let mode = match a.access.as_str() {
        "edit" => "auto",
        "full" => "bypassPermissions",
        _ => "dontAsk",
    };
    let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", mode].map(String::from).to_vec();
    if a.access == "read" || a.access.is_empty() {
        args.extend(["--disallowedTools", "Edit", "Write", "NotebookEdit"].map(String::from));
    }
    if !a.model.is_empty() {
        args.extend(["--model".to_string(), a.model.clone()]);
    }
    if !a.effort.is_empty() {
        args.extend(["--effort".to_string(), a.effort.clone()]);
    }
    if let Some(p) = prompt_file {
        args.extend(["--append-system-prompt-file".to_string(), p.to_string()]);
    }
    args
}

pub(crate) fn codex_args(a: &RosterAgent, cwd: &str) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "--json", "--skip-git-repo-check", "-C", cwd].map(String::from).to_vec();
    if a.access == "full" {
        args.push("--dangerously-bypass-approvals-and-sandbox".into());
    } else {
        args.extend(["--sandbox".to_string(), if a.access == "edit" { "workspace-write" } else { "read-only" }.to_string()]);
    }
    if !a.model.is_empty() {
        args.extend(["-m".to_string(), a.model.clone()]);
    }
    if !a.effort.is_empty() {
        args.extend(["-c".to_string(), format!("model_reasoning_effort={}", a.effort)]);
    }
    args.push("-".into()); // the prompt comes on stdin
    args
}

fn elapsed(r: &PublicRun) -> String {
    let end = if r.ended_at > 0 { r.ended_at } else { now() };
    let s = ((end - r.started_at).max(0) as f64 / 1000.).round() as i64;
    if s < 60 { format!("{s}s") } else { format!("{}m {}s", s / 60, s % 60) }
}

fn report(r: &PublicRun) -> String {
    let timing = if r.status == "running" { String::new() } else { format!(" · {}", elapsed(r)) };
    let head = format!("=== {} ({}) · {}{timing} ===", r.agent, r.id, r.status);
    match r.status.as_str() {
        "running" => format!("{head}\nStill running ({}). Check again with: crew wait {}", r.activity, r.id),
        "done" => format!("{head}\n{}", r.result.trim()),
        _ => format!("{head}\n{}{}", r.error, if r.result.is_empty() { String::new() } else { format!("\n{}", r.result.trim()) }),
    }
}

impl Service {
    fn notify_run(&self, run: &Run) {
        let public = run.state.lock().unwrap().public.clone();
        self.remote_on_run(&public);
        self.send(Event::Run(public));
    }

    fn finish_run(&self, run: &Run, status: &str, result: Option<String>, error: Option<String>) {
        {
            let mut s = run.state.lock().unwrap();
            if s.public.status != "running" {
                return;
            }
            s.public.status = status.to_string();
            s.public.ended_at = now();
            if let Some(r) = result {
                s.public.result = r;
            }
            if let Some(e) = error {
                s.public.error = e;
            }
            s.public.activity = match status {
                "done" => "Done".into(),
                "stopped" => "Stopped".into(),
                _ => short(if s.public.error.is_empty() { "Failed" } else { &s.public.error }, 120),
            };
        }
        let _ = run.done.send(true);
        self.notify_run(run);
    }

    /// One line of the CLI's JSON event stream.
    fn on_run_event(&self, run: &Run, line: &str) {
        let Ok(e) = serde_json::from_str::<Value>(line) else { return };
        let kind = e.get("type").and_then(Value::as_str).unwrap_or("");
        let text = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
        let mut activity = None;
        let engine = run.state.lock().unwrap().engine.clone();
        if engine == "claude" {
            if kind == "system" {
                if let Some(id) = text("session_id") {
                    run.state.lock().unwrap().public.session_id = Some(id);
                }
            }
            if kind == "assistant" {
                for c in e.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    match c.get("type").and_then(Value::as_str) {
                        Some("tool_use") => activity = Some(describe_tool(c.get("name").and_then(Value::as_str).unwrap_or(""), c.get("input").unwrap_or(&Value::Null))),
                        Some("text") => {
                            let t = c.get("text").and_then(Value::as_str).unwrap_or("");
                            if !t.trim().is_empty() {
                                run.state.lock().unwrap().last_text = t.to_string();
                                activity = Some(short(t, 90));
                            }
                        }
                        _ => {}
                    }
                }
            }
            if kind == "result" {
                if let Some(id) = text("session_id") {
                    run.state.lock().unwrap().public.session_id = Some(id);
                }
                let result = text("result").unwrap_or_default();
                let failed = e.get("is_error").and_then(Value::as_bool).unwrap_or(false) || text("subtype").as_deref() != Some("success");
                if failed {
                    let why = if result.is_empty() { text("subtype").unwrap_or_else(|| "The agent stopped with an error".into()) } else { result.clone() };
                    self.finish_run(run, "failed", Some(result), Some(short(&why, 300)));
                } else {
                    let last = run.state.lock().unwrap().last_text.clone();
                    self.finish_run(run, "done", Some(if result.is_empty() { last } else { result }), None);
                }
                return;
            }
        } else {
            if kind == "thread.started" {
                if let Some(id) = text("thread_id") {
                    run.state.lock().unwrap().public.session_id = Some(id);
                }
            }
            if let Some(item) = e.get("item").filter(|_| kind == "item.started" || kind == "item.completed") {
                let field = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("");
                match field("type") {
                    "command_execution" => activity = Some(format!("Running {}", short(field("command"), 60))),
                    "file_change" => {
                        let names: Vec<String> = item.get("changes").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.get("path").and_then(Value::as_str)).map(base).filter(|n| !n.is_empty()).take(3).collect();
                        activity = Some(format!("Editing {}", if names.is_empty() { "files".to_string() } else { names.join(", ") }));
                    }
                    "web_search" => activity = Some(format!("Looking up {}", short(field("query"), 50))),
                    "agent_message" if kind == "item.completed" && !field("text").is_empty() => {
                        run.state.lock().unwrap().last_text = field("text").to_string();
                        activity = Some(short(field("text"), 90));
                    }
                    "reasoning" => activity = Some("Thinking…".into()),
                    _ => {}
                }
            }
            if kind == "turn.failed" || kind == "error" {
                let why = e.pointer("/error/message").and_then(Value::as_str).map(str::to_string).or_else(|| text("message")).unwrap_or_else(|| "Codex stopped with an error".into());
                self.finish_run(run, "failed", None, Some(short(&why, 300)));
                return;
            }
            if kind == "turn.completed" {
                let last = run.state.lock().unwrap().last_text.clone();
                self.finish_run(run, "done", Some(last), None);
                return;
            }
        }
        if let Some(a) = activity {
            let changed = {
                let mut s = run.state.lock().unwrap();
                let changed = s.public.activity != a;
                s.public.activity = a;
                changed
            };
            if changed {
                self.notify_run(run);
            }
        }
    }

    fn start_run(&self, agent: &RosterAgent, task: &str, cwd: &str, caller: &str, depth: i64) -> Arc<Run> {
        let id = format!("r{}", self.0.runs.seq.fetch_add(1, Ordering::Relaxed) + 1);
        let (done, _) = watch::channel(false);
        let run = Arc::new(Run {
            state: Mutex::new(RunState {
                public: PublicRun { id: id.clone(), agent: agent.name.clone(), cli: agent.cli.clone(), task: task.to_string(), cwd: cwd.to_string(), caller: caller.to_string(), status: "running".into(), activity: "Starting…".into(), started_at: now(), ended_at: 0, result: String::new(), error: String::new(), session_id: None },
                engine: agent.cli.clone(),
                last_text: String::new(),
                pid: None,
            }),
            done,
        });
        self.0.runs.list.lock().unwrap().insert(id.clone(), run.clone());
        self.0.runs.order.lock().unwrap().push(id.clone());

        let paths = self.paths().clone();
        let config = Config::load(&paths);
        let mut environment = install::session_env(&paths, &self.0.bridge, env::clean_env(), "", &agent.cli);
        environment.insert("CREW_DEPTH".into(), (depth + 1).to_string());
        environment.insert("CREW_RUN_ID".into(), id.clone());
        // A custom provider's agent runs on the CLI that speaks its protocol, in its own home.
        let mut engine = agent.cli.clone();
        let mut pre = Vec::new();
        let mut opts = agent.clone();
        if let Some(provider) = agent.cli.strip_prefix("provider:") {
            let Some(l) = providers::launch(&paths, provider, Some(&agent.model), Some(&agent.effort)) else {
                self.finish_run(&run, "failed", None, Some("Its model provider no longer exists in Crew.".into()));
                return run;
            };
            engine = l.cli.clone();
            pre = l.args;
            providers::apply_env(&mut environment, &l.env);
            opts.model.clear();
            opts.effort.clear();
        }
        run.state.lock().unwrap().engine = engine.clone();
        let command = |key: &str, default: &str| {
            let c = config.str(key).trim();
            if c.is_empty() { default.to_string() } else { c.to_string() }
        };
        let mut prompt_file = None;
        let (argv, input) = if engine == "codex" {
            install::write_codex_rules(&paths, true); // lets crew-ssh and crew out of Codex's sandbox
            let mut argv = vec![command("codexCommand", "codex")];
            argv.extend(pre);
            argv.extend(codex_args(&opts, cwd));
            let instructions = agent.instructions.trim();
            (argv, if instructions.is_empty() { task.to_string() } else { format!("{instructions}\n\n---\n\nYour task:\n{task}") })
        } else {
            let instructions = agent.instructions.trim();
            if !instructions.is_empty() {
                let f = std::env::temp_dir().join(format!("crew-agent-{id}-{}.md", std::process::id()));
                let text = format!("You are {}, an agent another agent handed a task to through Crew. Do the task, then end with your answer: it's returned to them as is.\n\n{instructions}\n", agent.name);
                if std::fs::write(&f, text).is_ok() {
                    prompt_file = Some(f);
                }
            }
            let mut argv = vec![command("claudeCommand", "claude")];
            argv.extend(pre);
            argv.extend(claude_args(&opts, prompt_file.as_ref().and_then(|p| p.to_str())));
            (argv, task.to_string())
        };
        let launch = agents::shell_command(&argv);
        environment.extend(launch.env);
        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.args).current_dir(cwd).env_clear().envs(&environment).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.finish_run(&run, "failed", None, Some(e.to_string()));
                return run;
            }
        };
        run.state.lock().unwrap().pid = Some(child.id());
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                let _ = stdin.write_all(input.as_bytes());
            });
        }
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(mut err) = child.stderr.take() {
            let tail = stderr.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = err.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut t = tail.lock().unwrap();
                    t.push_str(&String::from_utf8_lossy(&buf[..n]));
                    let extra = t.len().saturating_sub(4000);
                    if extra > 0 {
                        let cut = (extra..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(0);
                        t.drain(..cut);
                    }
                }
            });
        }
        let stdout = child.stdout.take();
        let (service, watched, label) = (self.clone(), run.clone(), agent.cli.clone());
        std::thread::spawn(move || {
            if let Some(out) = stdout {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let line = line.trim();
                    if !line.is_empty() {
                        service.on_run_event(&watched, line);
                    }
                }
            }
            let code = child.wait().ok().and_then(|s| s.code());
            if let Some(f) = prompt_file {
                let _ = std::fs::remove_file(f);
            }
            let (result, last, running) = {
                let s = watched.state.lock().unwrap();
                (s.public.result.clone(), s.last_text.clone(), s.public.status == "running")
            };
            if !running {
                return;
            }
            if code == Some(0) && (!result.is_empty() || !last.is_empty()) {
                service.finish_run(&watched, "done", Some(if result.is_empty() { last } else { result }), None);
            } else {
                let tail = stderr.lock().unwrap().clone();
                let why = tail.lines().filter(|l| !l.trim().is_empty()).next_back().map(str::to_string).unwrap_or_else(|| format!("{label} exited with code {}", code.unwrap_or(-1)));
                service.finish_run(&watched, "failed", None, Some(why));
            }
        });
        self.notify_run(&run);
        run
    }

    pub fn stop_run(&self, id: &str) {
        let run = self.0.runs.list.lock().unwrap().get(id).cloned();
        if let Some(run) = run {
            self.stop(&run);
        }
    }

    fn stop(&self, run: &Run) {
        let (running, pid) = {
            let s = run.state.lock().unwrap();
            (s.public.status == "running", s.pid)
        };
        if !running {
            return;
        }
        // Said first: the process going away shouldn't read as a failure.
        self.finish_run(run, "stopped", None, Some("Stopped by the user".into()));
        // The CLI runs under cmd.exe: stop the whole tree, not just the shell.
        if let Some(pid) = pid {
            let mut kill = Command::new("taskkill");
            kill.args(["/pid", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                kill.creation_flags(0x0800_0000);
            }
            let _ = kill.status();
        }
    }

    pub fn stop_all_runs(&self) {
        let all: Vec<Arc<Run>> = self.0.runs.list.lock().unwrap().values().cloned().collect();
        for r in all {
            self.stop(&r);
        }
    }

    pub fn list_runs(&self) -> Vec<PublicRun> {
        let order = self.0.runs.order.lock().unwrap().clone();
        let list = self.0.runs.list.lock().unwrap();
        order.iter().filter_map(|id| list.get(id)).map(|r| r.state.lock().unwrap().public.clone()).collect()
    }

    async fn wait_for(run: &Run, ms: u64) -> bool {
        let mut rx = run.done.subscribe();
        tokio::time::timeout(Duration::from_millis(ms), rx.wait_for(|d| *d)).await.is_ok_and(|r| r.is_ok())
    }

    /// The `crew` command's requests. False when `op` isn't one of them.
    pub(crate) async fn run_agent_op(&self, op: &str, m: &Value, reply: &Reply) -> bool {
        let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let timeout = m.get("timeout").and_then(Value::as_u64).filter(|t| *t > 0).unwrap_or(WAIT_MS).min(WAIT_MS);
        match op {
            "agents" => {
                let list: Vec<Value> = roster::list(self.paths()).iter().map(|a| json!({"name": a.name, "cli": a.cli, "model": a.model, "effort": a.effort, "access": if a.access.is_empty() { "read" } else { &a.access }, "instructions": a.instructions})).collect();
                reply.send(json!({"type": "result", "agents": list})).await;
            }
            "ask" => {
                let wanted = text("agent");
                let Some(a) = roster::find(self.paths(), &wanted) else {
                    let names = roster::list(self.paths()).iter().map(|x| x.name.clone()).collect::<Vec<_>>().join(", ");
                    let names = if names.is_empty() { "none yet (add them under Agents in Crew)".to_string() } else { names };
                    reply.fail(&format!("No agent named \"{}\". Agents: {names}.", roster::clean_name(&wanted))).await;
                    return true;
                };
                let depth = m.get("depth").and_then(Value::as_i64).unwrap_or(0);
                if depth >= MAX_DEPTH {
                    reply.fail(&format!("Delegation is nested too deep ({depth} levels). Do this task yourself.")).await;
                    return true;
                }
                let (task, cwd) = (text("task"), text("cwd"));
                if task.trim().is_empty() {
                    reply.fail("No task given.").await;
                    return true;
                }
                if cwd.is_empty() || !Path::new(&cwd).exists() {
                    reply.fail("The working folder was not found.").await;
                    return true;
                }
                let run = self.start_run(&a, &task, &cwd, &text("session"), depth);
                let id = run.state.lock().unwrap().public.id.clone();
                reply.status(&format!("started {} as {id}", a.name)).await;
                if m.get("bg").and_then(Value::as_bool).unwrap_or(false) {
                    reply.out(format!("{id}\n").as_bytes()).await;
                    reply.exit(0).await;
                    return true;
                }
                let finished = Self::wait_for(&run, timeout).await;
                let public = run.state.lock().unwrap().public.clone();
                reply.out(format!("{}\n", report(&public)).as_bytes()).await;
                reply.exit(if !finished { 3 } else if public.status == "done" { 0 } else { 1 }).await;
            }
            "wait" | "result" => {
                let timeout = if op == "result" { 1 } else { timeout };
                let ids: Vec<String> = m.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
                let list: Vec<Option<Arc<Run>>> = {
                    let all = self.0.runs.list.lock().unwrap();
                    ids.iter().map(|id| all.get(id).cloned()).collect()
                };
                if list.is_empty() || list.iter().any(Option::is_none) {
                    let known = self.0.runs.order.lock().unwrap().join(", ");
                    reply.fail(&format!("Unknown run id. Runs: {}.", if known.is_empty() { "none" } else { &known })).await;
                    return true;
                }
                let start = std::time::Instant::now();
                for r in list.iter().flatten() {
                    Self::wait_for(r, timeout.saturating_sub(start.elapsed().as_millis() as u64)).await;
                }
                let publics: Vec<PublicRun> = list.iter().flatten().map(|r| r.state.lock().unwrap().public.clone()).collect();
                let body = publics.iter().map(report).collect::<Vec<_>>().join("\n\n");
                reply.out(format!("{body}\n").as_bytes()).await;
                let code = if publics.iter().any(|r| r.status == "running") { 3 } else if publics.iter().all(|r| r.status == "done") { 0 } else { 1 };
                reply.exit(code).await;
            }
            "stop" => {
                let id = text("id");
                let run = self.0.runs.list.lock().unwrap().get(&id).cloned();
                let Some(run) = run else {
                    reply.fail(&format!("Unknown run id {id}.")).await;
                    return true;
                };
                self.stop(&run);
                let public = run.state.lock().unwrap().public.clone();
                reply.out(format!("{}\n", report(&public)).as_bytes()).await;
                reply.exit(0).await;
            }
            "runs" => {
                let session = text("session");
                let mine: Vec<String> = self
                    .list_runs()
                    .iter()
                    .filter(|r| session.is_empty() || r.caller == session)
                    .map(|r| format!("{}\t{}\t{}\t{}\t{}", r.id, r.agent, r.status, elapsed(r), short(&r.task, 60)))
                    .collect();
                let body = if mine.is_empty() { "No agent runs yet.".to_string() } else { mine.join("\n") };
                reply.out(format!("{body}\n").as_bytes()).await;
                reply.exit(0).await;
            }
            _ => return false,
        }
        true
    }

    /// Codex's models for this account, from its app-server. Blocks; None when it can't say.
    pub fn codex_models(&self) -> Option<Vec<CodexModel>> {
        let config = Config::load(self.paths());
        let c = config.str("codexCommand");
        let launch = agents::shell_command(&[if c.is_empty() { "codex".to_string() } else { c.to_string() }, "app-server".to_string()]);
        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.args).env_clear().envs(env::clean_env()).envs(launch.env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().ok()?;
        let pid = child.id();
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let _ = writeln!(stdin, "{}", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"crew","title":"Crew","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}}));
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut out = None;
        while let Ok(line) = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
            match m.get("id").and_then(Value::as_i64) {
                Some(1) => {
                    let _ = writeln!(stdin, "{}", json!({"method":"initialized"}));
                    let _ = writeln!(stdin, "{}", json!({"id":2,"method":"model/list","params":{}}));
                }
                Some(2) => {
                    let list: Vec<CodexModel> = m
                        .pointer("/result/data")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|x| !x.get("hidden").and_then(Value::as_bool).unwrap_or(false))
                        .filter_map(|x| {
                            let id = x.get("id").and_then(Value::as_str)?;
                            Some(CodexModel {
                                value: x.get("model").and_then(Value::as_str).unwrap_or(id).to_string(),
                                label: x.get("displayName").and_then(Value::as_str).unwrap_or(id).to_string(),
                                is_default: x.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
                                efforts: x.get("supportedReasoningEfforts").and_then(Value::as_array).into_iter().flatten().filter_map(|e| e.get("reasoningEffort").and_then(Value::as_str)).map(str::to_string).collect(),
                                default_effort: x.get("defaultReasoningEffort").and_then(Value::as_str).unwrap_or("").to_string(),
                            })
                        })
                        .collect();
                    if !list.is_empty() {
                        out = Some(list);
                    }
                    break;
                }
                _ => {}
            }
        }
        drop(stdin);
        let mut kill = Command::new("taskkill");
        kill.args(["/pid", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            kill.creation_flags(0x0800_0000);
        }
        let _ = kill.status();
        let _ = child.wait();
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodexModel {
    pub value: String,
    pub label: String,
    pub is_default: bool,
    pub efforts: Vec<String>,
    pub default_effort: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_becomes_each_clis_flags() {
        let read = RosterAgent { access: "read".into(), model: "opus".into(), ..Default::default() };
        let args = claude_args(&read, Some("p.md"));
        assert!(args.windows(2).any(|w| w == ["--permission-mode", "dontAsk"]));
        assert!(args.contains(&"--disallowedTools".to_string()) && args.contains(&"--append-system-prompt-file".to_string()));
        let full = RosterAgent { access: "full".into(), ..Default::default() };
        assert!(claude_args(&full, None).windows(2).any(|w| w == ["--permission-mode", "bypassPermissions"]));
        assert!(codex_args(&full, "C:/x").contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        let edit = RosterAgent { access: "edit".into(), effort: "high".into(), ..Default::default() };
        let args = codex_args(&edit, "C:/x");
        assert!(args.windows(2).any(|w| w == ["--sandbox", "workspace-write"]));
        assert!(args.contains(&"model_reasoning_effort=high".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn says_what_a_tool_is_doing() {
        assert_eq!(describe_tool("Read", &json!({"file_path":"C:/a/b.rs"})), "Reading b.rs");
        assert_eq!(describe_tool("Bash", &json!({"command":"ls   -la"})), "Running ls -la");
        assert_eq!(describe_tool("Other", &json!({})), "Using Other");
    }

    #[test]
    fn a_report_says_how_it_went() {
        let mut r = PublicRun { id: "r1".into(), agent: "A".into(), cli: "claude".into(), task: "t".into(), cwd: "c".into(), caller: "".into(), status: "running".into(), activity: "Thinking…".into(), started_at: now(), ended_at: 0, result: "".into(), error: "".into(), session_id: None };
        assert!(report(&r).contains("crew wait r1"));
        r.status = "done".into();
        r.ended_at = r.started_at + 65_000;
        r.result = " answer ".into();
        assert!(report(&r).contains("· 1m 5s") && report(&r).ends_with("answer"));
    }
}
