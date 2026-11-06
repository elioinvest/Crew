//! What the prompt box offers: after / and $ an agent's commands and skills, read from where Claude
//! Code and Codex keep them, and the model, effort and permission a session is on.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::{Value, json};

use crate::config::Paths;
use crate::sessions::ProviderHome;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub scope: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommand {
    pub name: String,
    pub description: String,
    pub scope: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Items {
    pub skills: Vec<Skill>,
    pub commands: Vec<AgentCommand>,
}

/// The agents' own commands. Custom commands and skills come from their folders.
pub fn builtin_commands(agent: &str) -> &'static [(&'static str, &'static str)] {
    if agent == "codex" {
        &[
            ("model", "Switch the model and reasoning effort"),
            ("approvals", "Choose what Codex may do without asking"),
            ("review", "Review the current changes"),
            ("new", "Start a new conversation"),
            ("resume", "Resume an earlier conversation"),
            ("compact", "Summarize the conversation to free up context"),
            ("diff", "Show the Git diff"),
            ("mention", "Mention a file"),
            ("init", "Write an AGENTS.md for this project"),
            ("mcp", "List MCP tools"),
            ("status", "Show the session's settings and usage"),
        ]
    } else {
        &[
            ("model", "Switch the model"),
            ("effort", "Set the reasoning effort"),
            ("compact", "Summarize the conversation to free up context"),
            ("clear", "Start over with an empty conversation"),
            ("context", "Show what fills the context window"),
            ("resume", "Resume an earlier conversation"),
            ("rewind", "Go back to an earlier point in the conversation"),
            ("review", "Review a pull request"),
            ("security-review", "Review the pending changes for security problems"),
            ("init", "Write a CLAUDE.md for this project"),
            ("memory", "Edit memory files"),
            ("permissions", "Allow or deny tools"),
            ("agents", "Manage subagents"),
            ("mcp", "Manage MCP servers"),
            ("plugin", "Manage plugins"),
            ("hooks", "Manage hooks"),
            ("add-dir", "Add a folder Claude can work in"),
            ("export", "Export the conversation"),
            ("usage", "Show plan usage limits"),
            ("cost", "Show this session's cost and duration"),
            ("status", "Show version, model and account"),
            ("config", "Open settings"),
            ("doctor", "Check the installation"),
            ("help", "Show help"),
        ]
    }
}

pub fn scope_label(scope: &str) -> &'static str {
    match scope {
        "personal" => "Personal",
        "project" => "Project",
        "plugin" => "Plugin",
        "system" | "admin" => "System",
        "builtin" => "Built-in",
        _ => "",
    }
}

// ---------- skills and commands on disk ----------

/// The YAML front matter's plain keys (name, description, …), enough for SKILL.md and commands.
fn front_matter(text: &str) -> BTreeMap<String, String> {
    static BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^---\r?\n((?s:.*?))\r?\n---").unwrap());
    static PAIR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([\w-]+):[ \t]*(.*)$").unwrap());
    let mut out = BTreeMap::new();
    let Some(m) = BLOCK.captures(text) else { return out };
    let lines: Vec<&str> = m[1].lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some(kv) = PAIR.captures(lines[i]) {
            let mut v = kv[2].trim().to_string();
            if matches!(v.as_str(), "" | "|" | ">" | ">-" | "|-") {
                // A block on the lines below.
                let mut block = Vec::new();
                while i + 1 < lines.len() && lines[i + 1].starts_with(char::is_whitespace) && !lines[i + 1].trim().is_empty() {
                    i += 1;
                    block.push(lines[i].trim());
                }
                v = block.join(" ");
            }
            let quoted = v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
            out.insert(kv[1].to_string(), if quoted { v[1..v.len() - 1].to_string() } else { v });
        }
        i += 1;
    }
    out
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
}

fn dirs(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect()
}

fn read_json(file: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

fn skill_at(dir: &Path, fallback: &str, prefix: &str, scope: &str) -> Option<Skill> {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let fm = front_matter(&text);
    if fm.get("user-invocable").is_some_and(|v| matches!(v.trim().to_lowercase().as_str(), "false" | "no" | "off" | "n" | "0")) {
        return None;
    }
    let chosen = if prefix.is_empty() { fm.get("name").map_or(fallback, String::as_str) } else { fallback };
    let base: String = chosen.chars().map(|c| if c.is_alphanumeric() || c == '_' || c == ':' || c == '-' { c } else { '-' }).collect();
    Some(Skill {
        name: if prefix.is_empty() { base } else { format!("{prefix}:{base}") },
        description: one_line(fm.get("description").map_or("", String::as_str)),
        scope: scope.to_string(),
    })
}

/// A commands/ folder: each .md is a command; subfolders namespace them (dir:name).
fn commands_in(root: &Path, prefix: &str, scope: &str, out: &mut Vec<AgentCommand>, rel: &str) {
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if e.path().is_dir() {
            if rel.split('/').count() < 3 {
                commands_in(root, prefix, scope, out, &r);
            }
            continue;
        }
        let Some(stem) = r.strip_suffix(".md").or_else(|| r.strip_suffix(".MD")) else { continue };
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        let fm = front_matter(&text);
        let body = if text.starts_with("---") { text.splitn(3, "---").nth(2).unwrap_or("") } else { text.as_str() };
        let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let plain = stem.replace('/', ":");
        out.push(AgentCommand {
            name: if prefix.is_empty() { plain } else { format!("{prefix}:{plain}") },
            description: one_line(fm.get("description").map_or(first, String::as_str)),
            scope: scope.to_string(),
        });
    }
}

pub fn claude_items(paths: &Paths, cwd: &Path, config_dir: Option<&Path>) -> Items {
    let home = config_dir.map_or_else(|| paths.home.join(".claude"), Path::to_path_buf);
    let mut skills: Vec<Skill> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |s: Option<Skill>, skills: &mut Vec<Skill>| {
        if let Some(s) = s {
            if seen.insert(s.name.clone()) {
                skills.push(s);
            }
        }
    };
    // User skills come first: Claude Code prefers them over a project's skill of the same name.
    for d in dirs(&home.join("skills")) {
        add(skill_at(&home.join("skills").join(&d), &d, "", "personal"), &mut skills);
    }
    let local = cwd.join(".claude");
    for d in dirs(&local.join("skills")) {
        add(skill_at(&local.join("skills").join(&d), &d, "", "project"), &mut skills);
    }
    let mut commands = Vec::new();
    commands_in(&home.join("commands"), "", "personal", &mut commands, "");
    commands_in(&local.join("commands"), "", "project", &mut commands, "");

    // Plugins: the ones turned on in settings, installed for everyone or for this project.
    let mut enabled = serde_json::Map::new();
    for f in [home.join("settings.json"), local.join("settings.json"), local.join("settings.local.json")] {
        if let Some(on) = read_json(&f).and_then(|s| s.get("enabledPlugins").and_then(Value::as_object).cloned()) {
            enabled.extend(on);
        }
    }
    let installed = read_json(&home.join("plugins").join("installed_plugins.json")).and_then(|v| v.get("plugins").and_then(Value::as_object).cloned()).unwrap_or_default();
    let here = crate::worktrees::norm(cwd);
    let mut keys: Vec<&String> = installed.keys().collect();
    keys.sort();
    for key in keys {
        if enabled.get(key) != Some(&Value::Bool(true)) {
            continue;
        }
        let Some(list) = installed[key].as_array() else { continue };
        let install = list.iter().find_map(|r| {
            let path = PathBuf::from(r.get("installPath")?.as_str()?);
            let scope = r.get("scope").and_then(Value::as_str).unwrap_or("");
            let for_here = r.get("projectPath").and_then(Value::as_str).is_some_and(|p| crate::worktrees::norm(Path::new(p)) == here);
            ((scope == "user" || scope == "managed" || for_here) && path.exists()).then_some(path)
        });
        let Some(install) = install else { continue };
        let manifest = read_json(&install.join(".claude-plugin").join("plugin.json")).unwrap_or(Value::Null);
        let prefix = manifest.get("name").and_then(Value::as_str).map_or_else(|| key.split('@').next().unwrap_or(key).to_string(), String::from);
        let mut folders = vec!["skills".to_string()];
        match manifest.get("skills") {
            Some(Value::String(s)) => folders.push(s.clone()),
            Some(Value::Array(a)) => folders.extend(a.iter().filter_map(|x| x.as_str().map(String::from))),
            _ => {}
        }
        for rel in folders {
            if Path::new(&rel).components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                continue;
            }
            let root = install.join(&rel);
            for d in dirs(&root) {
                add(skill_at(&root.join(&d), &d, &prefix, "plugin"), &mut skills);
            }
        }
        commands_in(&install.join("commands"), &prefix, "plugin", &mut commands, "");
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Items { skills, commands }
}

/// Asks a Codex app-server one question. None when it doesn't answer in time.
fn ask_app_server(command: &str, codex_home: Option<&Path>, method: &str, params: Value, wait: Duration) -> Option<Value> {
    use std::os::windows::process::CommandExt;
    let launch = crate::agents::shell_command(&[command.to_string(), "app-server".to_string()]);
    let mut env = crate::env::clean_env();
    env.extend(launch.env);
    if let Some(h) = codex_home {
        env.insert("CODEX_HOME".to_string(), h.to_string_lossy().to_string());
    }
    let mut child = std::process::Command::new(&launch.program)
        .args(&launch.args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
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
    let mut send = |m: Value| writeln!(stdin, "{m}").is_ok();
    send(json!({ "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "crew", "title": "Crew", "version": env!("CARGO_PKG_VERSION") }, "capabilities": { "experimentalApi": true } } }));
    let deadline = std::time::Instant::now() + wait;
    let mut answer = None;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(line) = rx.recv_timeout(left) else { break };
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        match m.get("id").and_then(Value::as_i64) {
            Some(1) => {
                send(json!({ "method": "initialized" }));
                send(json!({ "id": 2, "method": method, "params": params }));
            }
            Some(2) => {
                answer = m.get("result").cloned();
                break;
            }
            _ => {}
        }
    }
    // The server runs under cmd.exe: the whole tree has to go.
    let _ = std::process::Command::new("taskkill").args(["/pid", &child.id().to_string(), "/T", "/F"]).creation_flags(CREATE_NO_WINDOW).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let _ = child.wait();
    answer
}

/// Codex's skills for a folder come from Codex itself, so system and repo skills are included.
pub fn codex_items(paths: &Paths, command: &str, cwd: &Path, provider_home: Option<&Path>) -> Items {
    let home = provider_home.unwrap_or(&paths.codex_home);
    let mut skills: Option<Vec<Skill>> = ask_app_server(command, provider_home, "skills/list", json!({ "cwds": [cwd] }), Duration::from_secs(15)).and_then(|r| {
        let data = r.get("data")?.as_array()?.clone();
        let here = crate::worktrees::norm(cwd);
        let chosen: Vec<Value> = match data.iter().find(|x| x.get("cwd").and_then(Value::as_str).is_some_and(|c| crate::worktrees::norm(Path::new(c)) == here)) {
            Some(e) => e.get("skills")?.as_array()?.clone(),
            None => data.iter().filter_map(|x| x.get("skills")?.as_array().cloned()).flatten().collect(),
        };
        Some(
            chosen
                .iter()
                .filter(|s| s.get("enabled") != Some(&Value::Bool(false)))
                .filter_map(|s| {
                    let text = |k: &str| s.get(k).and_then(Value::as_str);
                    let scope = match text("scope").unwrap_or("") {
                        "user" => "personal",
                        "repo" => "project",
                        other => other,
                    };
                    let description = text("shortDescription").or_else(|| s.get("interface")?.get("shortDescription")?.as_str()).or_else(|| text("description")).unwrap_or("");
                    Some(Skill { name: text("name")?.to_string(), description: one_line(description), scope: scope.to_string() })
                })
                .collect(),
        )
    });
    let mut list = skills.take().unwrap_or_else(|| {
        let mut found = Vec::new();
        for d in dirs(&home.join("skills")) {
            found.extend(skill_at(&home.join("skills").join(&d), &d, "", "personal"));
        }
        for d in dirs(&cwd.join(".codex").join("skills")) {
            found.extend(skill_at(&cwd.join(".codex").join("skills").join(&d), &d, "", "project"));
        }
        found
    });
    list.sort_by(|a, b| a.name.cmp(&b.name));
    let mut commands = Vec::new();
    commands_in(&home.join("prompts"), "prompts", "personal", &mut commands, "");
    Items { skills: list, commands }
}

// ---------- what's being typed ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    Path,
    Command,
    Skill,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub kind: TriggerKind,
    /// Byte offset of the @, / or $.
    pub start: usize,
    pub query: String,
}

/// The token being typed at the caret, and what it asks for. A Chinese keyboard types ￥ for $.
pub fn trigger_at(text: &str, caret: usize) -> Option<Trigger> {
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[\s(（，,])([@/$￥])([^\s@$￥]*)$").unwrap());
    let before = text.get(..caret)?;
    let m = TOKEN.captures(before)?;
    let sign = m.get(1)?;
    let query = m[2].to_string();
    let start = sign.start();
    let kind = match sign.as_str() {
        "/" => {
            // Commands work only at the start of a message; "/x/y" is a path, not a command.
            if !before[..start].trim().is_empty() || query.contains(['/', '\\']) {
                return None;
            }
            TriggerKind::Command
        }
        "@" => TriggerKind::Path,
        _ => {
            if query.starts_with(|c: char| c.is_ascii_digit()) {
                return None; // $5, a price
            }
            TriggerKind::Skill
        }
    };
    Some(Trigger { kind, start, query })
}

/// Lower is better: the name exactly, from its start, at a word start, anywhere; then the description.
pub fn match_score(name: &str, description: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(10);
    }
    let n = name.to_lowercase();
    if n == query {
        return Some(0);
    }
    if n.starts_with(query) {
        return Some(1);
    }
    if n.match_indices(query).any(|(i, _)| n[..i].ends_with(['-', '_', ':', '/', '.', ' '])) {
        return Some(2);
    }
    if n.contains(query) {
        return Some(3);
    }
    description.to_lowercase().contains(query).then_some(6)
}

/// Claude Code expands a skill only when /name opens the message, so the first $skill moves to the
/// front (the rest of the message becomes its arguments) and any others become /name.
pub fn with_skills(text: &str, skills: &[Skill]) -> String {
    static SKILL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|\s)\$([\w:-]+)(\s|$)").unwrap());
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
    if !text.contains('$') || skills.is_empty() || text.trim_start().starts_with('/') {
        return text.to_string();
    }
    let names: HashSet<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    let mut first = None;
    let mut body = String::new();
    let mut at = 0;
    // A separator can belong to two matches in a row, so step by hand instead of replace_all.
    while let Some(m) = SKILL.captures_at(text, at) {
        let whole = m.get(0).unwrap();
        let name = &m[2];
        body.push_str(&text[at..whole.start()]);
        body.push_str(&m[1]);
        if !names.contains(name) {
            body.push('$');
            body.push_str(name);
        } else if first.is_none() {
            first = Some(name.to_string());
        } else {
            body.push('/');
            body.push_str(name);
        }
        at = m.get(2).unwrap().end();
    }
    body.push_str(&text[at..]);
    let Some(first) = first else { return text.to_string() };
    format!("/{first} {}", SPACES.replace_all(&body, " ").trim()).trim().to_string()
}

// ---------- the session's model, effort and permission mode ----------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
}

impl Settings {
    /// Session file < what the agent's status line shows now < a model the user just picked.
    pub fn over(&self, base: &Settings) -> Settings {
        Settings {
            model: self.model.clone().or_else(|| base.model.clone()),
            effort: self.effort.clone().or_else(|| base.effort.clone()),
            mode: self.mode.clone().or_else(|| base.mode.clone()),
        }
    }
}

fn claude_defaults(paths: &Paths) -> Settings {
    let s = read_json(&paths.home.join(".claude").join("settings.json")).unwrap_or(Value::Null);
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(String::from);
    let model = text(s.get("model"));
    let per_model = s.get("modelSettings").and_then(Value::as_object);
    // modelSettings is keyed by full model id; match an alias like "opus" to its entry.
    let specific = model.as_deref().and_then(|m| {
        per_model?.iter().find(|(k, _)| k.as_str() == m || k.contains(&format!("-{m}-"))).and_then(|(_, v)| text(v.get("effortLevel")))
    });
    Settings { model, effort: specific.or_else(|| text(s.get("effortLevel"))), mode: text(s.get("permissions").and_then(|p| p.get("defaultMode"))) }
}

fn codex_defaults(paths: &Paths) -> Settings {
    let toml = std::fs::read_to_string(paths.codex_home.join("config.toml")).unwrap_or_default();
    // Only top-level keys: stop at the first [table].
    let top = Regex::new(r"(?m)^\s*\[").unwrap().split(&toml).next().unwrap_or("").to_string();
    let get = |key: &str| Regex::new(&format!(r#"(?m)^\s*{key}\s*=\s*"([^"]*)""#)).ok()?.captures(&top).map(|c| c[1].to_string());
    Settings { model: get("model"), effort: get("model_reasoning_effort"), mode: get("approval_policy") }
}

fn looks_like_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn files_named(dir: &Path, contains: &str, found: &mut Option<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if found.is_some() {
            return;
        }
        let path = e.path();
        if path.is_dir() {
            files_named(&path, contains, found);
        } else if e.file_name().to_string_lossy().contains(contains) && path.extension().is_some_and(|x| x == "jsonl") {
            *found = Some(path);
        }
    }
}

/// A Claude session's file, in the user's folder or a provider's.
pub fn claude_session_file(paths: &Paths, homes: &[ProviderHome], id: &str) -> Option<PathBuf> {
    if !looks_like_id(id) {
        return None;
    }
    let roots = std::iter::once(paths.claude_projects()).chain(homes.iter().filter(|h| h.cli == "claude").map(|h| h.home.join("projects")));
    for root in roots {
        let Ok(dirs) = std::fs::read_dir(root) else { continue };
        for d in dirs.flatten() {
            let f = d.path().join(format!("{id}.jsonl"));
            if f.exists() {
                return Some(f);
            }
        }
    }
    None
}

/// A Codex session's file and the home it's in (the user's or a provider's, then Some(provider)).
pub fn codex_session_file(paths: &Paths, homes: &[ProviderHome], id: &str) -> Option<(PathBuf, PathBuf, bool)> {
    if !looks_like_id(id) {
        return None;
    }
    let roots = std::iter::once((paths.codex_home.clone(), false)).chain(homes.iter().filter(|h| h.cli == "codex").map(|h| (h.home.clone(), true)));
    for (home, provider) in roots {
        let mut found = None;
        files_named(&home.join("sessions"), id, &mut found);
        if let Some(file) = found {
            return Some((file, home, provider));
        }
    }
    None
}

fn read_tail(file: &Path, bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file).ok()?;
    let size = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(size.saturating_sub(bytes))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn last_match(text: &str, pattern: &str) -> Option<String> {
    Regex::new(pattern).ok()?.captures_iter(text).last().map(|c| c[1].to_string())
}

/// What a session's file last saved, over the agent's defaults.
/// A session's transcript, whichever agent wrote it, and that agent.
pub fn session_transcript(paths: &Paths, homes: &[ProviderHome], id: &str) -> Option<(PathBuf, &'static str)> {
    if let Some(file) = claude_session_file(paths, homes, id) {
        return Some((file, "claude"));
    }
    codex_session_file(paths, homes, id).map(|(file, _, _)| (file, "codex"))
}

pub fn session_state(paths: &Paths, homes: &[ProviderHome], id: &str, agent: &str) -> Settings {
    let codex = agent == "codex";
    let mut state = if codex { codex_defaults(paths) } else { claude_defaults(paths) };
    let file = if codex { codex_session_file(paths, homes, id).map(|x| x.0) } else { claude_session_file(paths, homes, id) };
    let Some(tail) = file.and_then(|f| read_tail(&f, 1024 * 1024)) else { return state };
    let patterns = if codex {
        [r#""model":"([^"]+)""#, r#""reasoning_effort":"([^"]+)""#, r#""approval_policy":"([^"]+)""#]
    } else {
        [r#""model":"(claude-[^"]+)""#, r#""effort":"([a-z]+)""#, r#""permissionMode":"([A-Za-z]+)""#]
    };
    state.model = last_match(&tail, patterns[0]).or(state.model);
    state.effort = last_match(&tail, patterns[1]).or(state.effort);
    state.mode = last_match(&tail, patterns[2]).or(state.mode);
    state
}

/// Mode, effort and model off the agent's own status line at the bottom of the screen.
pub fn read_screen(agent: &str, lines: &[String]) -> Settings {
    static CODEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*([\w.-]+)\s+(minimal|low|medium|high|xhigh|max|ultra)\s+·").unwrap());
    static EFFORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(low|medium|high|xhigh|max)\s+·\s+/effort").unwrap());
    let from = lines.len().saturating_sub(8);
    let text = lines[from..].join("\n");
    let mut live = Settings::default();
    if agent == "codex" {
        if let Some(m) = CODEX.captures(&text) {
            live.model = Some(m[1].to_string());
            live.effort = Some(m[2].to_string());
        }
        return live;
    }
    let lower = text.to_lowercase();
    live.mode = if lower.contains("auto mode on") {
        Some("auto")
    } else if lower.contains("accept edits on") {
        Some("acceptEdits")
    } else if lower.contains("plan mode on") {
        Some("plan")
    } else if lower.contains("bypass permissions on") {
        Some("bypassPermissions")
    } else if lower.contains("don't ask on") || lower.contains("dont ask on") {
        Some("dontAsk")
    } else if text.contains("? for shortcuts") {
        Some("manual")
    } else {
        None
    }
    .map(String::from);
    live.effort = EFFORT.captures(&text).map(|m| m[1].to_string());
    live
}

/// What's typed in Claude's prompt box, off the screen. None when no box is showing.
pub fn prompt_text(lines: &[String]) -> Option<String> {
    let from = lines.len().saturating_sub(12);
    lines[from..].iter().rev().find(|l| l.trim_start().starts_with('❯')).map(|l| l.trim_start().trim_start_matches('❯').trim().to_string())
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickerRow {
    pub index: usize,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picker {
    pub heading: String,
    pub rows: Vec<PickerRow>,
    pub highlighted: Option<usize>,
}

/// One of Codex's pickers on screen: "› 3. High (current)    Greater reasoning depth…"
pub fn read_codex_picker(lines: &[String]) -> Option<Picker> {
    static ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(›)?\s*(\d+)\.\s+(.+?)(?:\s{2,}(.*?))?\s*$").unwrap());
    static NOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\((current|default)\)").unwrap());
    const HEADINGS: [&str; 2] = ["Update Model Permissions", "Enable full access"];
    let start = lines.iter().rposition(|l| HEADINGS.iter().any(|h| l.contains(h)))?;
    let mut rows = Vec::new();
    let mut highlighted = None;
    for l in &lines[start + 1..] {
        let Some(m) = ROW.captures(l) else { continue };
        let index: usize = m[2].parse().ok()?;
        rows.push(PickerRow { index, label: NOTE.replace_all(m[3].trim(), "").trim().to_string() });
        if m.get(1).is_some() {
            highlighted = Some(index);
        }
    }
    Some(Picker { heading: lines[start].trim().to_string(), rows, highlighted })
}

// ---------- the models each agent offers ----------

#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeEffort {
    pub id: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeModel {
    pub slug: String,
    pub label: String,
    pub aliases: Vec<String>,
    pub current: bool,
    pub efforts: Vec<ClaudeEffort>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeCatalog {
    pub models: Vec<ClaudeModel>,
    pub default_model: Option<String>,
}

const MODEL_SOURCE: &str = "https://raw.githubusercontent.com/pingdotgg/t3code/main/apps/server/src/provider/model-manifest.json";
// A copy ships with Crew; once a day it's replaced by the one T3 Code publishes (MIT License,
// Copyright (c) 2026 T3 Tools Inc.).
const BUNDLED_MODELS: &str = include_str!("../assets/claude-models.json");

fn valid_list(v: &Value) -> bool {
    v.get("models").and_then(Value::as_array).is_some_and(|m| !m.is_empty()) && v.get("profiles").is_some_and(Value::is_object)
}

fn version_parts(v: &str) -> Vec<u32> {
    v.split('.').map(|x| x.parse().unwrap_or(0)).collect()
}

fn fits(model: &Value, version: Option<&str>) -> bool {
    let Some(version) = version else { return true };
    let c = model.get("adapter").and_then(|a| a.get("claudeCode"));
    let text = |k: &str| c.and_then(|c| c.get(k)).and_then(Value::as_str);
    let at_least = text("minVersion").is_none_or(|m| version_parts(version) >= version_parts(m));
    let below = text("maxVersionExclusive").is_none_or(|m| version_parts(version) < version_parts(m));
    at_least && below
}

/// Current models first (in the list's order), then older ones, each with the effort levels
/// /effort can set. Models the installed Claude Code is too old for are left out.
fn catalog_from(list: &Value, version: Option<&str>) -> ClaudeCatalog {
    const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
    let order: Vec<&str> = list.get("current").and_then(Value::as_array).map_or(Vec::new(), |a| a.iter().filter_map(Value::as_str).collect());
    let rank = |slug: &str| order.iter().position(|s| *s == slug).unwrap_or(99);
    let mut models: Vec<ClaudeModel> = list["models"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|m| m.get("slug").is_some_and(Value::is_string) && m.get("status").and_then(Value::as_str) != Some("hidden") && fits(m, version))
        .map(|m| {
            let slug = m["slug"].as_str().unwrap_or("").to_string();
            let profile = m.get("profile").and_then(Value::as_str).unwrap_or("");
            let efforts = list["profiles"][profile]["capabilities"]["optionDescriptors"]
                .as_array()
                .and_then(|d| d.iter().find(|x| x.get("id").and_then(Value::as_str) == Some("effort")))
                .and_then(|d| d.get("options")?.as_array())
                .map_or(Vec::new(), |o| {
                    o.iter()
                        .filter_map(|x| {
                            let id = x.get("id")?.as_str()?;
                            EFFORTS.contains(&id).then(|| ClaudeEffort { id: id.to_string(), is_default: x.get("isDefault") == Some(&Value::Bool(true)) })
                        })
                        .collect()
                });
            let name = m.get("name").and_then(Value::as_str).unwrap_or(&slug);
            ClaudeModel {
                label: name.strip_prefix("Claude ").unwrap_or(name).to_string(),
                aliases: m.get("aliases").and_then(Value::as_array).map_or(Vec::new(), |a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()),
                current: m.get("status").and_then(Value::as_str) == Some("current"),
                efforts,
                slug,
            }
        })
        .collect();
    models.sort_by_key(|m| (!m.current, rank(&m.slug)));
    let wanted = list.get("defaultModel").and_then(Value::as_str);
    let default_model = models.iter().find(|m| Some(m.slug.as_str()) == wanted).or_else(|| models.iter().find(|m| m.current)).map(|m| m.slug.clone());
    ClaudeCatalog { models, default_model }
}

fn claude_version(config_command: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let command = if config_command.is_empty() { "claude" } else { config_command };
    let launch = crate::agents::shell_command(&[command.to_string(), "--version".to_string()]);
    let mut env = crate::env::clean_env();
    env.extend(launch.env);
    let out = std::process::Command::new(&launch.program).args(&launch.args).env_clear().envs(env).creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).output().ok()?;
    Regex::new(r"\d+\.\d+\.\d+").ok()?.find(&String::from_utf8_lossy(&out.stdout)).map(|m| m.as_str().to_string())
}

/// The list: the day's from T3 Code if it reads right, otherwise the copy Crew ships with.
pub fn claude_catalog(paths: &Paths, config_command: &str, force_refresh: bool) -> ClaudeCatalog {
    let cache = paths.data_dir.join("claude-models.json");
    let fresh = std::fs::metadata(&cache).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age < Duration::from_secs(24 * 3600));
    if force_refresh || !fresh {
        let fetched = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().new_agent().get(MODEL_SOURCE).call().ok().and_then(|mut r| r.body_mut().read_to_string().ok());
        if let Some(m) = fetched.and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            let c = &m["providers"]["claudeAgent"];
            let next = json!({ "source": MODEL_SOURCE, "updatedAt": m["updatedAt"], "current": m["currentModels"]["claudeAgent"], "models": c["models"], "profiles": c["profiles"], "defaultModel": c["defaults"]["chat"] });
            if valid_list(&next) {
                let _ = std::fs::write(&cache, next.to_string());
            }
        }
    }
    let bundled: Value = serde_json::from_str(BUNDLED_MODELS).unwrap_or(Value::Null);
    let cached = read_json(&cache).filter(valid_list);
    let stamp = |v: &Value| v.get("updatedAt").and_then(Value::as_str).unwrap_or("").to_string();
    let list = match cached {
        Some(c) if stamp(&c) >= stamp(&bundled) => c,
        _ => bundled,
    };
    catalog_from(&list, claude_version(config_command).as_deref())
}

impl ClaudeCatalog {
    /// A model as a session or Claude names it ("opus", "claude-opus-5-5", "Opus 5.5").
    /// "Default" is the model Claude Code recommends now.
    pub fn find(&self, name: &str) -> Option<&ClaudeModel> {
        let mut m = name.to_lowercase();
        m = m.trim_end_matches("[1m]").trim_end_matches("(default)").trim().to_string();
        if m.is_empty() {
            return None;
        }
        if m == "default" || m.starts_with("default ") {
            m = self.default_model.clone()?;
        }
        self.models.iter().find(|x| x.slug == m || x.aliases.iter().any(|a| *a == m) || x.label.to_lowercase() == m || format!("claude {}", x.label.to_lowercase()) == m)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodexModel {
    pub value: String,
    pub label: String,
    pub efforts: Vec<String>,
    pub default_effort: String,
}

/// Codex's models and their effort levels, as its app-server lists them.
pub fn codex_models(command: &str) -> Option<Vec<CodexModel>> {
    let command = if command.is_empty() { "codex" } else { command };
    let answer = ask_app_server(command, None, "model/list", json!({}), Duration::from_secs(20))?;
    let list: Vec<CodexModel> = answer
        .get("data")?
        .as_array()?
        .iter()
        .filter(|x| x.get("hidden") != Some(&Value::Bool(true)))
        .filter_map(|x| {
            let text = |k: &str| x.get(k).and_then(Value::as_str);
            let id = text("id");
            Some(CodexModel {
                value: text("model").or(id)?.to_string(),
                label: text("displayName").or(id)?.to_string(),
                efforts: x.get("supportedReasoningEfforts").and_then(Value::as_array).map_or(Vec::new(), |a| a.iter().filter_map(|e| e.get("reasoningEffort")?.as_str().map(String::from)).collect()),
                default_effort: text("defaultReasoningEffort").unwrap_or("").to_string(),
            })
        })
        .collect();
    (!list.is_empty()).then_some(list)
}

// ---------- sessions running outside this window ----------

/// Sessions with a live agent on this machine, whoever started it (the other Crew, a terminal), and
/// whether each is working now. Claude Code keeps a status file per process; Codex has none, so it
/// counts when `codex resume <id>` is running or its file was written in the last seconds.
pub fn live_sessions(paths: &Paths, homes: &[ProviderHome], codex_files: &[(String, PathBuf)]) -> std::collections::HashMap<String, bool> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    static RESUME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bresume\s+([0-9a-fA-F-]{36})\b").unwrap());
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always));
    let mut live = std::collections::HashMap::new();

    let claude_homes = std::iter::once(paths.home.join(".claude")).chain(homes.iter().filter(|h| h.cli == "claude").map(|h| h.home.clone()));
    for home in claude_homes {
        let Ok(entries) = std::fs::read_dir(home.join("sessions")) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(v) = read_json(&path) else { continue };
            let (Some(pid), Some(id)) = (v.get("pid").and_then(Value::as_u64), v.get("sessionId").and_then(Value::as_str)) else { continue };
            // A file can outlive its process.
            if system.process(sysinfo::Pid::from_u32(pid as u32)).is_some() {
                live.insert(id.to_string(), v.get("status").and_then(Value::as_str) == Some("busy"));
            }
        }
    }
    for process in system.processes().values() {
        if !process.name().to_string_lossy().to_lowercase().contains("codex") {
            continue;
        }
        let line = process.cmd().iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ");
        if let Some(m) = RESUME.captures(&line) {
            live.entry(m[1].to_lowercase()).or_insert(false);
        }
    }
    for (id, file) in codex_files {
        let written = std::fs::metadata(file).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
        if written.is_some_and(|age| age < Duration::from_secs(12)) {
            live.insert(id.clone(), true);
        }
    }
    live
}

// ---------- rename and delete ----------

/// Claude: the same record Claude Code's /rename writes. Codex: the same entry Codex appends to its
/// name index. Either way the agent's own resume picker shows the new name too.
pub fn rename_session(paths: &Paths, homes: &[ProviderHome], id: &str, title: &str, agent: &str) -> bool {
    let title = title.trim();
    if title.is_empty() || !looks_like_id(id) {
        return false;
    }
    let (file, line) = if agent == "codex" {
        let home = codex_session_file(paths, homes, id).map_or_else(|| paths.codex_home.clone(), |x| x.1);
        (home.join("session_index.jsonl"), json!({ "id": id, "thread_name": title, "updated_at": chrono::Utc::now().to_rfc3339() }))
    } else {
        let Some(f) = claude_session_file(paths, homes, id) else { return false };
        (f, json!({ "type": "custom-title", "customTitle": title, "sessionId": id }))
    };
    std::fs::OpenOptions::new().create(true).append(true).open(file).and_then(|mut f| writeln!(f, "{line}")).is_ok()
}

/// A Claude session's file goes to the Recycle Bin. Codex deletes its own, for good.
pub fn delete_session(paths: &Paths, homes: &[ProviderHome], id: &str, agent: &str, codex_command: &str, was_running: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if !looks_like_id(id) {
        return Err("Not a session ID.".to_string());
    }
    if agent != "codex" {
        let file = claude_session_file(paths, homes, id).ok_or("The session's file wasn't found.")?;
        trash::delete(&file).map_err(|e| e.to_string())?;
        let sidecar = file.with_extension(""); // subagent / tool-result folder for the session
        if sidecar.exists() {
            let _ = trash::delete(&sidecar);
        }
        return Ok(());
    }
    // A provider's session is in the provider's own Codex home.
    let root = codex_session_file(paths, homes, id);
    let command = if codex_command.is_empty() { "codex" } else { codex_command };
    let launch = crate::agents::shell_command(&[command.to_string(), "delete".to_string(), "--force".to_string(), id.to_string()]);
    let mut env = crate::env::clean_env();
    env.extend(launch.env);
    if let Some((_, home, true)) = &root {
        env.insert("CODEX_HOME".to_string(), home.to_string_lossy().to_string());
    }
    // A Codex that was just stopped can hold the session for a moment while it exits.
    let mut output = String::new();
    for attempt in 0..4 {
        if was_running || attempt > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let out = std::process::Command::new(&launch.program).args(&launch.args).env_clear().envs(env.clone()).creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(());
        }
        output = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
    let tail: String = output.trim().chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect();
    Err(format!("Codex could not delete the session. {tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trig(text: &str) -> Option<(TriggerKind, usize, String)> {
        trigger_at(text, text.len()).map(|t| (t.kind, t.start, t.query))
    }

    #[test]
    fn finds_what_is_being_typed() {
        assert_eq!(trig("/mo"), Some((TriggerKind::Command, 0, "mo".into())));
        assert_eq!(trig("hello /mo"), None); // only at the start of a message
        assert_eq!(trig("/a/b"), None); // a path
        assert_eq!(trig("see @src/ma"), Some((TriggerKind::Path, 4, "src/ma".into())));
        assert_eq!(trig("a@b"), None); // an email address
        assert_eq!(trig("use $sk"), Some((TriggerKind::Skill, 4, "sk".into())));
        assert_eq!(trig("costs $5"), None);
        assert_eq!(trig("用 ￥sk"), Some((TriggerKind::Skill, 4, "sk".into())));
        assert_eq!(trig("done "), None);
    }

    #[test]
    fn ranks_matches() {
        assert_eq!(match_score("model", "", "model"), Some(0));
        assert_eq!(match_score("model", "", "mo"), Some(1));
        assert_eq!(match_score("security-review", "", "review"), Some(2));
        assert_eq!(match_score("preview", "", "view"), Some(3));
        assert_eq!(match_score("x", "the view", "view"), Some(6));
        assert_eq!(match_score("x", "", "view"), None);
    }

    #[test]
    fn moves_skills_to_the_front() {
        let skills = vec![Skill { name: "ego".into(), description: String::new(), scope: String::new() }, Skill { name: "two".into(), description: String::new(), scope: String::new() }];
        assert_eq!(with_skills("open it with $ego please", &skills), "/ego open it with please");
        assert_eq!(with_skills("$ego $two now", &skills), "/ego /two now");
        assert_eq!(with_skills("costs $5 and $other", &skills), "costs $5 and $other");
        assert_eq!(with_skills("/model $ego", &skills), "/model $ego");
    }

    #[test]
    fn reads_front_matter() {
        let fm = front_matter("---\nname: my-skill\ndescription: |\n  two\n  lines\nuser-invocable: \"false\"\n---\nbody");
        assert_eq!(fm["name"], "my-skill");
        assert_eq!(fm["description"], "two lines");
        assert_eq!(fm["user-invocable"], "false");
    }

    #[test]
    fn reads_status_lines() {
        let lines: Vec<String> = ["x", "  ⏵⏵ accept edits on (shift+tab to cycle)", "  high · /effort"].iter().map(|s| s.to_string()).collect();
        let s = read_screen("claude", &lines);
        assert_eq!(s.mode.as_deref(), Some("acceptEdits"));
        assert_eq!(s.effort.as_deref(), Some("high"));
        let lines = vec!["  gpt-6-luna high · ~\\code".to_string()];
        let s = read_screen("codex", &lines);
        assert_eq!((s.model.as_deref(), s.effort.as_deref()), (Some("gpt-6-luna"), Some("high")));
        assert_eq!(prompt_text(&["a".to_string(), "❯ typed".to_string()]).as_deref(), Some("typed"));
    }

    #[test]
    fn reads_codex_pickers() {
        let lines: Vec<String> = ["old 1. no", "  Update Model Permissions", "› 1. Read Only (current)   Reads", "  2. Full Access   Uses"].iter().map(|s| s.to_string()).collect();
        let p = read_codex_picker(&lines).unwrap();
        assert_eq!(p.highlighted, Some(1));
        assert_eq!(p.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["Read Only", "Full Access"]);
    }

    #[test]
    fn the_bundled_model_list_reads() {
        let list: Value = serde_json::from_str(BUNDLED_MODELS).unwrap();
        let c = catalog_from(&list, None);
        assert!(!c.models.is_empty());
        assert!(c.models[0].current);
        assert!(c.find("default").is_some());
    }

    #[test]
    fn renames_a_claude_session() {
        let root = std::env::temp_dir().join(format!("crew-rename-{}", uuid::Uuid::new_v4()));
        let id = uuid::Uuid::new_v4().to_string();
        let dir = root.join(".claude").join("projects").join("p");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.jsonl")), "").unwrap();
        let paths = Paths { home: root.clone(), data_dir: root.clone(), codex_home: root.join(".codex") };
        assert!(rename_session(&paths, &[], &id, " New name ", "claude"));
        let text = std::fs::read_to_string(dir.join(format!("{id}.jsonl"))).unwrap();
        assert!(text.contains("\"customTitle\":\"New name\""));
        let _ = std::fs::remove_dir_all(&root);
    }
}
