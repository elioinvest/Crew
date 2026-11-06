//! The private pipe the `crew-ssh` and `crew` commands talk to: one JSON request per connection, then
//! a stream of JSON lines back. A session finds it through its environment.

use std::sync::{Arc, Mutex};

use base64::Engine;
use crew_core::hosts;
use rand::RngCore;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::Mutex as AsyncMutex;

use crate::Service;
use crate::rules;
use crate::ssh::{self, Abort};

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub struct BridgeInfo {
    pub pipe: String,
    pub token: String,
}

impl BridgeInfo {
    pub fn fresh() -> Self {
        Self { pipe: format!(r"\\.\pipe\crew-rs-{}", random_hex(8)), token: random_hex(24) }
    }
}

fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn start(service: &Service) {
    let s = service.clone();
    service.0.rt.spawn(async move {
        let mut options = ServerOptions::new();
        options.first_pipe_instance(true);
        let Ok(mut server) = options.create(&s.0.bridge.pipe) else { return };
        loop {
            if server.connect().await.is_err() {
                continue;
            }
            let client = server;
            let Ok(next) = ServerOptions::new().create(&s.0.bridge.pipe) else { return };
            server = next;
            let handler = s.clone();
            tokio::spawn(async move { handler.handle_client(client).await });
        }
    });
}

/// Where a client's answers go.
#[derive(Clone)]
pub(crate) struct Reply {
    writer: Arc<AsyncMutex<tokio::io::WriteHalf<NamedPipeServer>>>,
}

impl Reply {
    pub(crate) async fn send(&self, m: Value) {
        let mut w = self.writer.lock().await;
        let _ = w.write_all(format!("{m}\n").as_bytes()).await;
        let _ = w.flush().await;
    }
    pub(crate) async fn out(&self, bytes: &[u8]) {
        self.send(json!({"type": "out", "d": base64::engine::general_purpose::STANDARD.encode(bytes)})).await;
    }
    pub(crate) async fn err(&self, bytes: &[u8]) {
        self.send(json!({"type": "err", "d": base64::engine::general_purpose::STANDARD.encode(bytes)})).await;
    }
    pub(crate) async fn note(&self, text: &str) {
        self.err(format!("crew-ssh: {text}\n").as_bytes()).await;
    }
    pub(crate) async fn status(&self, text: &str) {
        self.send(json!({"type": "status", "message": text})).await;
    }
    pub(crate) async fn fail(&self, text: &str) {
        self.send(json!({"type": "fail", "message": text})).await;
    }
    pub(crate) async fn exit(&self, code: i64) {
        self.send(json!({"type": "exit", "code": code})).await;
    }
    async fn close(&self) {
        let mut w = self.writer.lock().await;
        let _ = w.flush().await;
        let _ = w.shutdown().await;
    }
}

impl Service {
    async fn handle_client(self, client: NamedPipeServer) {
        let (read, write) = tokio::io::split(client);
        let reply = Reply { writer: Arc::new(AsyncMutex::new(write)) };
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        if reader.read_line(&mut line).await.is_err() {
            return;
        }
        let Ok(m) = serde_json::from_str::<Value>(&line) else {
            reply.fail("Bad request.").await;
            reply.close().await;
            return;
        };
        if !same_token(m.get("token").and_then(Value::as_str).unwrap_or(""), &self.0.bridge.token) {
            reply.fail("Not authorized.").await;
            reply.close().await;
            return;
        }
        // The client going away stops what it started, and takes its question back.
        let abort = Arc::new(Abort::default());
        let prompt = Arc::new(Mutex::new(None::<u64>));
        let (watch_abort, watch_prompt, watch_service) = (abort.clone(), prompt.clone(), self.clone());
        tokio::spawn(async move {
            let mut rest = [0u8; 256];
            while matches!(reader.read(&mut rest).await, Ok(n) if n > 0) {}
            watch_abort.abort();
            if let Some(id) = *watch_prompt.lock().unwrap() {
                watch_service.cancel_prompt(id);
            }
        });
        self.run_op(&m, &reply, &abort, &prompt).await;
        reply.close().await;
    }

    async fn run_op(&self, m: &Value, reply: &Reply, abort: &Abort, prompt: &Mutex<Option<u64>>) {
        let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let op = text("op");
        if self.run_agent_op(&op, m, reply).await {
            return;
        }
        if op == "hosts" {
            let list: Vec<Value> = hosts::load(self.paths()).iter().map(|h| json!({"id": h.id, "name": h.name, "host": h.host, "port": h.port(), "user": h.user, "group": h.group, "notes": h.notes, "approval": if h.approval.is_empty() { "changes" } else { &h.approval }})).collect();
            reply.send(json!({"type": "result", "hosts": list})).await;
            return;
        }
        let h = match hosts::find(self.paths(), &text("host")) {
            Ok(h) => h,
            Err(e) => {
                reply.fail(&e).await;
                return;
            }
        };
        let (session, agent) = (text("session"), text("agent"));
        let no_terminal = format!("No terminal on {} is open in Crew. Use crew-ssh exec to run commands, or ask the user to open the host's terminal.", h.name);
        let waiting = || {
            let reply = reply.clone();
            self.0.rt.spawn(async move { reply.status("waiting for you to approve it in Crew").await });
        };
        let number = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.);
        match op.as_str() {
            "screen" => {
                let Some(shell) = self.terminal_of(&h.id) else {
                    reply.fail(&no_terminal).await;
                    return;
                };
                let wait = number("wait").clamp(0., 120.) as u64 * 1000;
                let state = if wait > 0 { Some(ssh::wait_quiet(&shell.term, ssh::now(), wait, abort).await) } else { None };
                if abort.is_aborted() {
                    return;
                }
                let lines = shell.term.lines_from(0);
                let count = (number("lines") as usize).clamp(1, 1000).max(if number("lines") == 0. { 60 } else { 1 });
                let from = lines.len().saturating_sub(count);
                reply.out(format!("{}\n", lines[from..].join("\n")).as_bytes()).await;
                match state {
                    Some("quiet") => reply.note("new output came and went quiet").await,
                    Some("none") => reply.note("no new output").await,
                    Some(_) => reply.note("output never went quiet: something may still be running").await,
                    None => {}
                }
                if shell.term.alt_screen() {
                    reply.note("a full-screen program is showing").await;
                }
                reply.exit(0).await;
            }
            "send" => {
                let Some(shell) = self.terminal_of(&h.id) else {
                    reply.fail(&no_terminal).await;
                    return;
                };
                let body = text("text");
                let keys: Vec<String> = m.get("keys").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|k| !k.trim().is_empty()).map(str::to_string).collect();
                if body.is_empty() && keys.is_empty() {
                    reply.fail("Nothing to send: give text, --keys, or both. To read more output, use crew-ssh screen <host> --wait N.").await;
                    return;
                }
                if let Some(bad) = rules::bad_key(&keys) {
                    reply.fail(&bad).await;
                    return;
                }
                if rules::asks_secret(&shell.term.cursor_line()) {
                    reply.fail("The terminal is asking for a password or code. The user types those: ask them to, then read the result with crew-ssh screen.").await;
                    return;
                }
                let described = rules::describe_input(&body, &keys);
                let reads = rules::input_only_reads(&shell.term, &body, &keys);
                let decision = self.approve(&h, "input", &described, "", &session, &agent, reads, prompt, waiting).await;
                if abort.is_aborted() {
                    return;
                }
                let Some(decision) = decision else {
                    self.log_activity(&h.id, &described, "denied", None, None);
                    reply.fail("The user denied this input in Crew. Ask them before trying again.").await;
                    return;
                };
                if self.terminal_of(&h.id).is_none() {
                    reply.fail(&no_terminal).await; // closed while waiting
                    return;
                }
                let seqs: Vec<String> = keys.iter().filter_map(|k| rules::key_sequence(k, shell.term.app_cursor())).collect();
                let start = shell.term.cursor_row();
                let sent_at = ssh::now();
                if !body.is_empty() {
                    shell.term.write(body.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
                }
                for s in seqs {
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    shell.term.write(s.as_bytes());
                }
                let wait = (number("wait").clamp(0., 120.) as u64).max(if number("wait") == 0. { 10 } else { 1 }) * 1000;
                let state = ssh::wait_quiet(&shell.term, sent_at, wait, abort).await;
                let lines = if shell.term.alt_screen() { shell.term.lines_from(shell.term.screen_top()) } else { shell.term.lines_from(start) };
                let from = lines.len().saturating_sub(400);
                self.log_activity(&h.id, &described, decision, Some(0), None);
                reply.out(format!("{}\n", lines[from..].join("\n")).as_bytes()).await;
                match state {
                    "quiet" => {}
                    "none" => reply.note("no output yet").await,
                    _ => reply.note("output is still coming: read more with crew-ssh screen <host> --wait N").await,
                }
                reply.exit(0).await;
            }
            "exec" => {
                let command = text("command");
                if command.trim().is_empty() {
                    reply.fail("No command given.").await;
                    return;
                }
                let cwd = text("cwd");
                let decision = self.approve(&h, "exec", &command, &cwd, &session, &agent, rules::only_reads(&command), prompt, waiting).await;
                if abort.is_aborted() {
                    return;
                }
                let Some(decision) = decision else {
                    self.log_activity(&h.id, &command, "denied", None, None);
                    reply.fail("The user denied this command in Crew. Ask them before trying again.").await;
                    return;
                };
                if !self.is_connected(&h.id) {
                    reply.status(&format!("connecting to {}", h.name)).await;
                }
                // Output goes out in order through a queue, as the SSH callback can't wait on the pipe.
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(bool, Vec<u8>)>();
                let forward = reply.clone();
                let sender = tokio::spawn(async move {
                    while let Some((is_err, bytes)) = rx.recv().await {
                        if is_err { forward.err(&bytes).await } else { forward.out(&bytes).await }
                    }
                });
                let (out_tx, err_tx) = (tx.clone(), tx);
                let result = self.exec_on(&h, &command, Some(&cwd), move |b| drop(out_tx.send((false, b.to_vec()))), move |b| drop(err_tx.send((true, b.to_vec()))), abort).await;
                let _ = sender.await;
                match result {
                    Ok(code) => {
                        self.log_activity(&h.id, &command, decision, Some(code), None);
                        reply.exit(code).await;
                    }
                    Err(e) => {
                        self.log_activity(&h.id, &command, decision, None, Some(e.to_string()));
                        reply.fail(&e.to_string()).await;
                    }
                }
            }
            "cp" => {
                let upload = text("direction") == "up";
                let (local, remote) = (text("local"), text("remote"));
                let described = if upload { format!("{local} → {}:{remote}", h.name) } else { format!("{}:{remote} → {local}", h.name) };
                let action = if upload { "upload" } else { "download" };
                let decision = self.approve(&h, action, &described, "", &session, &agent, !upload, prompt, waiting).await;
                if abort.is_aborted() {
                    return;
                }
                let Some(decision) = decision else {
                    self.log_activity(&h.id, &described, "denied", None, None);
                    reply.fail("The user denied this copy in Crew.").await;
                    return;
                };
                match self.copy_file(&h, upload, &local, &remote).await {
                    Ok(()) => {
                        self.log_activity(&h.id, &described, decision, Some(0), None);
                        reply.exit(0).await;
                    }
                    Err(e) => {
                        self.log_activity(&h.id, &described, decision, None, Some(e.to_string()));
                        reply.fail(&e.to_string()).await;
                    }
                }
            }
            other => reply.fail(&format!("Unknown operation \"{other}\".")).await,
        }
    }
}
