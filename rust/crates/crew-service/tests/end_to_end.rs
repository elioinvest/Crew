//! The whole path an agent's command takes: the real crew-cli, the pipe, approvals, and SSH to a real
//! (in-process) server: password sign-in, host keys, exec, the shared terminal and file copies.
//! Build crew-cli first: cargo build -p crew-cli

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crew_core::Paths;
use crew_core::hosts::{self, HostInput};
use crew_core::roster::{self, RosterAgent};
use crew_service::{Answer, Event, PromptKind, Service};
use crew_term::Palette;
use russh::keys::decode_secret_key;
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{Data, FileAttributes, Handle, OpenFlags, Status, StatusCode, Version};

// ---------- the server ----------

#[derive(Clone, Default)]
/// The crew-cli these tests drive, built once if need be: `cargo test` only builds a package's own programs.
fn crew_cli() -> std::path::PathBuf {
    static BUILT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let mut build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            build.args(["build", "-p", "crew-cli"]).current_dir(env!("CARGO_MANIFEST_DIR"));
            if !cfg!(debug_assertions) {
                build.arg("--release");
            }
            let built = build.output().expect("couldn't run cargo");
            assert!(built.status.success(), "couldn't build crew-cli: {}", String::from_utf8_lossy(&built.stderr));
            std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("crew-cli.exe")
        })
        .clone()
}

struct Disk(Arc<Mutex<HashMap<String, Vec<u8>>>>);

struct Sftp(Disk);

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;
    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }
    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, StatusCode> {
        Ok(Version::new())
    }
    async fn open(&mut self, id: u32, filename: String, flags: OpenFlags, _: FileAttributes) -> Result<Handle, StatusCode> {
        let mut disk = (self.0).0.lock().unwrap();
        if flags.contains(OpenFlags::CREATE) || flags.contains(OpenFlags::WRITE) {
            disk.insert(filename.clone(), Vec::new());
        } else if !disk.contains_key(&filename) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Handle { id, handle: filename })
    }
    async fn close(&mut self, id: u32, _: String) -> Result<Status, StatusCode> {
        Ok(Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() })
    }
    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, StatusCode> {
        let disk = (self.0).0.lock().unwrap();
        let bytes = disk.get(&handle).ok_or(StatusCode::NoSuchFile)?;
        let start = offset as usize;
        if start >= bytes.len() {
            return Err(StatusCode::Eof);
        }
        Ok(Data { id, data: bytes[start..(start + len as usize).min(bytes.len())].to_vec() })
    }
    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, StatusCode> {
        let mut disk = (self.0).0.lock().unwrap();
        let bytes = disk.entry(handle).or_default();
        let end = offset as usize + data.len();
        if bytes.len() < end {
            bytes.resize(end, 0);
        }
        bytes[offset as usize..end].copy_from_slice(&data);
        Ok(Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() })
    }
}

struct Handler {
    disk: Disk,
    channels: Arc<tokio::sync::Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    line: String,
    shells: Vec<ChannelId>,
}

impl russh::server::Handler for Handler {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "tester" && password == "right password" { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(&mut self, channel: Channel<Msg>, reply: ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let command = String::from_utf8_lossy(data).to_string();
        let code = command.strip_prefix("exit ").and_then(|c| c.parse().ok()).unwrap_or(0);
        session.data(channel, format!("ran: {command}\n").into_bytes())?;
        session.extended_data(channel, 1, b"to stderr\n".to_vec())?;
        session.exit_status_request(channel, code)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn pty_request(&mut self, channel: ChannelId, _: &str, _: u32, _: u32, _: u32, _: u32, _: &[(russh::Pty, u32)], session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        self.shells.push(channel);
        session.data(channel, b"welcome\r\ntester$ ".to_vec())?;
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        if !self.shells.contains(&channel) {
            return Ok(()); // an SFTP channel's bytes go to its stream
        }
        for b in data {
            if *b == b'\r' {
                let reply = if self.line == "sudo x" { "\r\n[sudo] password for tester: ".to_string() } else { format!("\r\nyou ran [{}]\r\ntester$ ", self.line) };
                self.line.clear();
                session.data(channel, reply.into_bytes())?;
            } else if *b == 3 {
                self.line.clear();
                session.data(channel, b"^C\r\ntester$ ".to_vec())?;
            } else {
                self.line.push(*b as char);
                session.data(channel, vec![*b])?;
            }
        }
        Ok(())
    }

    async fn subsystem_request(&mut self, channel: ChannelId, name: &str, session: &mut Session) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel)?;
            return Ok(());
        }
        let opened = self.channels.lock().await.remove(&channel).unwrap();
        session.channel_success(channel)?;
        russh_sftp::server::run(opened.into_stream(), Sftp(self.disk.clone())).await;
        Ok(())
    }
}

/// A server on a free port. Returns the port.
fn start_server(rt: &tokio::runtime::Runtime, disk: Disk) -> u16 {
    // A throwaway key made for this test.
    let key = decode_secret_key(include_str!("test_host_key"), None).unwrap();
    let config = Arc::new(russh::server::Config { auth_rejection_time: Duration::from_millis(10), auth_rejection_time_initial: Some(Duration::ZERO), keys: vec![key], ..Default::default() });
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    rt.spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let handler = Handler { disk: disk.clone(), channels: Default::default(), line: String::new(), shells: Vec::new() };
            let config = config.clone();
            tokio::spawn(async move {
                if let Ok(session) = russh::server::run_stream(config, stream, handler).await {
                    let _ = session.await;
                }
            });
        }
    });
    port
}

// ---------- the client side ----------

type Policy = Arc<Mutex<Box<dyn FnMut(&PromptKind) -> Option<Answer> + Send>>>;

struct World {
    service: Service,
    paths: Paths,
    env: HashMap<String, String>,
    policy: Policy,
    asked: Arc<Mutex<Vec<PromptKind>>>,
    dir: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("crew-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths { home: dir.clone(), data_dir: dir.clone(), codex_home: dir.join(".codex") };
        let (service, rx) = Service::start(paths.clone());
        let env = service.session_env(HashMap::new(), "session-1", "claude");
        let policy: Policy = Arc::new(Mutex::new(Box::new(|_| Some(Answer::Trust))));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (s, p, a) = (service.clone(), policy.clone(), asked.clone());
        std::thread::spawn(move || {
            while let Ok(event) = rx.recv_blocking() {
                if let Event::Prompt(prompt) = event {
                    a.lock().unwrap().push(prompt.kind.clone());
                    let answer = (p.lock().unwrap())(&prompt.kind);
                    s.answer(prompt.id, answer);
                }
            }
        });
        Self { service, paths, env, policy, asked, dir }
    }

    fn answer_with(&self, f: impl FnMut(&PromptKind) -> Option<Answer> + Send + 'static) {
        *self.policy.lock().unwrap() = Box::new(f);
    }

    fn asked(&self) -> Vec<PromptKind> {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }

    fn cli(&self, tool: &str, args: &[&str]) -> Output {
        self.cli_env(tool, args, &[])
    }

    fn cli_env(&self, tool: &str, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let exe = crew_cli();
        Command::new(exe).arg(tool).args(args).env("CREW_SSH_PIPE", &self.env["CREW_SSH_PIPE"]).env("CREW_SSH_TOKEN", &self.env["CREW_SSH_TOKEN"]).env("CREW_SESSION_ID", "session-1").envs(extra.iter().copied()).current_dir(&self.dir).stdin(Stdio::null()).output().unwrap()
    }

    fn add_host(&self, name: &str, port: u16, password: &str, approval: &str) -> String {
        let input = HostInput { name: name.into(), host: "127.0.0.1".into(), port: port.to_string(), user: "tester".into(), auth: "password".into(), password: password.into(), approval: approval.into(), ..HostInput::default() };
        hosts::save(&self.paths, &input).unwrap().id
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn server_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

// ---------- what agents do ----------

#[test]
fn hosts_never_show_secrets_and_a_bad_token_is_refused() {
    let w = World::new();
    w.add_host("web", 22, "very secret", "changes");
    let listed = w.cli("ssh", &["hosts", "--json"]);
    assert!(listed.status.success(), "{}", err(&listed));
    assert!(out(&listed).contains("\"web\""));
    assert!(!out(&listed).contains("secret"), "{}", out(&listed));
    let plain = w.cli("ssh", &["hosts"]);
    assert!(out(&plain).contains("web\ttester@127.0.0.1"));
    let sealed = std::fs::read_to_string(w.dir.join("hosts.json")).unwrap();
    assert!(!sealed.contains("very secret"), "the password is sealed on disk");
    let refused = w.cli_env("ssh", &["hosts"], &[("CREW_SSH_TOKEN", "wrong")]);
    assert_eq!(refused.status.code(), Some(255));
    assert!(err(&refused).contains("Not authorized"));
}

#[test]
fn exec_trusts_the_key_once_and_asks_only_for_changes() {
    let rt = server_rt();
    let port = start_server(&rt, Disk::default());
    let w = World::new();
    w.add_host("web", port, "right password", "changes");

    let first = w.cli("ssh", &["exec", "web", "uptime"]);
    assert_eq!(first.status.code(), Some(0), "{}", err(&first));
    assert!(out(&first).contains("ran: uptime"));
    assert!(err(&first).contains("to stderr"));
    let asked = w.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(matches!(&asked[0], PromptKind::HostKey { fingerprint, .. } if fingerprint.starts_with("SHA256:")));

    let again = w.cli("ssh", &["exec", "web", "--cwd", "/srv", "df -h"]);
    assert!(out(&again).contains("ran: cd '/srv' && df -h"));
    assert!(w.asked().is_empty(), "the key is trusted now and this only reads");

    let code = w.cli("ssh", &["exec", "web", "ls; true"]);
    assert_eq!(code.status.code(), Some(0));

    w.answer_with(|_| None);
    let denied = w.cli("ssh", &["exec", "web", "rm -rf /tmp/x"]);
    assert_eq!(denied.status.code(), Some(255));
    assert!(err(&denied).contains("denied"), "{}", err(&denied));
    assert!(err(&denied).contains("waiting for you to approve"));
    let asked = w.asked();
    assert!(matches!(&asked[0], PromptKind::Approve { command, session, .. } if command == "rm -rf /tmp/x" && session == "session-1"), "{asked:?}");

    w.answer_with(|_| Some(Answer::Once));
    let approved = w.cli("ssh", &["exec", "web", "exit 7"]);
    assert_eq!(approved.status.code(), Some(7), "the remote exit status comes through");
    assert_eq!(w.asked().len(), 1);

    w.answer_with(|_| Some(Answer::AllowHost));
    w.cli("ssh", &["exec", "web", "touch a"]);
    assert_eq!(w.asked().len(), 1);
    w.answer_with(|_| None);
    let allowed = w.cli("ssh", &["exec", "web", "touch b"]);
    assert_eq!(allowed.status.code(), Some(0), "allowed for 15 minutes");
    assert!(w.asked().is_empty());

    let id = hosts::find(&w.paths, "web").unwrap().id;
    let log = w.service.activity(&id);
    assert_eq!(log[0].command, "touch b");
    assert!(log.iter().any(|a| a.decision == "denied" && a.command == "rm -rf /tmp/x"));
    assert!(log.iter().any(|a| a.code == Some(7) && a.decision == "approved"));
    assert_eq!(w.service.test_host("web").unwrap().contains("ran: echo"), true);
}

#[test]
fn a_wrong_password_and_a_changed_key_stop_the_connection() {
    let rt = server_rt();
    let port = start_server(&rt, Disk::default());
    let w = World::new();
    w.add_host("bad", port, "wrong", "auto");
    let refused = w.cli("ssh", &["exec", "bad", "uptime"]);
    assert_eq!(refused.status.code(), Some(255));
    assert!(err(&refused).contains("Couldn't sign in to bad"), "{}", err(&refused));

    let other_port = start_server(&rt, Disk::default());
    let id = format!("127.0.0.1:{other_port}");
    let known = serde_json::json!({ &id: { "type": "ssh-ed25519", "fingerprint": "SHA256:someoneelse", "at": 1 } });
    std::fs::write(w.dir.join("ssh-known-hosts.json"), known.to_string()).unwrap();
    w.add_host("moved", other_port, "right password", "auto");
    w.asked();
    w.answer_with(|_| None);
    let blocked = w.cli("ssh", &["exec", "moved", "uptime"]);
    assert_eq!(blocked.status.code(), Some(255));
    assert!(err(&blocked).contains("host key wasn't trusted"), "{}", err(&blocked));
    assert!(matches!(&w.asked()[0], PromptKind::HostKeyChanged { previous, .. } if previous == "SHA256:someoneelse"));
    let text = std::fs::read_to_string(w.dir.join("ssh-known-hosts.json")).unwrap();
    assert!(text.contains("someoneelse"), "a refused key isn't saved");
}

#[test]
fn a_password_asked_for_is_remembered_when_chosen() {
    let rt = server_rt();
    let port = start_server(&rt, Disk::default());
    let w = World::new();
    w.add_host("web", port, "", "auto");
    w.answer_with(|k| match k {
        PromptKind::Password { .. } => Some(Answer::Secret { value: "right password".into(), remember: true }),
        _ => Some(Answer::Trust),
    });
    assert_eq!(w.cli("ssh", &["exec", "web", "uptime"]).status.code(), Some(0));
    assert!(w.asked().iter().any(|k| matches!(k, PromptKind::Password { .. })));
    assert!(hosts::find(&w.paths, "web").unwrap().has_password());
}

#[test]
fn the_shared_terminal_can_be_read_and_typed_into() {
    let rt = server_rt();
    let port = start_server(&rt, Disk::default());
    let w = World::new();
    let id = w.add_host("web", port, "right password", "changes");

    let none = w.cli("ssh", &["screen", "web"]);
    assert!(err(&none).contains("No terminal on web"), "{}", err(&none));

    let term = w.service.open_terminal(&id, Palette::light()).unwrap();
    for _ in 0..200 {
        if w.service.terminal_of(&id).is_some() && term.lines_from(0).iter().any(|l| l.contains("tester$")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(w.service.is_connected(&id));

    let screen = w.cli("ssh", &["screen", "web", "--lines", "5"]);
    assert_eq!(screen.status.code(), Some(0), "{}", err(&screen));
    assert!(out(&screen).contains("welcome"), "{}", out(&screen));

    let sent = w.cli("ssh", &["send", "web", "--keys", "enter", "--wait", "5", "df -h"]);
    assert_eq!(sent.status.code(), Some(0), "{}", err(&sent));
    assert!(out(&sent).contains("you ran [df -h]"), "{}", out(&sent));
    assert!(w.asked().iter().all(|k| !matches!(k, PromptKind::Approve { .. })), "a typed read runs at once");

    w.answer_with(|_| None);
    let denied = w.cli("ssh", &["send", "web", "--keys", "enter", "rm -rf x"]);
    assert_eq!(denied.status.code(), Some(255));
    assert!(err(&denied).contains("denied"));
    assert!(w.asked().iter().any(|k| matches!(k, PromptKind::Approve { action, command, .. } if action == "input" && command == "rm -rf x [enter]")));

    w.answer_with(|_| Some(Answer::Once));
    w.cli("ssh", &["send", "web", "--keys", "enter", "--wait", "3", "sudo x"]);
    let secret = w.cli("ssh", &["send", "web", "hunter2"]);
    assert_eq!(secret.status.code(), Some(255));
    assert!(err(&secret).contains("asking for a password"), "{}", err(&secret));

    let bad = w.cli("ssh", &["send", "web", "--keys", "nope"]);
    assert!(err(&bad).contains("Unknown key"));

    term.kill();
    for _ in 0..100 {
        if term.exited() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(term.exited());
    assert!(w.service.terminal_of(&id).is_none());
}

#[test]
fn files_copy_both_ways_and_uploads_ask() {
    let rt = server_rt();
    let disk = Disk::default();
    let port = start_server(&rt, disk.clone());
    let w = World::new();
    w.add_host("web", port, "right password", "changes");
    let local = w.dir.join("local.txt");
    let body: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&local, &body).unwrap();

    w.asked();
    w.answer_with(|k| Some(if matches!(k, PromptKind::Approve { .. }) { Answer::Once } else { Answer::Trust }));
    let up = w.cli("ssh", &["cp", local.to_str().unwrap(), "web:remote.txt"]);
    assert_eq!(up.status.code(), Some(0), "{}", err(&up));
    assert_eq!(disk.0.lock().unwrap().get("remote.txt").map(Vec::len), Some(body.len()));
    assert!(w.asked().iter().any(|k| matches!(k, PromptKind::Approve { action, .. } if action == "upload")));

    let back = w.dir.join("back.txt");
    let down = w.cli("ssh", &["cp", "web:~/remote.txt", back.to_str().unwrap()]);
    assert_eq!(down.status.code(), Some(0), "{}", err(&down));
    assert_eq!(std::fs::read(&back).unwrap(), body);
    assert!(w.asked().iter().all(|k| !matches!(k, PromptKind::Approve { .. })), "downloads only read");

    let both = w.cli("ssh", &["cp", "aa:b", "cc:d"]);
    assert!(err(&both).contains("between two hosts"));
    let drive = w.cli("ssh", &["cp", r"C:\a", r"D:\b"]);
    assert!(err(&drive).contains("one side must be"));
}

#[test]
fn agents_run_through_the_crew_command() {
    let w = World::new();
    // Stands in for Claude Code in print mode: reads the task and answers in its JSON stream.
    let fake = w.dir.join("fake-claude.cmd");
    std::fs::write(&fake, "@echo off\r\nmore >nul\r\necho {\"type\":\"system\",\"session_id\":\"sess-1\"}\r\necho {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"working on it\"}]}}\r\necho {\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"sess-1\",\"result\":\"the answer\"}\r\n").unwrap();
    let slow = w.dir.join("slow-claude.cmd");
    std::fs::write(&slow, "@echo off\r\nping -n 30 127.0.0.1 >nul\r\n").unwrap();
    let bad = w.dir.join("bad-claude.cmd");
    std::fs::write(&bad, "@echo off\r\necho it broke 1>&2\r\nexit /b 3\r\n").unwrap();
    let set_command = |path: &PathBuf| {
        std::fs::write(w.dir.join("config.json"), serde_json::json!({"claudeCommand": path.to_string_lossy()}).to_string()).unwrap();
    };
    set_command(&fake);
    let saved = roster::save(&w.paths, RosterAgent { name: "Reviewer".into(), cli: "claude".into(), access: "read".into(), instructions: "Review it.".into(), ..RosterAgent::default() }, |_| false).unwrap();

    let listed = w.cli("crew", &["agents"]);
    assert!(out(&listed).contains("Reviewer\tclaude\tonly reads"), "{}", out(&listed));
    let json = w.cli("crew", &["agents", "--json"]);
    assert!(out(&json).contains("Review it."));

    let unknown = w.cli("crew", &["ask", "Nobody", "x"]);
    assert_eq!(unknown.status.code(), Some(255));
    assert!(err(&unknown).contains("Agents: Reviewer"));

    let done = w.cli("crew", &["ask", "@reviewer", "check this"]);
    assert_eq!(done.status.code(), Some(0), "{} {}", out(&done), err(&done));
    assert!(out(&done).contains("=== Reviewer (r1) · done") && out(&done).contains("the answer"), "{}", out(&done));
    let run = &w.service.list_runs()[0];
    assert_eq!((run.status.as_str(), run.session_id.as_deref(), run.caller.as_str()), ("done", Some("sess-1"), "session-1"));

    let deep = w.cli_env("crew", &["ask", "Reviewer", "x"], &[("CREW_DEPTH", "2")]);
    assert!(err(&deep).contains("nested too deep"));

    set_command(&bad);
    let failed = w.cli("crew", &["ask", "Reviewer", "x"]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(out(&failed).contains("it broke"), "{}", out(&failed));

    set_command(&slow);
    let started = w.cli("crew", &["ask", "Reviewer", "x", "--bg"]);
    assert_eq!(started.status.code(), Some(0));
    let id = out(&started).trim().to_string();
    let waiting = w.cli("crew", &["result", &id]);
    assert_eq!(waiting.status.code(), Some(3), "still running");
    assert!(out(&waiting).contains("Still running"));
    let stopped = w.cli("crew", &["stop", &id]);
    assert!(out(&stopped).contains("stopped"), "{}", out(&stopped));
    let runs = w.cli("crew", &["runs"]);
    assert_eq!(out(&runs).lines().count(), 3);
    let wrong = w.cli("crew", &["wait", "r99"]);
    assert!(err(&wrong).contains("Unknown run id"));
    let _ = saved;
}
