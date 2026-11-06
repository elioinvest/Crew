//! What runs behind the window: SSH connections, the pipe the agents' `crew-ssh` and `crew` commands
//! talk to, approvals, and agent runs. Secrets stay here; the agents only ever use hosts by name.

pub mod bridge;
pub mod install;
pub mod remote;
pub mod rules;
pub mod runs;
pub mod ssh;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crew_core::Paths;
use tokio::sync::oneshot;

use crate::runs::PublicRun;

/// What Crew asks while something waits.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptKind {
    HostKey { host: String, address: String, key_type: String, fingerprint: String },
    HostKeyChanged { host: String, address: String, key_type: String, fingerprint: String, previous: String },
    Password { host: String },
    Passphrase { host: String, key_file: String },
    Keyboard { host: String, instructions: String, prompts: Vec<(String, bool)> },
    Approve { host_id: String, host: String, action: String, command: String, cwd: String, session: String, agent: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub id: u64,
    pub kind: PromptKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Trust,
    Secret { value: String, remember: bool },
    Keyboard(Vec<String>),
    Once,
    AllowHost,
}

#[derive(Clone, Debug)]
pub enum Event {
    HostsChanged,
    Prompt(Prompt),
    PromptCancelled(u64),
    Activity(String),
    Run(PublicRun),
    RemoteChanged,
    RemoteOpen(remote::RemoteOpen),
    RemoteAsk(remote::RemoteAsk),
}

struct Inner {
    paths: Paths,
    rt: tokio::runtime::Runtime,
    events: async_channel::Sender<Event>,
    prompts: Mutex<HashMap<u64, oneshot::Sender<Option<Answer>>>>,
    prompt_seq: AtomicU64,
    ssh: ssh::SshState,
    runs: runs::Runs,
    bridge: bridge::BridgeInfo,
    remote: Arc<remote::Remote>,
}

#[derive(Clone)]
pub struct Service(Arc<Inner>);

impl Service {
    /// Starts the pipe and installs the commands sessions use. Prompts and changes arrive on the receiver.
    pub fn start(paths: Paths) -> (Self, async_channel::Receiver<Event>) {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("crew-service").build().expect("couldn't start the service runtime");
        let (tx, rx) = async_channel::unbounded();
        let service = Self(Arc::new(Inner {
            bridge: bridge::BridgeInfo::fresh(),
            remote: Arc::new(remote::Remote::new(paths.clone(), tx.clone())),
            paths,
            rt,
            events: tx,
            prompts: Mutex::default(),
            prompt_seq: AtomicU64::new(0),
            ssh: ssh::SshState::default(),
            runs: runs::Runs::default(),
        }));
        install::install_cli(&service.0.paths, &service.0.bridge);
        install::refresh_skill(&service.0.paths);
        install::write_codex_rules(&service.0.paths, false);
        bridge::start(&service);
        ssh::start_idle_sweep(&service);
        if crew_core::config::Config::load(&service.0.paths).bool("remoteEnabled") {
            service.remote_start();
        }
        (service, rx)
    }

    pub fn paths(&self) -> &Paths {
        &self.0.paths
    }

    fn send(&self, event: Event) {
        let _ = self.0.events.try_send(event);
    }

    /// Asks in the window. The caller gets the id, to take the question back if nobody's waiting any more.
    fn ask(&self, kind: PromptKind) -> (u64, oneshot::Receiver<Option<Answer>>) {
        let id = self.0.prompt_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.0.prompts.lock().unwrap().insert(id, tx);
        let prompt = Prompt { id, kind };
        self.remote_on_prompt(&prompt);
        self.send(Event::Prompt(prompt));
        (id, rx)
    }

    fn cancel_prompt(&self, id: u64) {
        if let Some(tx) = self.0.prompts.lock().unwrap().remove(&id) {
            let _ = tx.send(None);
            self.remote_on_prompt_done(id);
            self.send(Event::PromptCancelled(id));
        }
    }

    /// The window's answer; None is "no".
    pub fn answer(&self, id: u64, answer: Option<Answer>) {
        if let Some(tx) = self.0.prompts.lock().unwrap().remove(&id) {
            let _ = tx.send(answer);
            self.remote_on_prompt_done(id);
        }
    }

    /// A phone's answer: the window takes its question down.
    fn answer_from_phone(&self, id: u64, answer: Option<Answer>) {
        if self.0.prompts.lock().unwrap().contains_key(&id) {
            self.answer(id, answer);
            self.send(Event::PromptCancelled(id));
        }
    }

    pub fn shutdown(&self) {
        self.remote_shutdown();
        self.stop_all_runs();
        self.disconnect_all();
    }

    /// Runs a future on the service's runtime from anywhere; for what the window starts.
    pub fn spawn_blocking<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> tokio::task::JoinHandle<T> {
        self.0.rt.spawn_blocking(work)
    }
}
