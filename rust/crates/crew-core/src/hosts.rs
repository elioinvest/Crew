//! The SSH hosts, in hosts.json. Secrets are sealed (secrets.rs) and never leave
//! this side of the app: the agents reach hosts by name, through crew-ssh.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Paths;
use crate::secrets;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    /// "auto", "key", "password" or "agent".
    pub auth: String,
    pub key_path: String,
    pub group: String,
    /// Instructions for agents.
    pub notes: String,
    /// "changes", "all" or "auto".
    pub approval: String,
    pub secret: Secret,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Keys this version doesn't know are kept.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Host {
    /// As the sidebar shows it: the port only when it isn't SSH's own.
    pub fn address(&self) -> String {
        let user = if self.user.is_empty() { String::new() } else { format!("{}@", self.user) };
        match self.port {
            Some(p) if p != 22 => format!("{user}{}:{p}", self.host),
            _ => format!("{user}{}", self.host),
        }
    }

    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.address())
    }

    pub fn port(&self) -> u16 {
        self.port.filter(|p| *p != 0).unwrap_or(22)
    }

    pub fn has_password(&self) -> bool {
        self.secret.password.is_some()
    }

    pub fn has_passphrase(&self) -> bool {
        self.secret.passphrase.is_some()
    }
}

/// What the host editor saves. Empty secrets leave the saved ones as they are.
#[derive(Clone, Debug, Default)]
pub struct HostInput {
    pub id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub auth: String,
    pub key_path: String,
    pub group: String,
    pub notes: String,
    pub approval: String,
    pub password: String,
    pub passphrase: String,
    pub clear_password: bool,
    pub clear_passphrase: bool,
}

fn file(paths: &Paths) -> PathBuf {
    paths.data_dir.join("hosts.json")
}

/// In the file's order.
pub fn load(paths: &Paths) -> Vec<Host> {
    let Ok(text) = std::fs::read_to_string(file(paths)) else { return Vec::new() };
    serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default()
}

pub fn save_all(paths: &Paths, hosts: &[Host]) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.data_dir)?;
    let tmp = file(paths).with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(hosts)?)?;
    std::fs::rename(&tmp, file(paths))?;
    Ok(())
}

/// For the sidebar: grouped hosts last, each group by name.
pub fn list(paths: &Paths) -> Vec<Host> {
    let mut hosts = load(paths);
    hosts.sort_by(|a, b| (!a.group.is_empty(), &a.group, &a.name).cmp(&(!b.group.is_empty(), &b.group, &b.name)));
    hosts
}

pub fn save(paths: &Paths, input: &HostInput) -> anyhow::Result<Host> {
    let mut hosts = load(paths);
    let at = input.id.as_ref().and_then(|id| hosts.iter().position(|h| &h.id == id));
    let mut h = at.map(|i| hosts[i].clone()).unwrap_or_else(|| Host { id: uuid::Uuid::new_v4().to_string(), ..Host::default() });
    h.name = input.name.trim().to_string();
    h.host = input.host.trim().to_string();
    h.port = Some(input.port.trim().parse().ok().filter(|p| *p != 0).unwrap_or(22));
    h.user = input.user.trim().to_string();
    h.auth = input.auth.trim().to_string();
    h.key_path = input.key_path.trim().to_string();
    h.group = input.group.trim().to_string();
    h.notes = input.notes.trim().to_string();
    h.approval = input.approval.trim().to_string();
    if h.name.is_empty() {
        h.name = h.host.clone();
    }
    if !input.password.is_empty() {
        h.secret.password = secrets::seal(paths, &input.password);
    }
    if input.clear_password {
        h.secret.password = None;
    }
    if !input.passphrase.is_empty() {
        h.secret.passphrase = secrets::seal(paths, &input.passphrase);
    }
    if input.clear_passphrase {
        h.secret.passphrase = None;
    }
    match at {
        Some(i) => hosts[i] = h.clone(),
        None => hosts.push(h.clone()),
    }
    save_all(paths, &hosts)?;
    Ok(h)
}

pub fn delete(paths: &Paths, id: &str) -> anyhow::Result<()> {
    let mut hosts = load(paths);
    hosts.retain(|h| h.id != id);
    save_all(paths, &hosts)
}

/// Saves a secret typed at a prompt with "Remember it".
pub fn remember(paths: &Paths, id: &str, password: Option<&str>, passphrase: Option<&str>) {
    let mut hosts = load(paths);
    let Some(h) = hosts.iter_mut().find(|h| h.id == id) else { return };
    if let Some(p) = password {
        h.secret.password = secrets::seal(paths, p);
    }
    if let Some(p) = passphrase {
        h.secret.passphrase = secrets::seal(paths, p);
    }
    let _ = save_all(paths, &hosts);
}

/// A host by id or name (exact first, then ignoring case). Ambiguous names are an error.
pub fn find(paths: &Paths, reference: &str) -> Result<Host, String> {
    let list = load(paths);
    if let Some(h) = list.iter().find(|h| h.id == reference) {
        return Ok(h.clone());
    }
    let mut named: Vec<&Host> = list.iter().filter(|h| h.name == reference).collect();
    if named.is_empty() {
        named = list.iter().filter(|h| h.name.to_lowercase() == reference.to_lowercase()).collect();
    }
    match named.as_slice() {
        [h] => Ok((*h).clone()),
        [] => {
            let names = list.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(", ");
            Err(format!("No saved host named \"{reference}\". Saved hosts: {}. Add it in Crew (Hosts in the sidebar).", if names.is_empty() { "none" } else { &names }))
        }
        many => {
            let names = many.iter().map(|h| format!("{} [{}]", h.name, h.id)).collect::<Vec<_>>().join(", ");
            Err(format!("Several saved hosts are named \"{reference}\": {names}. Use the id."))
        }
    }
}

/// The folder a host's agent sessions run in, so they list under the host and not as a project. Its
/// path is fixed when first needed, so renaming the host keeps its sessions.
pub fn workspace_path(paths: &Paths, h: &Host) -> PathBuf {
    if let Some(w) = h.workspace.as_deref().filter(|w| !w.is_empty()) {
        return PathBuf::from(w);
    }
    static UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^A-Za-z0-9_.-]+").unwrap());
    let base = if h.name.is_empty() { &h.host } else { &h.name };
    let safe = UNSAFE.replace_all(base, "-");
    let safe: String = safe.trim_matches('-').chars().take(40).collect();
    let safe = if safe.is_empty() { "host".to_string() } else { safe };
    let short: String = h.id.chars().take(8).collect();
    paths.data_dir.join("host-workspaces").join(format!("{safe}-{short}"))
}

/// Fixes the workspace path in hosts.json and refreshes its CLAUDE.md / AGENTS.md, which tell Claude
/// Code and Codex which machine they're managing and how.
pub fn write_workspace(paths: &Paths, h: &Host) -> anyhow::Result<PathBuf> {
    let dir = workspace_path(paths, h);
    if h.workspace.as_deref().unwrap_or("").is_empty() {
        let mut hosts = load(paths);
        if let Some(saved) = hosts.iter_mut().find(|x| x.id == h.id) {
            saved.workspace = Some(dir.to_string_lossy().to_string());
            save_all(paths, &hosts)?;
        }
    }
    std::fs::create_dir_all(&dir)?;
    let text = workspace_text(h);
    for f in ["CLAUDE.md", "AGENTS.md"] {
        std::fs::write(dir.join(f), &text)?;
    }
    Ok(dir)
}

fn workspace_text(h: &Host) -> String {
    let (name, addr) = (&h.name, h.address());
    let notes = if h.notes.trim().is_empty() { "None yet." } else { h.notes.trim() };
    format!(
        r#"# Remote host: {name}

You're in Crew's workspace for the remote machine **{name}** ({addr}). The user's terminal on it is open beside this chat, and you share it with them. Work on that machine, not in this folder.

- Read the shared terminal: `crew-ssh screen {name}` (`--lines N`; `--wait S` first waits up to S seconds for new output to come and go quiet). Read it when the user refers to something on their screen ("look at this error").
- Type into the shared terminal: `crew-ssh send {name} --keys enter "<command>"`. It types the text, presses the keys (`enter`, `ctrl+c`, `tab`, `up`, `esc`, `q`, …), waits until the output is quiet for a second (`--wait S`, default 10) and prints what appeared. The user sees it happen. Use it for interactive work: answering a prompt (`--keys y,enter`), REPLs, `top`, editors, long-running commands to watch, stopping something (`--keys ctrl+c`), or continuing where the user is (same directory, environment, `sudo -s` shell).
- Run a command on its own: `crew-ssh exec {name} "<command>"` (add `--cwd DIR` before the command). It has no terminal and no input, so use non-interactive forms (`--no-pager`, `-y`, `DEBIAN_FRONTEND=noninteractive`) and keep output short (`| tail -n 100`). It's best for checks whose full output you need, and doesn't disturb the user's terminal.
- Copy files: `crew-ssh cp <local> {name}:<remote>` or `crew-ssh cp {name}:<remote> <local>`.

Put the command or text in one quoted argument after the options, with no `--` before it (Codex runs a line with a bare `--` in its sandbox, where crew-ssh can't reach Crew). A quiet second doesn't mean a command finished: for more, read with `crew-ssh screen {name} --wait 30`. Before typing, read the screen if you're unsure what's on it (the user may be typing, or a program may be open). Don't type over a line the user is writing.

Crew holds the credentials: never ask for passwords or keys, and don't use `ssh`, `scp` or `sftp` directly. When the terminal asks for a password, a passphrase or a code (e.g. after `sudo`), Crew won't let you type it: ask the user to type it in the terminal, then continue with `crew-ssh screen {name} --wait 10`.

What only reads runs at once. Anything that may change the machine, typed or run, waits until the user approves it in Crew: say in one line what you'll do and why, and give crew-ssh a long timeout (10 minutes). If the user denies something, don't do it again or work around it; ask.

## The user's notes about {name}

{notes}
"#
    )
}

/// Hosts from ~/.ssh/config: one per Host alias without wildcards.
pub fn parse_ssh_config(text: &str) -> Vec<Host> {
    static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#.*").unwrap());
    static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\S+)\s*=?\s*(.+)$").unwrap());
    let mut out: Vec<Host> = Vec::new();
    let mut current: std::ops::Range<usize> = 0..0;
    for raw in text.lines() {
        let line = COMMENT.replace(raw, "");
        let line = line.trim();
        let Some(m) = LINE.captures(line) else { continue };
        let key = m[1].to_lowercase();
        let value = m[2].trim().trim_matches('"').to_string();
        if key == "host" || key == "match" {
            let start = out.len();
            if key == "host" {
                for alias in value.split_whitespace().filter(|a| !a.contains(['*', '?', '!'])) {
                    out.push(Host { name: alias.into(), host: alias.into(), ..Host::default() });
                }
            }
            current = start..out.len();
        } else {
            for h in &mut out[current.clone()] {
                match key.as_str() {
                    "hostname" => h.host = value.clone(),
                    "user" => h.user = value.clone(),
                    "port" => h.port = value.parse().ok(),
                    "identityfile" if h.key_path.is_empty() => h.key_path = value.clone(),
                    _ => {}
                }
            }
        }
    }
    out
}

/// Adds the hosts of ~/.ssh/config not already saved. None when there's no such file.
pub fn import_ssh_config(paths: &Paths) -> Option<usize> {
    let text = std::fs::read_to_string(paths.home.join(".ssh").join("config")).ok()?;
    let mut hosts = load(paths);
    let mut added = 0;
    for mut c in parse_ssh_config(&text) {
        if hosts.iter().any(|h| h.name.to_lowercase() == c.name.to_lowercase()) {
            continue;
        }
        c.id = uuid::Uuid::new_v4().to_string();
        c.auth = if c.key_path.is_empty() { "auto" } else { "key" }.into();
        c.approval = "changes".into();
        c.group = "ssh config".into();
        c.port = Some(c.port.unwrap_or(22));
        hosts.push(c);
        added += 1;
    }
    if added > 0 {
        let _ = save_all(paths, &hosts);
    }
    Some(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> Paths {
        let d = std::env::temp_dir().join(format!("crew-hosts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        Paths { home: d.clone(), data_dir: d.clone(), codex_home: d.join(".codex") }
    }

    #[test]
    fn workspace_names_match_the_original() {
        let paths = scratch();
        let h = Host { id: "abcdefgh-1234".into(), name: "JP 拾云1服".into(), ..Host::default() };
        assert!(workspace_path(&paths, &h).ends_with("JP-1-abcdefgh"));
    }

    #[test]
    fn formats_the_address_and_keeps_unknown_keys_and_secrets() {
        let paths = scratch();
        std::fs::write(
            paths.data_dir.join("hosts.json"),
            r#"[{"id":"2","name":"b","host":"h2","user":"root","port":43015,"auth":"password","secret":{"password":"sealed"},"future":7},{"id":"1","name":"a","host":"h1","port":22}]"#,
        )
        .unwrap();
        let hosts = list(&paths);
        assert_eq!(hosts[0].address(), "h1");
        assert_eq!(hosts[1].address(), "root@h2:43015");
        save_all(&paths, &load(&paths)).unwrap();
        let again = load(&paths);
        assert_eq!(again[0].secret.password.as_deref(), Some("sealed"));
        assert_eq!(again[0].extra.get("future"), Some(&serde_json::json!(7)));
    }

    #[test]
    fn finds_by_id_and_name_and_reports_ambiguity() {
        let paths = scratch();
        let host = |id: &str, name: &str| Host { id: id.into(), name: name.into(), host: "x".into(), ..Host::default() };
        save_all(&paths, &[host("1", "Web"), host("2", "db"), host("3", "DB")]).unwrap();
        assert_eq!(find(&paths, "1").unwrap().name, "Web");
        assert_eq!(find(&paths, "web").unwrap().id, "1");
        assert_eq!(find(&paths, "db").unwrap().id, "2");
        assert!(find(&paths, "Db").unwrap_err().contains("Several"));
        assert!(find(&paths, "nope").unwrap_err().contains("No saved host"));
    }

    #[test]
    fn saving_keeps_a_secret_unless_replaced_or_forgotten() {
        let paths = scratch();
        let input = HostInput { name: "a".into(), host: "h".into(), port: "2222".into(), auth: "password".into(), approval: "changes".into(), ..HostInput::default() };
        let saved = save(&paths, &input).unwrap();
        assert_eq!(saved.port, Some(2222));
        let mut hosts = load(&paths);
        hosts[0].secret.password = Some("sealed".into());
        save_all(&paths, &hosts).unwrap();
        let edit = HostInput { id: Some(saved.id.clone()), ..input.clone() };
        assert!(save(&paths, &edit).unwrap().has_password());
        let forget = HostInput { id: Some(saved.id.clone()), clear_password: true, ..input };
        assert!(!save(&paths, &forget).unwrap().has_password());
    }

    #[test]
    fn reads_ssh_config() {
        let hosts = parse_ssh_config("Host *\n  User all\nHost web web2 # two\n  HostName example.com\n  User root\n  Port 2222\n  IdentityFile ~/.ssh/a\nMatch all\n  User x\n");
        assert_eq!(hosts.len(), 2);
        assert_eq!((hosts[0].name.as_str(), hosts[0].host.as_str(), hosts[0].user.as_str()), ("web", "example.com", "root"));
        assert_eq!((hosts[1].port, hosts[1].key_path.as_str()), (Some(2222), "~/.ssh/a"));
    }

    #[test]
    fn the_workspace_tells_the_agents_about_the_host() {
        let paths = scratch();
        let h = Host { id: "abcdefgh-1".into(), name: "web 1".into(), host: "h".into(), notes: "Careful.".into(), ..Host::default() };
        save_all(&paths, std::slice::from_ref(&h)).unwrap();
        let dir = write_workspace(&paths, &h).unwrap();
        assert!(dir.ends_with("web-1-abcdefgh"));
        let text = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert!(text.contains("crew-ssh screen web 1") && text.contains("Careful."));
        assert_eq!(load(&paths)[0].workspace.as_deref(), Some(dir.to_string_lossy().as_ref()));
    }
}
