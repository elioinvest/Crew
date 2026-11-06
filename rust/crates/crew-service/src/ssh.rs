//! SSH on the model of Hatoba (github.com/scarletkc/Hatoba), as in the Electron app: connections are
//! shared per host and close after ten idle minutes, host keys are trusted on first use and blocked
//! when they change, and the user's terminal on a host can be read and typed into by agents.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use crew_core::hosts::{self, Host};
use crew_core::secrets;
use crew_term::{Palette, RemoteControl, Terminal};
use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, decode_secret_key};
use russh::keys::PublicKeyOrCertificate;
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};

use crate::{Answer, Event, PromptKind, Service};

const IDLE_MS: i64 = 10 * 60_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const LOG_SIZE: usize = 20;
const ALLOW_FOR_MS: i64 = 15 * 60_000;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Cancels with the client that asked: its pipe closing stops what it started.
#[derive(Default)]
pub struct Abort {
    aborted: AtomicBool,
    notify: Notify,
}

impl Abort {
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }
    pub async fn wait(&self) {
        while !self.is_aborted() {
            let waiting = self.notify.notified();
            if self.is_aborted() {
                return;
            }
            waiting.await;
        }
    }
}

struct Conn {
    handle: client::Handle<Handler>,
    busy: AtomicI64,
    last_used: AtomicI64,
}

pub struct Shell {
    pub term: Arc<Terminal>,
}

#[derive(Clone, Debug)]
pub struct ActivityItem {
    pub at: i64,
    pub command: String,
    pub decision: String,
    pub code: Option<i64>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct SshState {
    pool: Mutex<HashMap<String, Arc<Conn>>>,
    connecting: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    shells: Mutex<HashMap<String, Vec<Arc<Shell>>>>,
    activity: Mutex<HashMap<String, Vec<ActivityItem>>>,
    allowed_until: Mutex<HashMap<String, i64>>,
}

struct Handler {
    service: Service,
    host: Host,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else { return Ok(false) };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let key_type = key.algorithm().as_str().to_string();
        Ok(self.service.verify_host_key(&self.host, &key_type, &fingerprint).await)
    }
}

pub fn start_idle_sweep(service: &Service) {
    let s = service.clone();
    service.0.rt.spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let stale: Vec<String> = s
                .0
                .ssh
                .pool
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, c)| c.handle.is_closed() || (c.busy.load(Ordering::Relaxed) == 0 && now_ms() - c.last_used.load(Ordering::Relaxed) > IDLE_MS))
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                s.disconnect(&id);
            }
        }
    });
}

fn known_file(service: &Service) -> PathBuf {
    service.paths().data_dir.join("ssh-known-hosts.json")
}

fn expand_home(service: &Service, p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) => service.paths().home.join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(p),
    }
}

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Service {
    // ---------- host keys: trust on first use ----------

    async fn verify_host_key(&self, h: &Host, key_type: &str, fingerprint: &str) -> bool {
        let id = format!("{}:{}", h.host, h.port());
        let file = known_file(self);
        let read = || std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_else(|| json!({}));
        let known = read();
        let entry = known.get(&id);
        if entry.and_then(|e| e.get("fingerprint")).and_then(Value::as_str) == Some(fingerprint) {
            return true;
        }
        let previous = entry.filter(|e| e.get("type").and_then(Value::as_str) == Some(key_type)).and_then(|e| e.get("fingerprint")).and_then(Value::as_str);
        let kind = match previous {
            Some(p) => PromptKind::HostKeyChanged { host: h.label(), address: id.clone(), key_type: key_type.into(), fingerprint: fingerprint.into(), previous: p.into() },
            None => PromptKind::HostKey { host: h.label(), address: id.clone(), key_type: key_type.into(), fingerprint: fingerprint.into() },
        };
        let (_, rx) = self.ask(kind);
        if rx.await.ok().flatten() != Some(Answer::Trust) {
            return false;
        }
        let mut known = read();
        known[&id] = json!({"type": key_type, "fingerprint": fingerprint, "at": now_ms()});
        let _ = std::fs::write(&file, serde_json::to_string_pretty(&known).unwrap_or_default());
        true
    }

    // ---------- connections ----------

    fn touch(&self, id: &str) {
        if let Some(c) = self.0.ssh.pool.lock().unwrap().get(id) {
            c.last_used.store(now_ms(), Ordering::Relaxed);
        }
    }

    fn hold(&self, id: &str, delta: i64) {
        if let Some(c) = self.0.ssh.pool.lock().unwrap().get(id) {
            c.busy.fetch_add(delta, Ordering::Relaxed);
            c.last_used.store(now_ms(), Ordering::Relaxed);
        }
    }

    pub fn is_connected(&self, id: &str) -> bool {
        self.0.ssh.pool.lock().unwrap().get(id).is_some_and(|c| !c.handle.is_closed())
    }

    pub fn disconnect(&self, id: &str) {
        let conn = self.0.ssh.pool.lock().unwrap().remove(id);
        if let Some(conn) = conn {
            self.0.rt.spawn(async move {
                let _ = conn.handle.disconnect(Disconnect::ByApplication, "", "en").await;
            });
            self.send(Event::HostsChanged);
        }
    }

    pub fn disconnect_all(&self) {
        let ids: Vec<String> = self.0.ssh.pool.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.disconnect(&id);
        }
    }

    async fn connect(&self, h: &Host) -> Result<Arc<Conn>> {
        let lock = self.0.ssh.connecting.lock().unwrap().entry(h.id.clone()).or_default().clone();
        let _one_at_a_time = lock.lock().await;
        if let Some(c) = self.0.ssh.pool.lock().unwrap().get(&h.id).filter(|c| !c.handle.is_closed()) {
            c.last_used.store(now_ms(), Ordering::Relaxed);
            return Ok(c.clone());
        }
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            inactivity_timeout: None,
            ..client::Config::default()
        });
        let address = format!("{}:{}", h.host, h.port());
        // Only reaching the host times out quickly: the sign-in after it can wait on the user.
        let tcp = match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(&address)).await {
            Err(_) => return Err(anyhow!("Timed out connecting to {address}.")),
            Ok(Err(e)) => return Err(anyhow!("Can't reach {address}: {e}.")),
            Ok(Ok(s)) => s,
        };
        let handler = Handler { service: self.clone(), host: h.clone() };
        let mut handle = match tokio::time::timeout(SIGN_IN_TIMEOUT, client::connect_stream(config, tcp, handler)).await {
            Err(_) => return Err(anyhow!("Timed out signing in to {}.", h.host)),
            Ok(Err(russh::Error::UnknownKey)) => return Err(anyhow!("Crew didn't connect to {}: its host key wasn't trusted (or has changed since it was). The user has to confirm it in Crew.", h.name)),
            Ok(Err(e)) => return Err(anyhow!("Can't connect to {}: {e}.", h.name)),
            Ok(Ok(handle)) => handle,
        };
        self.sign_in(h, &mut handle).await?;
        let conn = Arc::new(Conn { handle, busy: AtomicI64::new(0), last_used: AtomicI64::new(now_ms()) });
        self.0.ssh.pool.lock().unwrap().insert(h.id.clone(), conn.clone());
        self.send(Event::HostsChanged);
        Ok(conn)
    }

    /// The key file, with its passphrase asked for if needed.
    async fn load_key(&self, h: &Host) -> Result<Option<PrivateKey>> {
        let wants_file = matches!(h.auth.as_str(), "key" | "auto" | "");
        if !wants_file {
            return Ok(None);
        }
        let default = ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|k| self.paths().home.join(".ssh").join(k)).find(|p| p.exists());
        let file = if h.key_path.is_empty() { if h.auth == "key" { None } else { default } } else { Some(expand_home(self, &h.key_path)) };
        let Some(file) = file else {
            if h.auth == "key" {
                return Err(anyhow!("No key file is set for {}.", h.name));
            }
            return Ok(None);
        };
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(_) if h.auth == "key" => return Err(anyhow!("Can't read the key file {}.", file.display())),
            Err(_) => return Ok(None),
        };
        let saved = h.secret.passphrase.as_deref().and_then(|s| secrets::unseal(self.paths(), s));
        match decode_secret_key(&text, saved.as_deref()) {
            Ok(k) => Ok(Some(k)),
            Err(russh::keys::Error::KeyIsEncrypted) | Err(russh::keys::Error::CouldNotReadKey) => {
                let (_, rx) = self.ask(PromptKind::Passphrase { host: h.label(), key_file: file.display().to_string() });
                let Ok(Some(Answer::Secret { value, remember })) = rx.await else { return Err(anyhow!("No passphrase was given for the key.")) };
                let key = decode_secret_key(&text, Some(&value)).map_err(|e| anyhow!("Can't use the key file {}: {e}.", file.display()))?;
                if remember {
                    hosts::remember(self.paths(), &h.id, None, Some(&value));
                }
                Ok(Some(key))
            }
            Err(e) if h.auth == "key" => Err(anyhow!("Can't use the key file {}: {e}.", file.display())),
            Err(_) => Ok(None),
        }
    }

    async fn sign_in(&self, h: &Host, handle: &mut client::Handle<Handler>) -> Result<()> {
        let user = if h.user.is_empty() { std::env::var("USERNAME").unwrap_or_default() } else { h.user.clone() };
        let failed = || anyhow!("Couldn't sign in to {}: check the user, key or password in Crew.", h.name);
        if let Some(key) = self.load_key(h).await? {
            let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
            let with_hash = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            if matches!(handle.authenticate_publickey(&user, with_hash).await, Ok(AuthResult::Success)) {
                return Ok(());
            }
        }
        if matches!(h.auth.as_str(), "agent" | "auto" | "") {
            if let Ok(mut agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
                let identities = agent.request_identities().await.unwrap_or_default();
                let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
                for identity in identities {
                    let key = identity.public_key().into_owned();
                    if matches!(handle.authenticate_publickey_with(&user, key, hash, &mut agent).await, Ok(AuthResult::Success)) {
                        return Ok(());
                    }
                }
            }
        }
        if matches!(h.auth.as_str(), "password" | "auto" | "") {
            let mut password = h.secret.password.as_deref().and_then(|s| secrets::unseal(self.paths(), s));
            let mut remember = false;
            if password.is_none() && h.auth == "password" {
                let (_, rx) = self.ask(PromptKind::Password { host: h.label() });
                let Ok(Some(Answer::Secret { value, remember: keep })) = rx.await else { return Err(anyhow!("No password was given.")) };
                password = Some(value);
                remember = keep;
            }
            if let Some(password) = password {
                if matches!(handle.authenticate_password(&user, &password).await, Ok(AuthResult::Success)) {
                    if remember {
                        hosts::remember(self.paths(), &h.id, Some(&password), None);
                    }
                    return Ok(());
                }
            }
        }
        // Keyboard-interactive: a password or a code the server asks for by itself.
        let mut reply = handle.authenticate_keyboard_interactive_start(&user, None::<String>).await.map_err(|_| failed())?;
        loop {
            match reply {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure { .. } => return Err(failed()),
                KeyboardInteractiveAuthResponse::InfoRequest { instructions, prompts, .. } => {
                    let answers = if prompts.is_empty() {
                        Vec::new()
                    } else {
                        let list = prompts.iter().map(|p| (p.prompt.clone(), p.echo)).collect();
                        let (_, rx) = self.ask(PromptKind::Keyboard { host: h.label(), instructions, prompts: list });
                        match rx.await {
                            Ok(Some(Answer::Keyboard(a))) => a,
                            _ => return Err(failed()),
                        }
                    };
                    reply = handle.authenticate_keyboard_interactive_respond(answers).await.map_err(|_| failed())?;
                }
            }
        }
    }

    /// Runs a command on the host's exec channel (no PTY, empty stdin). Returns its exit code.
    pub async fn exec_on(&self, h: &Host, command: &str, cwd: Option<&str>, mut on_out: impl FnMut(&[u8]) + Send, mut on_err: impl FnMut(&[u8]) + Send, abort: &Abort) -> Result<i64> {
        let conn = self.connect(h).await?;
        let line = match cwd.filter(|c| !c.is_empty()) {
            Some(c) => format!("cd {} && {command}", shq(c)),
            None => command.to_string(),
        };
        let mut channel = match conn.handle.channel_open_session().await {
            Ok(c) => c,
            Err(e) => {
                self.disconnect(&h.id);
                return Err(e.into());
            }
        };
        channel.exec(true, line.into_bytes()).await?;
        channel.eof().await?;
        let mut code = None;
        loop {
            tokio::select! {
                _ = abort.wait() => {
                    let _ = channel.signal(russh::Sig::KILL).await;
                    let _ = channel.close().await;
                    return Ok(255);
                }
                msg = channel.wait() => match msg {
                    Some(ChannelMsg::Data { data }) => on_out(&data),
                    Some(ChannelMsg::ExtendedData { data, .. }) => on_err(&data),
                    Some(ChannelMsg::ExitStatus { exit_status }) => code = Some(exit_status as i64),
                    Some(ChannelMsg::ExitSignal { .. }) => code = code.or(Some(255)),
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                },
            }
        }
        match code {
            Some(c) => {
                self.touch(&h.id);
                Ok(c)
            }
            None => {
                self.disconnect(&h.id);
                Err(anyhow!("The connection to {} closed before the command finished.", h.name))
            }
        }
    }

    pub async fn copy_file(&self, h: &Host, upload: bool, local: &str, remote: &str) -> Result<()> {
        let conn = self.connect(h).await?;
        let channel = conn.handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(channel.into_stream()).await?;
        let target = remote.strip_prefix("~/").unwrap_or(remote).to_string(); // SFTP starts in the home folder
        if upload {
            let mut from = tokio::fs::File::open(local).await?;
            let mut to = sftp.create(target).await?;
            tokio::io::copy(&mut from, &mut to).await?;
            to.shutdown().await?;
        } else {
            let mut from = sftp.open(target).await?;
            let mut to = tokio::fs::File::create(local).await?;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = from.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                to.write_all(&buf[..n]).await?;
            }
            to.flush().await?;
        }
        let _ = sftp.close().await;
        self.touch(&h.id);
        Ok(())
    }

    /// `whoami@hostname · system`, or why it couldn't connect. Blocks.
    pub fn test_host(&self, id: &str) -> Result<String, String> {
        let h = hosts::find(self.paths(), id)?;
        let service = self.clone();
        self.0.rt.block_on(async move {
            let abort = Abort::default();
            let mut out = Vec::new();
            let code = service.exec_on(&h, r#"echo "$(whoami)@$(hostname) · $(uname -sr 2>/dev/null || ver)""#, None, |b| out.extend_from_slice(b), |_| {}, &abort).await.map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out).trim().to_string();
            if code == 0 { Ok(text) } else { Err(if text.is_empty() { format!("Exit code {code}") } else { text }) }
        })
    }

    // ---------- terminals on a host ----------

    /// The user's terminal on a host: an SSH shell with a PTY. The terminal exists at once and shows
    /// the connecting; agents read and type into the same one (crew-ssh screen, send).
    pub fn open_terminal(&self, host_id: &str, palette: Palette) -> Result<Arc<Terminal>, String> {
        let h = hosts::find(self.paths(), host_id)?;
        enum Cmd {
            Write(Vec<u8>),
            Resize(u16, u16),
            Close,
        }
        struct Input(mpsc::UnboundedSender<Cmd>);
        impl Write for Input {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let _ = self.0.send(Cmd::Write(buf.to_vec()));
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (on_resize, on_close) = (tx.clone(), tx.clone());
        let term = Arc::new(Terminal::remote(
            120,
            30,
            palette,
            Box::new(Input(tx)),
            RemoteControl {
                resize: Box::new(move |c, r| {
                    let _ = on_resize.send(Cmd::Resize(c, r));
                }),
                close: Box::new(move || {
                    let _ = on_close.send(Cmd::Close);
                }),
            },
        ));
        term.note(&format!("\x1b[90mConnecting to {} ({})…\x1b[0m\r\n", h.name, h.address()));
        let (service, shown) = (self.clone(), term.clone());
        self.0.rt.spawn(async move {
            let term = shown;
            let opened = async {
                let conn = service.connect(&h).await?;
                let channel = conn.handle.channel_open_session().await?;
                let (cols, rows) = term.size();
                channel.request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[]).await?;
                channel.request_shell(true).await?;
                anyhow::Ok(channel)
            };
            let mut channel = match opened.await {
                Ok(c) => c,
                Err(e) => {
                    term.note(&format!("\x1b[31m{e}\x1b[0m\r\n\x1b[90m[Press Enter to try again]\x1b[0m\r\n"));
                    term.mark_exited();
                    return;
                }
            };
            let shell = Arc::new(Shell { term: term.clone() });
            service.0.ssh.shells.lock().unwrap().entry(h.id.clone()).or_default().push(shell.clone());
            service.hold(&h.id, 1);
            service.send(Event::HostsChanged);
            loop {
                tokio::select! {
                    msg = channel.wait() => match msg {
                        Some(ChannelMsg::Data { data }) => term.feed(&data),
                        Some(ChannelMsg::ExtendedData { data, .. }) => term.feed(&data),
                        Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => break,
                        _ => {}
                    },
                    cmd = rx.recv() => match cmd {
                        Some(Cmd::Write(bytes)) => { let _ = channel.data_bytes(bytes).await; }
                        Some(Cmd::Resize(c, r)) => { let _ = channel.window_change(c as u32, r as u32, 0, 0).await; }
                        Some(Cmd::Close) | None => { let _ = channel.close().await; break; }
                    },
                }
            }
            if let Some(list) = service.0.ssh.shells.lock().unwrap().get_mut(&h.id) {
                list.retain(|s| !Arc::ptr_eq(s, &shell));
            }
            service.hold(&h.id, -1);
            service.send(Event::HostsChanged);
            term.note("\r\n\x1b[90m[Disconnected — press Enter to reconnect]\x1b[0m\r\n");
            term.mark_exited();
        });
        Ok(term)
    }

    /// The host's most recently opened terminal: the one agents read and type into.
    pub fn terminal_of(&self, id: &str) -> Option<Arc<Shell>> {
        self.0.ssh.shells.lock().unwrap().get(id).and_then(|l| l.last().cloned())
    }

    // ---------- approvals and the activity log ----------

    pub fn activity(&self, id: &str) -> Vec<ActivityItem> {
        self.0.ssh.activity.lock().unwrap().get(id).cloned().unwrap_or_default()
    }

    pub(crate) fn log_activity(&self, id: &str, command: &str, decision: &str, code: Option<i64>, error: Option<String>) {
        let mut all = self.0.ssh.activity.lock().unwrap();
        let list = all.entry(id.to_string()).or_default();
        list.insert(0, ActivityItem { at: now_ms(), command: command.to_string(), decision: decision.to_string(), code, error });
        list.truncate(LOG_SIZE);
        drop(all);
        self.send(Event::Activity(id.to_string()));
    }

    pub fn forget_host(&self, id: &str) {
        self.disconnect(id);
        self.0.ssh.activity.lock().unwrap().remove(id);
        self.send(Event::HostsChanged);
    }

    /// "auto" or "approved" when it may run, None when denied. `prompt_id` lets the caller take the
    /// question back if the agent goes away.
    pub(crate) async fn approve(&self, h: &Host, action: &str, command: &str, cwd: &str, session: &str, agent: &str, reads: bool, prompt_id: &Mutex<Option<u64>>, waiting: impl Fn()) -> Option<&'static str> {
        let mode = if h.approval.is_empty() { "changes" } else { h.approval.as_str() };
        let allowed = self.0.ssh.allowed_until.lock().unwrap().get(&h.id).is_some_and(|t| *t > now_ms());
        if mode == "auto" || allowed || (mode == "changes" && reads) {
            return Some("auto");
        }
        waiting();
        let (id, rx) = self.ask(PromptKind::Approve { host_id: h.id.clone(), host: h.label(), action: action.into(), command: command.into(), cwd: cwd.into(), session: session.into(), agent: agent.into() });
        *prompt_id.lock().unwrap() = Some(id);
        match rx.await.ok().flatten() {
            Some(Answer::AllowHost) => {
                self.0.ssh.allowed_until.lock().unwrap().insert(h.id.clone(), now_ms() + ALLOW_FOR_MS);
                Some("approved")
            }
            Some(Answer::Once) => Some("approved"),
            _ => None,
        }
    }
}

/// Waits until a terminal's output has been quiet for a second after `since`, or `ms` passed. Says
/// which: "quiet" (new output that went quiet), "none" (no new output) or "busy" (still going).
pub(crate) async fn wait_quiet(term: &Terminal, since: i64, ms: u64, abort: &Abort) -> &'static str {
    let end = now_ms() + ms as i64;
    while !abort.is_aborted() {
        let now = now_ms();
        let last = term.last_output();
        let got = last > since;
        if got && now - last >= 1000 {
            return "quiet";
        }
        if now >= end {
            return if got { "busy" } else { "none" };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    "none"
}

pub(crate) fn now() -> i64 {
    now_ms()
}
