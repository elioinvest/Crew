//! Phone access: the Crew app on a phone (github CrewApp) connects here to see and drive the sessions
//! open in Crew, over a WebSocket. Off until the user turns it on in Settings; a phone pairs by
//! scanning a QR code that carries the address and a random token, and every request must carry that
//! token. On the same network it's plain WebSocket inside the LAN; from elsewhere use a private
//! network like Tailscale (its 100.x address is offered too), never an open port.
//!
//! What a phone gets: the open terminals (agents and host shells) with their recent output and live
//! updates, typing into them, opening sessions in Crew, crew-ssh's approval prompts, and the
//! @-agents' runs.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD};
use base64::engine::DecodePaddingMode;
use crew_core::config::Config;
use crew_core::sessions::{Session, scan_projects};
use crew_core::transcript::Follower;
use crew_core::{Paths, composer, hosts, providers, roster};
use crew_term::Terminal;
use futures_util::{SinkExt, StreamExt};
use regex::Regex;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::runs::PublicRun;
use crate::{Answer, Event, Prompt, PromptKind, Service};

const BUFFER: usize = 300 * 1024; // recent output kept per terminal, replayed when a phone opens it
const PROTOCOL: u32 = 2; // 2: sessions as chats (chat / msgs / screen)
const QUIET: u64 = 3000; // a terminal with no output for this long counts as idle (its agent finished or is waiting)
const UPLOAD_MAX: usize = 20 * 1024 * 1024;
const MESSAGE_MAX: usize = 30 * 1024 * 1024;
const DEFAULT_PORT: u16 = 7788;
// An agent redraws in many small pieces (its spinner alone, several times a second): phones get each
// terminal's output in one message per frame instead, which keeps them smooth.
const OUT_EVERY: Duration = Duration::from_millis(16);
const WATCH_EVERY: Duration = Duration::from_millis(120);
const CHAT_EVERY: Duration = Duration::from_millis(400);
const STATE_DELAY: Duration = Duration::from_millis(400);

const DENIED: &str = "This phone isn't paired with Crew, or the pairing code changed. Scan the QR code in Crew's Settings again.";

// The phone app's web version (a copy of CrewApp's www), served so any phone browser can use Crew
// without installing the app. It's only the page; everything else still needs the pairing token.
const PAGE: &[(&str, &str, &[u8])] = &[
    ("index.html", "text/html; charset=utf-8", include_bytes!("../../../../phone/index.html")),
    ("app.js", "text/javascript; charset=utf-8", include_bytes!("../../../../phone/app.js")),
    ("style.css", "text/css; charset=utf-8", include_bytes!("../../../../phone/style.css")),
    ("icon.svg", "image/svg+xml", include_bytes!("../../../../phone/icon.svg")),
    ("manifest.webmanifest", "application/manifest+json", include_bytes!("../../../../phone/manifest.webmanifest")),
    ("vendor/addon-fit.js", "text/javascript; charset=utf-8", include_bytes!("../../../../phone/vendor/addon-fit.js")),
    ("vendor/addon-webgl.js", "text/javascript; charset=utf-8", include_bytes!("../../../../phone/vendor/addon-webgl.js")),
    ("vendor/xterm.css", "text/css; charset=utf-8", include_bytes!("../../../../phone/vendor/xterm.css")),
    ("vendor/xterm.js", "text/javascript; charset=utf-8", include_bytes!("../../../../phone/vendor/xterm.js")),
];

macro_rules! re {
    ($pattern:expr) => {{
        static R: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
        &*R
    }};
}

/// A phone asked to open something, which goes through the window as if clicked there.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteOpen {
    pub host_id: Option<String>,
    pub session_id: Option<String>,
    pub terminal_only: bool,
    pub project_path: String,
    pub agent: String,
    pub provider: Option<String>,
    pub resume: bool,
}

/// What a phone needs from the window: a session's chips, their menus, and picks from those.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteAsk {
    pub id: u64,
    pub what: String,
    pub session_id: String,
    pub kind: String,
    pub menu_id: u64,
    pub index: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteAddress {
    pub address: String,
    /// "lan" or "tailscale".
    pub kind: String,
    pub name: String,
}

/// What Settings shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteInfo {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub addresses: Vec<RemoteAddress>,
    pub error: String,
    pub devices: Vec<String>,
    pub code: String,
    pub pair_url: String,
}

#[derive(Default)]
struct Output {
    buffer: String,
    pending: String,
    partial: Vec<u8>,
}

#[derive(Default)]
struct Activity {
    busy: bool,
    since: u64,
    last: u64,
}

/// An open terminal. It goes away with its pane: only the window keeps it alive.
struct Term {
    id: String,
    session_id: Mutex<String>,
    agent: String,
    cwd: String,
    term: Weak<Terminal>,
    out: Mutex<Output>,
    activity: Mutex<Activity>,
    dirty: AtomicBool,
}

impl Term {
    fn alive(&self) -> Option<Arc<Terminal>> {
        self.term.upgrade().filter(|t| !t.exited())
    }

    fn busy(&self) -> bool {
        let a = self.activity.lock().unwrap();
        a.busy && now_ms().saturating_sub(a.last) < QUIET
    }
}

struct Chat {
    session_id: String,
    follower: Option<Follower>,
    tried_at: Option<Instant>,
}

#[derive(Default)]
struct ClientState {
    attached: HashSet<String>,
    sized: HashSet<String>,
    chat: Option<Chat>,
    /// Changes whenever the chat followed does.
    chat_turn: u64,
    chat_term: Option<String>,
    last_screen: String,
    watch: Option<String>,
}

struct Client {
    tx: mpsc::UnboundedSender<Message>,
    closed: Notify,
    ready: AtomicBool,
    device: Mutex<String>,
    state: Mutex<ClientState>,
}

impl Client {
    fn send(&self, message: &Value) {
        let _ = self.tx.send(Message::text(message.to_string()));
    }

    fn close(&self) {
        let _ = self.tx.send(Message::Close(None));
    }
}

pub struct Remote {
    paths: Paths,
    events: async_channel::Sender<Event>,
    terms: Mutex<Vec<Arc<Term>>>,
    clients: Mutex<Vec<Arc<Client>>>,
    prompts: Mutex<Vec<Value>>,
    runs: Mutex<Vec<Value>>,
    asks: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    ask_seq: AtomicU64,
    error: Mutex<String>,
    running: AtomicBool,
    stop: Mutex<Option<watch::Sender<bool>>>,
    state_due: Mutex<Option<Instant>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Computer".to_string())
}

fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

fn new_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 18]>())
}

fn text<'a>(m: &'a Value, key: &str) -> &'a str {
    m.get(key).and_then(Value::as_str).unwrap_or("")
}

fn truthy(m: &Value, key: &str) -> bool {
    m.get(key).is_some_and(|v| match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::String(s) => !s.is_empty(),
        _ => true,
    })
}

fn optional(m: &Value, key: &str) -> Option<String> {
    Some(text(m, key).to_string()).filter(|s| !s.is_empty())
}

/// Bytes to text, keeping a character cut between two reads for the next.
fn decode(partial: &mut Vec<u8>, bytes: &[u8]) -> String {
    partial.extend_from_slice(bytes);
    let mut out = String::with_capacity(partial.len());
    let mut rest = &partial[..];
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                rest = &[];
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                out.push_str(std::str::from_utf8(&rest[..good]).unwrap_or(""));
                match e.error_len() {
                    None => {
                        rest = &rest[good..];
                        break;
                    }
                    Some(n) => {
                        out.push('\u{fffd}');
                        rest = &rest[good + n..];
                    }
                }
            }
        }
    }
    *partial = rest.to_vec();
    out
}

fn trim_front(buffer: &mut String) {
    // A little over before cutting: this runs on every read of a busy terminal.
    if buffer.len() > BUFFER + 64 * 1024 {
        let mut cut = buffer.len() - BUFFER;
        while !buffer.is_char_boundary(cut) {
            cut += 1;
        }
        buffer.drain(..cut);
    }
}

/// The computer's addresses a phone can reach: the LAN ones, and Tailscale's (100.64.0.0/10).
pub fn addresses() -> Vec<RemoteAddress> {
    let mut out: Vec<RemoteAddress> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| {
            let std::net::IpAddr::V4(ip) = i.ip() else { return None };
            let [a, b, ..] = ip.octets();
            let tailscale = a == 100 && (64..=127).contains(&b);
            if !tailscale && a == 169 && b == 254 {
                return None;
            }
            Some(RemoteAddress { address: ip.to_string(), kind: if tailscale { "tailscale" } else { "lan" }.to_string(), name: i.name.clone() })
        })
        .collect();
    // The first one goes in the QR code's link. 198.18.0.0/15 is a proxy's or VPN's virtual adapter,
    // never a network a phone is on, so it comes after the real ones.
    out.sort_by_key(|a| (a.kind != "lan", a.address.starts_with("198.18.") || a.address.starts_with("198.19.")));
    out
}

fn norm_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn brief_session(s: &Session) -> Value {
    json!({ "id": s.id, "title": s.title, "agent": s.agent, "provider": s.provider, "mtime": s.mtime })
}

fn prompt_json(p: &Prompt) -> Value {
    let mut out = match &p.kind {
        PromptKind::HostKey { host, address, key_type, fingerprint } => json!({ "kind": "hostkey", "host": host, "address": address, "type": key_type, "fingerprint": fingerprint }),
        PromptKind::HostKeyChanged { host, address, key_type, fingerprint, previous } => json!({ "kind": "hostkey-changed", "host": host, "address": address, "type": key_type, "fingerprint": fingerprint, "previous": previous }),
        PromptKind::Password { host } => json!({ "kind": "password", "host": host }),
        PromptKind::Passphrase { host, key_file } => json!({ "kind": "passphrase", "host": host, "keyFile": key_file }),
        PromptKind::Keyboard { host, instructions, prompts } => json!({
            "kind": "keyboard", "host": host, "instructions": instructions,
            "prompts": prompts.iter().map(|(prompt, echo)| json!({ "prompt": prompt, "echo": echo })).collect::<Vec<_>>(),
        }),
        PromptKind::Approve { host_id, host, action, command, cwd, session, agent } => json!({ "kind": "approve", "hostId": host_id, "host": host, "action": action, "command": command, "cwd": cwd, "session": session, "agent": agent }),
    };
    out["id"] = json!(p.id);
    out
}

/// A phone's answer as the window's: "trust", {value, remember}, [answers], "once", "host", or null.
fn phone_answer(v: &Value) -> Option<Answer> {
    match v {
        Value::String(s) => match s.as_str() {
            "trust" => Some(Answer::Trust),
            "once" => Some(Answer::Once),
            "host" => Some(Answer::AllowHost),
            _ => None,
        },
        Value::Array(a) => Some(Answer::Keyboard(a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect())),
        Value::Object(_) => Some(Answer::Secret { value: text(v, "value").to_string(), remember: truthy(v, "remember") }),
        _ => None,
    }
}

fn run_json(r: &PublicRun) -> Value {
    json!({
        "id": r.id, "agent": r.agent, "cli": r.cli, "task": r.task, "cwd": r.cwd, "caller": r.caller, "status": r.status, "activity": r.activity,
        "startedAt": r.started_at, "endedAt": r.ended_at, "result": r.result, "error": r.error, "sessionId": r.session_id,
    })
}

// ---------- what only the terminal shows ----------

fn chars(s: &str, from: usize) -> String {
    s.chars().skip(from).collect()
}

/// What the agent is writing or running right now, as its terminal shows it: from its last bullet
/// (Claude's ⏺, Codex's •) down to the working line. The transcript has it only once it's done.
fn live_block(lines: &[String]) -> String {
    let working = re!(r"(?i)esc to (interrupt|cancel)");
    let Some(end) = lines.iter().rposition(|l| working.is_match(l)) else { return String::new() };
    let bullet = re!(r"^\s*[⏺●•]\s");
    let Some(start) = (end.saturating_sub(250)..end).rev().find(|i| bullet.is_match(&lines[*i])) else { return String::new() };
    let mut block: Vec<&String> = lines[start..end].iter().collect();
    while block.last().is_some_and(|l| l.trim().is_empty()) {
        block.pop();
    }
    // Text under the bullet is indented to its text.
    let indent = block[0].chars().position(|c| "⏺●•".contains(c)).unwrap_or(0) + 2;
    let joined = block
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let leading = l.chars().position(|c| !c.is_whitespace()).unwrap_or(l.chars().count());
            chars(l, if i == 0 { indent } else { indent.min(leading) })
        })
        .collect::<Vec<_>>()
        .join("\n");
    joined.trim_end().chars().take(6000).collect()
}

/// "✻ Churning… (12s · ↓ 1.2k tokens · esc to interrupt)" -> "Churning… (12s · ↓ 1.2k tokens)"
fn working_line(lines: &[String]) -> String {
    let working = re!(r"(?i)esc to (interrupt|cancel)");
    let Some(l) = lines.iter().rev().find(|l| working.is_match(l)) else { return String::new() };
    let l = re!(r"(?i)\s*[·•]?\s*esc to (interrupt|cancel)").replace(l, "");
    let l = re!(r"\(\s*\)").replace(&l, "");
    re!(r"^[\s✻✶✳✢·•*+⏺●◦]+").replace(&l, "").trim().to_string()
}

/// A choice the agent is waiting on: numbered rows near the bottom, one of them highlighted (❯ or ›),
/// under a question. Numbered lists in replies have no highlight, so they don't count.
fn ask_of(lines: &[String]) -> Value {
    let row = re!(r"^[\s│|]*([❯›>])?\s*(\d+)[.)]\s+(.+?)[\s│|]*$");
    let mut rows: Vec<(u64, String)> = Vec::new();
    let mut marked = false;
    let mut first_at = 0;
    for (i, line) in lines.iter().enumerate().skip(lines.len().saturating_sub(24)) {
        let Some(m) = row.captures(line) else { continue };
        if rows.is_empty() {
            first_at = i;
        }
        marked |= m.get(1).is_some();
        let label = re!(r"\s{2,}.*$").replace(&m[3], "").trim().to_string();
        rows.push((m[2].parse().unwrap_or(0), label));
    }
    if !marked || rows.len() < 2 || rows[0].0 != 1 {
        return Value::Null;
    }
    let question = (first_at.saturating_sub(8)..first_at)
        .rev()
        .map(|i| re!(r"[│╭╮╰╯─┌┐└┘|]").replace_all(&lines[i], "").trim().to_string())
        .find(|t| !t.is_empty())
        .unwrap_or_default();
    json!({ "question": question, "options": rows.iter().take(9).map(|(n, label)| json!({ "n": n, "label": label })).collect::<Vec<_>>() })
}

/// What a phone following the terminal sees, sent when it changes.
fn screen_state(term: &Terminal, busy: bool) -> Value {
    let all = term.raw_lines(200);
    let rows = term.size().1 as usize;
    let lines = &all[all.len().saturating_sub(rows)..];
    json!({
        "working": if busy { working_line(lines) } else { String::new() },
        "live": if busy { live_block(&all) } else { String::new() },
        "ask": if busy { Value::Null } else { ask_of(lines) },
    })
}

fn screen_text(term_id: &str, term: &Terminal) -> Value {
    let (lines, (x, y)) = term.text_runs(150);
    json!({ "t": "screen-text", "termId": term_id, "lines": lines, "cursor": [x, y] })
}

fn session_file(paths: &Paths, id: &str) -> Option<(PathBuf, &'static str)> {
    composer::session_transcript(paths, &providers::homes(paths), id)
}

/// A file from the phone (a photo, a document), saved with Crew's pasted images so its path can go
/// into a message for the agent.
fn save_upload(paths: &Paths, name: &str, data: &str) -> Result<PathBuf, String> {
    const LENIENT: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent));
    let bytes = LENIENT.decode(data.as_bytes()).unwrap_or_default();
    if bytes.is_empty() {
        return Err("The file is empty.".to_string());
    }
    if bytes.len() > UPLOAD_MAX {
        return Err("Files up to 20 MB can be sent.".to_string());
    }
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let base = if name.is_empty() { "file" } else { base };
    let clean = re!(r"[^A-Za-z0-9_.\-一-鿿]+").replace_all(base, "_").to_string();
    let count = clean.chars().count();
    let safe = chars(&clean, count.saturating_sub(80));
    let safe = if safe.is_empty() { "file".to_string() } else { safe };
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let random: [u8; 3] = rand::random();
    let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let dir = paths.data_dir.join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("phone-{stamp}-{hex}-{safe}"));
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    Ok(file)
}

// ---------- hooks from the rest of Crew ----------

impl Remote {
    pub(crate) fn new(paths: Paths, events: async_channel::Sender<Event>) -> Self {
        Self {
            paths,
            events,
            terms: Mutex::default(),
            clients: Mutex::default(),
            prompts: Mutex::default(),
            runs: Mutex::default(),
            asks: Mutex::default(),
            ask_seq: AtomicU64::new(0),
            error: Mutex::default(),
            running: AtomicBool::new(false),
            stop: Mutex::default(),
            state_due: Mutex::default(),
        }
    }

    fn changed(&self) {
        let _ = self.events.try_send(Event::RemoteChanged);
    }

    fn all_clients(&self) -> Vec<Arc<Client>> {
        self.clients.lock().unwrap().clone()
    }

    fn broadcast(&self, message: &Value) {
        for c in self.all_clients() {
            if c.ready.load(Ordering::Acquire) {
                c.send(message);
            }
        }
    }

    fn broadcast_attached(&self, term_id: &str, message: &Value) {
        for c in self.all_clients() {
            if c.ready.load(Ordering::Acquire) && c.state.lock().unwrap().attached.contains(term_id) {
                c.send(message);
            }
        }
    }

    fn find(&self, term_id: &str) -> Option<(Arc<Term>, Arc<Terminal>)> {
        let entry = self.terms.lock().unwrap().iter().find(|t| t.id == term_id).cloned()?;
        let term = entry.alive()?;
        Some((entry, term))
    }

    fn state_changed(&self) {
        let mut due = self.state_due.lock().unwrap();
        *due = Some(Instant::now() + STATE_DELAY);
    }

    fn on_output(&self, t: &Term, bytes: &[u8]) {
        {
            let mut out = t.out.lock().unwrap();
            let text = decode(&mut out.partial, bytes);
            if text.is_empty() {
                return;
            }
            out.buffer.push_str(&text);
            trim_front(&mut out.buffer);
            if !self.clients.lock().unwrap().is_empty() {
                out.pending.push_str(&text);
            }
        }
        t.dirty.store(true, Ordering::Release);
        self.note_activity(t);
    }

    /// Working or idle, from the output: an agent at work keeps redrawing its spinner and output, and
    /// goes quiet when it's done or waiting for you. A phone shows it and says when one finished.
    fn note_activity(&self, t: &Term) {
        let now = now_ms();
        let started = {
            let mut a = t.activity.lock().unwrap();
            let started = !a.busy || now.saturating_sub(a.last) >= QUIET;
            if started {
                a.busy = true;
                a.since = now;
            }
            a.last = now;
            started
        };
        if started {
            self.broadcast(&json!({ "t": "activity", "termId": t.id, "busy": true }));
        }
    }

    fn on_prompt(&self, p: &Prompt) {
        let value = prompt_json(p);
        self.prompts.lock().unwrap().push(value.clone());
        self.broadcast(&json!({ "t": "prompt", "prompt": value }));
    }

    fn on_prompt_done(&self, id: u64) {
        let mut prompts = self.prompts.lock().unwrap();
        let before = prompts.len();
        prompts.retain(|p| p["id"] != json!(id));
        let removed = prompts.len() != before;
        drop(prompts);
        if removed {
            self.broadcast(&json!({ "t": "prompt-done", "id": id }));
        }
    }

    fn on_run(&self, r: &PublicRun) {
        let value = run_json(r);
        {
            let mut runs = self.runs.lock().unwrap();
            match runs.iter_mut().find(|x| x["id"] == value["id"]) {
                Some(old) => *old = value.clone(),
                None => runs.push(value.clone()),
            }
            if runs.len() > 30 {
                runs.remove(0);
            }
        }
        self.broadcast(&json!({ "t": "run", "run": value }));
    }

    // ---------- what a phone sees ----------

    /// The saved token; made the first time it's needed.
    fn token(&self) -> String {
        let mut config = Config::load(&self.paths);
        let saved = config.str("remoteToken").to_string();
        if !saved.is_empty() {
            return saved;
        }
        let token = new_token();
        config.set("remoteToken", json!(token));
        let _ = config.save(&self.paths);
        self.changed();
        token
    }

    fn port(&self) -> u16 {
        Config::load(&self.paths).raw().get("remotePort").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0).unwrap_or(DEFAULT_PORT)
    }

    /// Everything a state message holds but the prompts and runs: reads the session lists, so not on
    /// the runtime's own threads.
    fn state_message(&self, service: &Service) -> Value {
        let mut config = Config::load(&self.paths);
        let homes = providers::homes(&self.paths);
        let (scanned, _) = scan_projects(&self.paths, &mut config, &homes);
        let host_list = hosts::list(&self.paths);

        let mut titles: HashMap<&str, (Option<&String>, &str)> = HashMap::new();
        for p in &scanned {
            for s in &p.sessions {
                titles.insert(&s.id, (s.title.as_ref(), &p.name));
            }
        }
        let terminals: Vec<Value> = self
            .terms
            .lock()
            .unwrap()
            .iter()
            .filter_map(|t| {
                let term = t.alive()?;
                let session = t.session_id.lock().unwrap().clone();
                let (cols, rows) = term.size();
                let host = if t.agent == "ssh" { session.strip_prefix("ssh:").and_then(|id| host_list.iter().find(|h| h.id == id)).map(|h| h.name.clone()) } else { None };
                let found = titles.get(session.as_str());
                let title = match &host {
                    Some(name) => format!("{name} · terminal"),
                    None => found.and_then(|f| f.0).cloned().unwrap_or_else(|| "New session".to_string()),
                };
                let project = host.unwrap_or_else(|| found.map_or(String::new(), |f| f.1.to_string()));
                Some(json!({ "termId": t.id, "sessionId": session, "agent": t.agent, "cwd": t.cwd, "cols": cols, "rows": rows, "busy": t.busy(), "title": title, "project": project }))
            })
            .collect();

        // As Crew's sidebar has them: the projects; the hosts, each with the agent sessions of its
        // workspace (which isn't listed as a project); and the @-agents.
        let workspace_of = |h: &hosts::Host| hosts::workspace_path(&self.paths, h).to_string_lossy().to_string();
        let workspaces: HashSet<String> = host_list.iter().map(|h| norm_path(&workspace_of(h))).collect();
        let projects: Vec<Value> = scanned
            .iter()
            .filter(|p| !workspaces.contains(&norm_path(&p.path.to_string_lossy())))
            .take(60)
            .map(|p| json!({ "path": p.path, "name": p.name, "sessions": p.sessions.iter().take(40).map(brief_session).collect::<Vec<_>>() }))
            .collect();
        let host_values: Vec<Value> = host_list
            .iter()
            .map(|h| {
                let workspace = workspace_of(h);
                let wanted = norm_path(&workspace);
                let sessions: Vec<Value> = scanned.iter().find(|p| norm_path(&p.path.to_string_lossy()) == wanted).map_or(Vec::new(), |p| p.sessions.iter().take(20).map(brief_session).collect());
                json!({ "id": h.id, "name": h.name, "group": h.group, "user": h.user, "host": h.host, "port": h.port.unwrap_or(22), "workspace": workspace, "connected": service.is_connected(&h.id), "sessions": sessions })
            })
            .collect();
        let agents: Vec<Value> = roster::list(&self.paths).iter().map(|a| json!({ "id": a.id, "name": a.name, "cli": a.cli, "access": a.access, "model": a.model })).collect();
        let provider_values: Vec<Value> = providers::list(&self.paths).iter().map(|p| json!({ "id": p.id, "name": p.name, "cli": p.cli(), "icon": p.emoji })).collect();
        json!({
            "t": "state", "terminals": terminals, "projects": projects, "hosts": host_values, "agents": agents, "providers": provider_values,
            "prompts": *self.prompts.lock().unwrap(), "runs": *self.runs.lock().unwrap(),
        })
    }

    // ---------- a terminal at the phone's size ----------

    /// While a phone shows a terminal, the terminal takes the phone's size, so the agent lays its
    /// screen out for the phone (no sideways scrolling); when the phone leaves, it goes back to the
    /// window's size. The latest phone to open it wins.
    fn fit_to_phone(&self, c: &Arc<Client>, term_id: &str, m: &Value) -> bool {
        let Some((_, term)) = self.find(term_id) else { return false };
        let size = |key: &str, low: f64, high: f64| m.get(key).and_then(Value::as_f64).unwrap_or(0.).round().clamp(low, high) as u16;
        let (cols, rows) = (size("cols", 20., 300.), size("rows", 8., 200.));
        for other in self.all_clients() {
            if !Arc::ptr_eq(&other, c) {
                other.state.lock().unwrap().sized.remove(term_id);
            }
        }
        c.state.lock().unwrap().sized.insert(term_id.to_string());
        let before = term.size();
        term.set_phone_size(Some((cols, rows)));
        before != (cols, rows)
    }

    fn release(&self, c: &Client, term_id: &str) {
        if c.state.lock().unwrap().sized.remove(term_id) {
            if let Some((_, term)) = self.find(term_id) {
                term.set_phone_size(None);
            }
        }
    }

    // ---------- a session as a chat ----------

    fn push_screen(&self, c: &Client) {
        let Some(term_id) = c.state.lock().unwrap().chat_term.clone() else { return };
        let Some((entry, term)) = self.find(&term_id) else { return };
        let mut screen = screen_state(&term, entry.busy());
        let text = screen.to_string();
        {
            let mut state = c.state.lock().unwrap();
            if state.last_screen == text {
                return;
            }
            state.last_screen = text;
        }
        screen["t"] = json!("screen");
        screen["termId"] = json!(term_id);
        c.send(&screen);
    }

    /// The chat a phone follows: the transcript, read at once, then what's added to it.
    fn follow_chat(&self, c: &Client, session_id: &str, term_id: Option<String>) {
        let mut follower = session_file(&self.paths, session_id).map(|(file, agent)| Follower::new(&file, agent));
        let items = follower.as_mut().map_or(Vec::new(), Follower::read);
        {
            let mut state = c.state.lock().unwrap();
            state.chat = Some(Chat { session_id: session_id.to_string(), follower, tried_at: Some(Instant::now()) });
            state.chat_turn += 1;
            state.chat_term = term_id;
            state.last_screen.clear();
        }
        c.send(&json!({ "t": "msgs", "sessionId": session_id, "items": last_items(items), "reset": true }));
        self.push_screen(c);
    }

    fn poll_chats(&self) {
        for c in self.all_clients() {
            // Reads away from the lock: messages to this phone keep going meanwhile.
            let (mut chat, turn) = {
                let mut state = c.state.lock().unwrap();
                let Some(chat) = state.chat.take() else { continue };
                (chat, state.chat_turn)
            };
            let mut message = None;
            if let Some(follower) = chat.follower.as_mut() {
                let items = follower.poll();
                if !items.is_empty() {
                    message = Some(json!({ "t": "msgs", "sessionId": chat.session_id, "items": items }));
                }
            } else if chat.tried_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(2500)) {
                // A new session's file appears once its first message is written.
                chat.tried_at = Some(Instant::now());
                if let Some((file, agent)) = session_file(&self.paths, &chat.session_id) {
                    let mut follower = Follower::new(&file, agent);
                    message = Some(json!({ "t": "msgs", "sessionId": chat.session_id, "items": last_items(follower.read()), "reset": true }));
                    chat.follower = Some(follower);
                }
            }
            {
                let mut state = c.state.lock().unwrap();
                if state.chat_turn != turn {
                    continue;
                }
                state.chat = Some(chat);
            }
            if let Some(message) = message {
                c.send(&message);
            }
            self.push_screen(&c);
        }
    }

    // ---------- the steady work ----------

    fn flush_out(&self) {
        let terms = self.terms.lock().unwrap().clone();
        for t in terms {
            let data = std::mem::take(&mut t.out.lock().unwrap().pending);
            if !data.is_empty() {
                self.broadcast_attached(&t.id, &json!({ "t": "out", "termId": t.id, "data": data }));
            }
        }
    }

    fn sweep_activity(&self) {
        let now = now_ms();
        let terms = self.terms.lock().unwrap().clone();
        for t in terms {
            let ended = {
                let mut a = t.activity.lock().unwrap();
                if a.busy && now.saturating_sub(a.last) >= QUIET {
                    a.busy = false;
                    Some(a.last.saturating_sub(a.since))
                } else {
                    None
                }
            };
            if let Some(ran) = ended {
                self.broadcast(&json!({ "t": "activity", "termId": t.id, "busy": false, "ran": ran }));
            }
        }
    }

    /// Terminals that ended, or whose pane closed.
    fn sweep_terminals(&self) {
        let mut gone = Vec::new();
        self.terms.lock().unwrap().retain(|t| {
            let alive = t.alive().is_some();
            if !alive {
                gone.push(t.id.clone());
            }
            alive
        });
        for id in &gone {
            self.broadcast_attached(id, &json!({ "t": "exit", "termId": id, "code": null }));
        }
        if !gone.is_empty() {
            self.state_changed();
        }
    }

    fn push_watched(&self) {
        let terms = self.terms.lock().unwrap().clone();
        let clients = self.all_clients();
        for t in terms.iter().filter(|t| t.dirty.swap(false, Ordering::AcqRel)) {
            let Some(term) = t.alive() else { continue };
            let watchers: Vec<_> = clients.iter().filter(|c| c.state.lock().unwrap().watch.as_deref() == Some(t.id.as_str())).collect();
            if !watchers.is_empty() {
                let message = screen_text(&t.id, &term);
                for c in watchers {
                    c.send(&message);
                }
            }
        }
    }

    // ---------- the composer's chips ----------

    /// A session's model, effort and mode chips and their menus come from Crew's window, which builds
    /// them for its own composer; a pick made on the phone runs there too.
    async fn ask_window(&self, what: &str, session_id: &str, kind: &str, menu_id: u64, index: usize, wait: Duration) -> Value {
        let id = self.ask_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.asks.lock().unwrap().insert(id, tx);
        let ask = RemoteAsk { id, what: what.to_string(), session_id: session_id.to_string(), kind: kind.to_string(), menu_id, index };
        let _ = self.events.try_send(Event::RemoteAsk(ask));
        let answer = tokio::time::timeout(wait, rx).await;
        self.asks.lock().unwrap().remove(&id);
        answer.ok().and_then(Result::ok).unwrap_or(Value::Null)
    }

    async fn send_chips(&self, c: &Client, session_id: &str) {
        let chips = self.ask_window("chips", session_id, "", 0, 0, Duration::from_secs(5)).await;
        c.send(&json!({ "t": "chips", "sessionId": session_id, "chips": chips }));
    }
}

fn last_items(items: Vec<Value>) -> Vec<Value> {
    let skip = items.len().saturating_sub(400);
    items.into_iter().skip(skip).collect()
}

// ---------- requests from a phone ----------

async fn handle(service: &Service, c: &Arc<Client>, m: Value) {
    let remote = service.0.remote.clone();
    let t = text(&m, "t");
    if !c.ready.load(Ordering::Acquire) {
        if t != "hello" || !same_token(text(&m, "token"), &remote.token()) {
            c.send(&json!({ "t": "denied", "reason": DENIED }));
            c.close();
            return;
        }
        *c.device.lock().unwrap() = Some(text(&m, "device")).filter(|d| !d.is_empty()).unwrap_or("Phone").chars().take(60).collect();
        c.ready.store(true, Ordering::Release);
        c.send(&json!({ "t": "ready", "protocol": PROTOCOL, "computer": hostname() }));
        send_state(service, c).await;
        remote.changed();
        return;
    }
    let term_id = text(&m, "termId").to_string();
    match t {
        "state" => send_state(service, c).await,
        "attach" => {
            if remote.find(&term_id).is_none() {
                c.send(&json!({ "t": "exit", "termId": term_id, "code": null }));
                return;
            }
            c.state.lock().unwrap().attached.insert(term_id.clone());
            // Shown at the phone's size: the agent redraws for it, then the phone gets the screen.
            let resized = truthy(&m, "cols") && truthy(&m, "rows") && remote.fit_to_phone(c, &term_id, &m);
            let (remote, c) = (remote.clone(), c.clone());
            tokio::spawn(async move {
                if resized {
                    tokio::time::sleep(Duration::from_millis(450)).await;
                }
                if !c.state.lock().unwrap().attached.contains(&term_id) {
                    return;
                }
                remote.flush_out(); // what's pending is in the snapshot: don't send it again after
                let Some((entry, term)) = remote.find(&term_id) else { return };
                let data = entry.out.lock().unwrap().buffer.clone();
                let (cols, rows) = term.size();
                c.send(&json!({ "t": "snapshot", "termId": term_id, "data": data, "cols": cols, "rows": rows }));
            });
        }
        "resize" => {
            if c.state.lock().unwrap().attached.contains(&term_id) {
                remote.fit_to_phone(c, &term_id, &m);
            }
        }
        "detach" => {
            c.state.lock().unwrap().attached.remove(&term_id);
            remote.release(c, &term_id);
        }
        "input" => {
            if let Some((_, term)) = remote.find(&term_id) {
                term.write(text(&m, "data").as_bytes());
            }
        }
        // Opening a session goes through Crew's window, which shows it as usual.
        "open" => service.send(Event::RemoteOpen(RemoteOpen {
            host_id: None,
            session_id: optional(&m, "sessionId"),
            terminal_only: false,
            project_path: text(&m, "projectPath").to_string(),
            agent: optional(&m, "agent").unwrap_or_else(|| "claude".to_string()),
            provider: optional(&m, "provider"),
            resume: truthy(&m, "resume"),
        })),
        // A host: its terminal beside an agent session, as clicking it in Crew's sidebar does.
        "open-host" => service.send(Event::RemoteOpen(RemoteOpen {
            host_id: optional(&m, "hostId"),
            session_id: optional(&m, "sessionId"),
            terminal_only: truthy(&m, "terminalOnly"),
            project_path: String::new(),
            agent: String::new(),
            provider: None,
            resume: false,
        })),
        "chips" => {
            let (remote, c) = (remote.clone(), c.clone());
            let session = text(&m, "sessionId").to_string();
            tokio::spawn(async move { remote.send_chips(&c, &session).await });
        }
        "chat" => {
            let (work, client) = (remote.clone(), c.clone());
            let session = text(&m, "sessionId").to_string();
            let term = optional(&m, "termId");
            let _ = tokio::task::spawn_blocking(move || work.follow_chat(&client, &session, term)).await;
        }
        "chat-term" => {
            {
                let mut state = c.state.lock().unwrap();
                if state.chat.is_none() {
                    return;
                }
                state.chat_term = optional(&m, "termId");
                state.last_screen.clear();
            }
            remote.push_screen(c);
        }
        "unchat" => {
            let mut state = c.state.lock().unwrap();
            state.chat = None;
            state.chat_term = None;
            state.chat_turn += 1;
        }
        "watch" => {
            let previous = c.state.lock().unwrap().watch.take();
            if let Some(previous) = previous {
                remote.release(c, &previous);
            }
            let Some((_, term)) = remote.find(&term_id) else {
                c.send(&json!({ "t": "exit", "termId": term_id, "code": null }));
                return;
            };
            c.state.lock().unwrap().watch = Some(term_id.clone());
            if truthy(&m, "cols") && truthy(&m, "rows") {
                remote.fit_to_phone(c, &term_id, &m);
            }
            // After a resize the shell redraws: send the screen at once, and again once it has.
            c.send(&screen_text(&term_id, &term));
            let c = c.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(400)).await;
                if c.state.lock().unwrap().watch.as_deref() == Some(term_id.as_str()) {
                    c.send(&screen_text(&term_id, &term));
                }
            });
        }
        "unwatch" => {
            let previous = c.state.lock().unwrap().watch.take();
            if let Some(previous) = previous {
                remote.release(c, &previous);
            }
        }
        "chip-menu" => {
            let (remote, c) = (remote.clone(), c.clone());
            tokio::spawn(async move {
                let menu = remote.ask_window("menu", text(&m, "sessionId"), text(&m, "kind"), 0, 0, Duration::from_secs(20)).await;
                c.send(&json!({ "t": "chip-menu", "reqId": m.get("reqId"), "menu": menu }));
            });
        }
        "chip-pick" => {
            let (remote, c) = (remote.clone(), c.clone());
            tokio::spawn(async move {
                let menu = m.get("menuId").and_then(Value::as_u64).unwrap_or(0);
                let index = m.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                remote.ask_window("pick", "", "", menu, index, Duration::from_secs(20)).await;
                // The pick takes a moment in the agent (a picker, a command): show the new chips after.
                let session = text(&m, "sessionId").to_string();
                for ms in [800, 2200] {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    remote.send_chips(&c, &session).await;
                }
            });
        }
        "answer" => {
            if let Some(id) = m.get("id").and_then(Value::as_u64) {
                service.answer_from_phone(id, m.get("answer").and_then(phone_answer));
            }
        }
        "stop-run" => service.stop_run(text(&m, "id")),
        "upload" => {
            let paths = remote.paths.clone();
            let req = m.get("reqId").cloned();
            let saved = tokio::task::spawn_blocking(move || save_upload(&paths, text(&m, "name"), text(&m, "data"))).await;
            match saved {
                Ok(Ok(file)) => c.send(&json!({ "t": "uploaded", "reqId": req, "path": file })),
                Ok(Err(why)) => c.send(&json!({ "t": "uploaded", "reqId": req, "error": why })),
                Err(_) => {}
            }
        }
        _ => {}
    }
}

async fn send_state(service: &Service, c: &Arc<Client>) {
    let (work, client) = (service.clone(), c.clone());
    let _ = tokio::task::spawn_blocking(move || client.send(&work.0.remote.state_message(&work))).await;
}

// ---------- the server ----------

fn page_response(request: &str) -> Vec<u8> {
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split(['?', '#']).next().unwrap_or("/").trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match PAGE.iter().find(|(name, _, _)| *name == path) {
        Some((_, kind, body)) => {
            let mut out = format!("HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).into_bytes();
            if !request.starts_with("HEAD") {
                out.extend_from_slice(body);
            }
            out
        }
        None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
    }
}

/// One port serves both: the page, and the phones' WebSocket.
async fn connection(service: Service, mut stream: TcpStream, stop: watch::Receiver<bool>) {
    let mut head = [0u8; 8192];
    let read_head = async {
        loop {
            let n = stream.peek(&mut head).await.ok()?;
            if n == 0 {
                return None;
            }
            if let Some(at) = head[..n].windows(4).position(|w| w == b"\r\n\r\n") {
                return Some(at + 4);
            }
            if n == head.len() {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    };
    let Ok(Some(end)) = tokio::time::timeout(Duration::from_secs(10), read_head).await else { return };
    let request = String::from_utf8_lossy(&head[..end]).to_string();
    let upgrade = request.lines().any(|l| {
        let l = l.to_lowercase();
        l.starts_with("upgrade:") && l.contains("websocket")
    });
    if upgrade {
        return socket(service, stream, stop).await;
    }
    let mut taken = vec![0u8; end];
    if stream.read_exact(&mut taken).await.is_ok() {
        let _ = stream.write_all(&page_response(&request)).await;
        let _ = stream.shutdown().await;
    }
}

async fn socket(service: Service, stream: TcpStream, mut stop: watch::Receiver<bool>) {
    let config = WebSocketConfig::default().max_message_size(Some(MESSAGE_MAX)).max_frame_size(Some(MESSAGE_MAX));
    let Ok(ws) = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await else { return };
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let remote = service.0.remote.clone();
    let c = Arc::new(Client { tx, closed: Notify::new(), ready: AtomicBool::new(false), device: Mutex::default(), state: Mutex::default() });
    remote.clients.lock().unwrap().push(c.clone());

    let writer_client = c.clone();
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let closing = matches!(message, Message::Close(_));
            if sink.send(message).await.is_err() || closing {
                break;
            }
        }
        let _ = sink.close().await;
        writer_client.closed.notify_one();
    });
    let hello_client = c.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        if !hello_client.ready.load(Ordering::Acquire) {
            hello_client.close();
        }
    });

    loop {
        tokio::select! {
            message = source.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(m) = serde_json::from_str::<Value>(text.as_str()) {
                        handle(&service, &c, m).await;
                    }
                }
                Some(Ok(Message::Binary(bytes))) => {
                    if let Ok(m) = serde_json::from_slice::<Value>(&bytes) {
                        handle(&service, &c, m).await;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            () = c.closed.notified() => break,
            _ = stop.changed() => {
                c.close();
                break;
            }
        }
    }

    remote.clients.lock().unwrap().retain(|x| !Arc::ptr_eq(x, &c));
    let sized: Vec<String> = c.state.lock().unwrap().sized.iter().cloned().collect();
    for id in sized {
        remote.release(&c, &id);
    }
    remote.changed();
}

async fn tick(service: Service, mut stop: watch::Receiver<bool>) {
    let remote = service.0.remote.clone();
    let (mut swept, mut watched, mut chatted) = (Instant::now(), Instant::now(), Instant::now());
    let polling = Arc::new(AtomicBool::new(false));
    let stating = Arc::new(AtomicBool::new(false));
    loop {
        let any = !remote.clients.lock().unwrap().is_empty();
        tokio::select! {
            () = tokio::time::sleep(if any { OUT_EVERY } else { Duration::from_millis(250) }) => {}
            _ = stop.changed() => return,
        }
        remote.flush_out();
        remote.sweep_activity();
        if swept.elapsed() >= Duration::from_millis(250) {
            swept = Instant::now();
            remote.sweep_terminals();
        }
        if any && watched.elapsed() >= WATCH_EVERY {
            watched = Instant::now();
            remote.push_watched();
        }
        if any && chatted.elapsed() >= CHAT_EVERY && !polling.swap(true, Ordering::AcqRel) {
            chatted = Instant::now();
            let (work, flag) = (remote.clone(), polling.clone());
            tokio::task::spawn_blocking(move || {
                work.poll_chats();
                flag.store(false, Ordering::Release);
            });
        }
        let due = {
            let mut state_due = remote.state_due.lock().unwrap();
            let ready = state_due.is_some_and(|at| at <= Instant::now());
            if ready {
                *state_due = None;
            }
            ready
        };
        if due && any && !stating.swap(true, Ordering::AcqRel) {
            let (work, flag) = (service.clone(), stating.clone());
            tokio::task::spawn_blocking(move || {
                let message = work.0.remote.state_message(&work);
                work.0.remote.broadcast(&message);
                flag.store(false, Ordering::Release);
            });
        }
    }
}

async fn serve(service: Service, port: u16, mut stop: watch::Receiver<bool>) {
    let remote = service.0.remote.clone();
    // After a stop the old listener may take a moment to let go of the port.
    let mut bound = None;
    for attempt in 0..10 {
        match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => {
                bound = Some(Ok(listener));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempt < 9 => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if *stop.borrow() {
                    return;
                }
            }
            Err(e) => {
                bound = Some(Err(e));
                break;
            }
        }
    }
    let listener = match bound {
        Some(Ok(listener)) => listener,
        Some(Err(e)) => {
            *remote.error.lock().unwrap() = if e.kind() == std::io::ErrorKind::AddrInUse { format!("Port {port} is in use. Choose another one.") } else { e.to_string() };
            remote.stop();
            remote.changed();
            return;
        }
        None => return,
    };
    remote.running.store(true, Ordering::Release);
    remote.changed();
    tokio::spawn(tick(service.clone(), stop.clone()));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    tokio::spawn(connection(service.clone(), stream, stop.clone()));
                }
            }
            _ = stop.changed() => return,
        }
    }
}

impl Remote {
    fn stop(&self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(true);
        }
        self.running.store(false, Ordering::Release);
    }
}

impl Service {
    pub(crate) fn remote_start(&self) {
        let remote = &self.0.remote;
        let mut stop = remote.stop.lock().unwrap();
        if stop.is_some() {
            return;
        }
        let (tx, rx) = watch::channel(false);
        *stop = Some(tx);
        remote.error.lock().unwrap().clear();
        self.0.rt.spawn(serve(self.clone(), remote.port(), rx));
    }

    pub(crate) fn remote_shutdown(&self) {
        self.0.remote.stop();
    }

    pub(crate) fn remote_on_prompt(&self, p: &Prompt) {
        self.0.remote.on_prompt(p);
    }

    pub(crate) fn remote_on_prompt_done(&self, id: u64) {
        self.0.remote.on_prompt_done(id);
    }

    pub(crate) fn remote_on_run(&self, r: &PublicRun) {
        self.0.remote.on_run(r);
    }

    /// Lets phones see a new terminal, from now until it ends or its pane closes.
    pub fn remote_terminal(&self, session_id: &str, agent: &str, cwd: &str, term: &Arc<Terminal>) {
        let remote = self.0.remote.clone();
        let entry = Arc::new(Term {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: Mutex::new(session_id.to_string()),
            agent: agent.to_string(),
            cwd: cwd.to_string(),
            term: Arc::downgrade(term),
            out: Mutex::default(),
            activity: Mutex::default(),
            dirty: AtomicBool::new(false),
        });
        remote.terms.lock().unwrap().push(entry.clone());
        let tap_remote = remote.clone();
        term.set_tap(Arc::new(move |bytes| tap_remote.on_output(&entry, bytes)));
        remote.state_changed();
    }

    /// A new session's id is only known once its file appears.
    pub fn remote_session_changed(&self, from: &str, to: &str) {
        let remote = &self.0.remote;
        for t in remote.terms.lock().unwrap().iter() {
            let mut session = t.session_id.lock().unwrap();
            if *session == from {
                *session = to.to_string();
            }
        }
        remote.state_changed();
    }

    /// The window's answer to a [`RemoteAsk`].
    pub fn remote_answer(&self, id: u64, answer: Value) {
        if let Some(tx) = self.0.remote.asks.lock().unwrap().remove(&id) {
            let _ = tx.send(answer);
        }
    }

    pub fn remote_info(&self) -> RemoteInfo {
        let remote = &self.0.remote;
        let config = Config::load(&remote.paths);
        let enabled = config.bool("remoteEnabled");
        let port = remote.port();
        let addresses = addresses();
        let (mut code, mut pair_url) = (String::new(), String::new());
        if enabled {
            code = remote.token();
            let pair = json!({ "v": 1, "name": hostname(), "port": port, "token": code, "hosts": addresses.iter().map(|a| a.address.as_str()).collect::<Vec<_>>() });
            let first = addresses.first().map_or("127.0.0.1", |a| a.address.as_str());
            pair_url = format!("http://{first}:{port}/?d={}", URL_SAFE_NO_PAD.encode(pair.to_string()));
        }
        RemoteInfo {
            enabled,
            running: remote.running.load(Ordering::Acquire),
            port,
            addresses,
            error: remote.error.lock().unwrap().clone(),
            devices: remote.all_clients().iter().filter(|c| c.ready.load(Ordering::Acquire)).map(|c| c.device.lock().unwrap().clone()).collect(),
            code,
            pair_url,
        }
    }

    /// Turns phone access on or off, or makes a new pairing code. Any of them drops the paired phones.
    pub fn remote_set(&self, enabled: Option<bool>, renew_token: bool) -> RemoteInfo {
        let remote = &self.0.remote;
        let mut config = Config::load(&remote.paths);
        if let Some(on) = enabled {
            config.set("remoteEnabled", json!(on));
        }
        if renew_token {
            config.set("remoteToken", json!(new_token()));
        }
        let _ = config.save(&remote.paths);
        remote.stop();
        remote.error.lock().unwrap().clear();
        if config.bool("remoteEnabled") {
            self.remote_start();
        }
        self.remote_info()
    }

    /// What this side saves in config.json, for the window's own copy of it.
    pub fn remote_settings(&self) -> Map<String, Value> {
        let config = Config::load(&self.0.remote.paths);
        ["remoteEnabled", "remoteToken", "remotePort"].iter().filter_map(|k| config.raw().get(*k).map(|v| ((*k).to_string(), v.clone()))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|l| (*l).to_string()).collect()
    }

    #[test]
    fn reads_the_working_line_and_live_block() {
        let screen = lines(&["old", "⏺ Writing the file", "  second line", "    deeper", "", "✻ Churning… (12s · ↓ 1.2k tokens · esc to interrupt)", "> "]);
        assert_eq!(working_line(&screen), "Churning… (12s · ↓ 1.2k tokens)");
        assert_eq!(live_block(&screen), "Writing the file\nsecond line\n  deeper");
        assert_eq!(working_line(&lines(&["✻ Thinking… (esc to cancel)"])), "Thinking…");
        assert_eq!(working_line(&lines(&["nothing"])), "");
        assert_eq!(live_block(&lines(&["no bullet", "esc to interrupt"])), "");
    }

    #[test]
    fn reads_a_question() {
        let screen = lines(&["│ Do you want to proceed? │", "│ ❯ 1. Yes                │", "│   2. No, and tell Claude   (esc) │"]);
        let ask = ask_of(&screen);
        assert_eq!(ask["question"], "Do you want to proceed?");
        assert_eq!(ask["options"][0]["n"], 1);
        assert_eq!(ask["options"][0]["label"], "Yes");
        assert_eq!(ask["options"][1]["label"], "No, and tell Claude");
        // A numbered list in a reply has no highlight.
        assert!(ask_of(&lines(&["1. one", "2. two"])).is_null());
        assert!(ask_of(&lines(&["❯ 2. two", "3. three"])).is_null());
    }

    #[test]
    fn decodes_characters_cut_between_reads() {
        let mut partial = Vec::new();
        let bytes = "好a".as_bytes();
        assert_eq!(decode(&mut partial, &bytes[..2]), "");
        assert_eq!(decode(&mut partial, &bytes[2..]), "好a");
        assert_eq!(decode(&mut partial, &[b'x', 0xff, b'y']), "x\u{fffd}y");
    }

    #[test]
    fn maps_answers() {
        assert_eq!(phone_answer(&json!("trust")), Some(Answer::Trust));
        assert_eq!(phone_answer(&json!("once")), Some(Answer::Once));
        assert_eq!(phone_answer(&json!("host")), Some(Answer::AllowHost));
        assert_eq!(phone_answer(&json!(null)), None);
        assert_eq!(phone_answer(&json!(["a", "b"])), Some(Answer::Keyboard(vec!["a".into(), "b".into()])));
        assert_eq!(phone_answer(&json!({"value": "x", "remember": true})), Some(Answer::Secret { value: "x".into(), remember: true }));
    }

    #[test]
    fn serves_only_the_page() {
        let ok = page_response("GET / HTTP/1.1\r\n\r\n");
        assert!(ok.starts_with(b"HTTP/1.1 200"));
        assert!(page_response("GET /app.js?v=1 HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 200"));
        assert!(page_response("GET /../config.json HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 404"));
        assert!(page_response("GET /vendor/../../remote.js HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 404"));
    }

    #[test]
    fn compares_tokens() {
        assert!(same_token("abc", "abc"));
        assert!(!same_token("abc", "abd"));
        assert!(!same_token("", "abc"));
        assert_eq!(new_token().len(), 24);
    }
}
