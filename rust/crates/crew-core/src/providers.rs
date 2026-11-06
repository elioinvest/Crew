//! Custom model providers: each runs as a Claude Code or Codex shell in its own home folder, kept by
//! Crew, where the CLI keeps the provider's settings, sessions and history apart from the user's own.
//! providers.json holds them (the key sealed), provider-sessions.json what each session runs on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::config::Paths;
use crate::secrets;
use crate::sessions::ProviderHome;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
}

/// What the app needs of a provider; the key stays in the file until a session starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub models: Vec<ProviderModel>,
    pub default_model: String,
    pub image: Option<String>,
    pub emoji: Option<String>,
    sealed_key: Option<String>,
}

impl Provider {
    /// The CLI that speaks its protocol.
    pub fn cli(&self) -> &'static str {
        if self.protocol == "anthropic" { "claude" } else { "codex" }
    }

    pub fn has_key(&self) -> bool {
        self.sealed_key.is_some()
    }

    fn model_or_default<'a>(&'a self, model: Option<&'a str>) -> &'a str {
        model.filter(|m| !m.is_empty()).unwrap_or(if self.default_model.is_empty() { self.models.first().map_or("", |m| m.id.as_str()) } else { &self.default_model })
    }
}

fn parse(p: &Value) -> Option<Provider> {
    let id = p.get("id")?.as_str()?.to_string();
    let icon = |key: &str| p.get("icon").and_then(|i| i.get(key)).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let text = |key: &str| p.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let models = p
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let mid = m.get("id")?.as_str()?.to_string();
            Some(ProviderModel { name: m.get("name").and_then(Value::as_str).unwrap_or(&mid).to_string(), id: mid })
        })
        .collect();
    Some(Provider {
        name: p.get("name").and_then(Value::as_str).unwrap_or(&id).to_string(),
        protocol: text("protocol"),
        base_url: text("baseUrl"),
        models,
        default_model: text("defaultModel"),
        image: icon("image"),
        emoji: icon("emoji"),
        sealed_key: p.get("key").and_then(Value::as_str).map(str::to_string),
        id,
    })
}

pub fn list(paths: &Paths) -> Vec<Provider> {
    let Ok(text) = std::fs::read_to_string(paths.data_dir.join("providers.json")) else { return Vec::new() };
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    list.iter().filter_map(parse).collect()
}

pub fn homes(paths: &Paths) -> Vec<ProviderHome> {
    list(paths)
        .into_iter()
        .filter(|p| matches!(p.protocol.as_str(), "anthropic" | "responses"))
        .map(|p| ProviderHome { cli: p.cli().to_string(), home: paths.provider_homes().join(&p.id), id: p.id })
        .collect()
}

// ---------- which provider each session runs on ----------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Binding {
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

fn read_bindings(paths: &Paths) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(paths.data_dir.join("provider-sessions.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn binding(paths: &Paths, session: &str) -> Option<Binding> {
    let map = read_bindings(paths);
    let v = map.get(session)?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let b = match v.as_str() {
        Some(provider) => Binding { provider: provider.to_string(), ..Binding::default() },
        None => Binding { provider: field("provider")?, model: field("model"), effort: field("effort") },
    };
    list(paths).iter().any(|p| p.id == b.provider).then_some(b)
}

pub fn bind(paths: &Paths, session: &str, b: &Binding) {
    let mut map = read_bindings(paths);
    map.insert(session.to_string(), json!({"provider": b.provider, "model": b.model, "effort": b.effort}));
    let _ = std::fs::write(paths.data_dir.join("provider-sessions.json"), serde_json::to_string_pretty(&map).unwrap_or_default());
}

/// A new Codex session picks its own id; what it ran on moves to that id when it's known.
pub fn rebind(paths: &Paths, from: &str, to: &str) {
    let mut map = read_bindings(paths);
    if from != to {
        if let Some(v) = map.remove(from) {
            map.insert(to.to_string(), v);
            let _ = std::fs::write(paths.data_dir.join("provider-sessions.json"), serde_json::to_string_pretty(&map).unwrap_or_default());
        }
    }
}

// ---------- each provider's own environment ----------

fn trim_url(u: &str) -> &str {
    u.trim().trim_end_matches('/')
}

fn anthropic_base(u: &str) -> &str {
    let t = trim_url(u);
    t.strip_suffix("/v1").unwrap_or(t)
}

fn toml(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Crew's own tools follow the agent into the provider's home, linked so they stay the user's installed ones.
fn link_skills(home: &Path, cli_home: &Path) {
    for name in ["crew-ssh", "ego-browser"] {
        let src = cli_home.join("skills").join(name);
        let dst = home.join("skills").join(name);
        if !src.exists() || dst.exists() {
            continue;
        }
        let _ = std::fs::create_dir_all(home.join("skills"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("cmd")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&dst)
                .arg(&src)
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

/// The [windows] table of the user's own Codex config.toml (the sandbox setting), or nothing.
fn windows_table(user_codex: &Path) -> Option<String> {
    static TABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\[windows\][^\n]*\n((?s:.*?))(?:^\s*\[|\z)").unwrap());
    let text = std::fs::read_to_string(user_codex.join("config.toml")).ok()?;
    let m = TABLE.captures(&text)?;
    Some(format!("[windows]\n{}\n", m[1].trim()))
}

/// What Codex itself wrote in the home's config.toml (folders it trusted, its own settings), so a
/// restart doesn't ask again. Crew rewrites only what it manages. Returns (top-level keys, tables).
fn codex_own_settings(config_file: &Path) -> Option<(String, String)> {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\[\[?\s*([^\]]+?)\s*\]\]?\s*(#.*)?$").unwrap());
    static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*([A-Za-z0-9_-]+)\s*=").unwrap());
    let text = std::fs::read_to_string(config_file).ok()?;
    let mut top = Vec::new();
    let mut tables = Vec::new();
    let mut table: Option<String> = None;
    let mut keep = true;
    for line in text.lines() {
        if let Some(h) = HEADER.captures(line) {
            let name = h[1].to_string();
            keep = !["model_providers.crew", "windows"].contains(&name.as_str());
            table = Some(name);
        } else if table.is_none() {
            let key = KEY.captures(line).map(|k| k[1].to_string());
            let crew = key.as_deref().is_none_or(|k| ["model_provider", "model", "check_for_update_on_startup", "model_catalog_json"].contains(&k));
            if crew || line.trim_start().starts_with("# Written by Crew") {
                continue;
            }
        }
        if keep && (table.is_some() || !line.trim().is_empty()) {
            if table.is_some() { tables.push(line) } else { top.push(line) }
        }
    }
    Some((top.join("\n").trim().to_string(), tables.join("\n").trim().to_string()))
}

/// Codex's /model only accepts models in its catalog, so the provider's models go into one. Its
/// entries carry Codex's base instructions (codex-prompt.md, from openai/codex, Apache-2.0).
fn write_catalog(p: &Provider, home: &Path) -> Option<PathBuf> {
    if p.models.is_empty() {
        return None;
    }
    let efforts: Vec<Value> = [("low", "Fast responses with lighter reasoning"), ("medium", "Balances speed and reasoning depth"), ("high", "Greater reasoning depth for complex problems"), ("xhigh", "Extra high reasoning depth")]
        .iter()
        .map(|(effort, description)| json!({"effort": effort, "description": description}))
        .collect();
    let models: Vec<Value> = p
        .models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            json!({
                "slug": m.id, "display_name": m.name, "description": p.name, "default_reasoning_level": "medium",
                "supported_reasoning_levels": efforts, "shell_type": "unified_exec", "visibility": "list", "supported_in_api": true,
                "priority": i, "availability_nux": null, "upgrade": null, "support_verbosity": false, "default_verbosity": null,
                "apply_patch_tool_type": null, "truncation_policy": {"mode": "bytes", "limit": 10000}, "experimental_supported_tools": [],
                "base_instructions": include_str!("../assets/codex-prompt.md"),
            })
        })
        .collect();
    let file = home.join("models.json");
    std::fs::write(&file, json!({"models": models}).to_string()).ok()?;
    Some(file)
}

/// Rewritten whenever a session starts; the sessions and history the CLI keeps there are left alone.
fn write_home(paths: &Paths, p: &Provider) -> std::io::Result<PathBuf> {
    let home = paths.provider_homes().join(&p.id);
    std::fs::create_dir_all(&home)?;
    let m = p.model_or_default(None);
    if p.protocol == "responses" {
        let config_file = home.join("config.toml");
        let own = codex_own_settings(&config_file);
        let catalog = write_catalog(p, &home);
        let (own_top, own_tables) = own.unwrap_or_default();
        let mut lines = vec![
            format!("# Written by Crew for the custom provider {}. Crew rewrites its own settings when the provider changes and keeps the rest.", toml(&p.name)),
            "model_provider = \"crew\"".to_string(),
        ];
        if !m.is_empty() {
            lines.push(format!("model = {}", toml(m)));
        }
        lines.push("check_for_update_on_startup = false".into());
        if let Some(c) = catalog {
            lines.push(format!("model_catalog_json = {}", toml(&c.to_string_lossy())));
        }
        if !own_top.is_empty() {
            lines.push(own_top);
        }
        lines.extend([
            String::new(),
            "[model_providers.crew]".into(),
            format!("name = {}", toml(&p.name)),
            format!("base_url = {}", toml(trim_url(&p.base_url))),
            "env_key = \"CREW_PROVIDER_API_KEY\"".into(),
            "wire_api = \"responses\"".into(),
            String::new(),
        ]);
        if let Some(w) = windows_table(&paths.codex_home) {
            lines.push(w);
        }
        if !own_tables.is_empty() {
            lines.push(own_tables);
        }
        lines.push(String::new());
        std::fs::write(&config_file, lines.join("\n"))?;
        link_skills(&home, &paths.codex_home);
        let rules = paths.codex_home.join("rules").join("crew.rules");
        if rules.exists() {
            let _ = std::fs::create_dir_all(home.join("rules"));
            let _ = std::fs::copy(rules, home.join("rules").join("crew.rules"));
        }
    } else {
        // Claude Code keeps its global state here: skip the first-run setup, with the user's theme.
        let state = home.join(".claude.json");
        if !state.exists() {
            let theme = std::fs::read_to_string(paths.home.join(".claude.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.get("theme").cloned());
            let mut v = json!({"hasCompletedOnboarding": true});
            if let Some(theme) = theme {
                v["theme"] = theme;
            }
            std::fs::write(&state, serde_json::to_string_pretty(&v).unwrap_or_default())?;
        }
        link_skills(&home, &paths.home.join(".claude"));
    }
    Ok(home)
}

/// How to start a session on a provider. An environment change of None removes the variable.
#[derive(Clone, Debug, Default)]
pub struct ProviderLaunch {
    pub cli: String,
    pub args: Vec<String>,
    pub env: Vec<(String, Option<String>)>,
}

const EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

pub fn launch(paths: &Paths, id: &str, model: Option<&str>, effort: Option<&str>) -> Option<ProviderLaunch> {
    let p = list(paths).into_iter().find(|p| p.id == id)?;
    let home = write_home(paths, &p).ok()?.to_string_lossy().to_string();
    let key = p.sealed_key.as_deref().and_then(|k| secrets::unseal(paths, k)).unwrap_or_default();
    let m = p.model_or_default(model).to_string();
    let effort = effort.filter(|e| EFFORTS.contains(e));
    if p.protocol == "anthropic" {
        let mut env = vec![
            ("CLAUDE_CONFIG_DIR".to_string(), Some(home)),
            ("ANTHROPIC_BASE_URL".to_string(), Some(anthropic_base(&p.base_url).to_string())),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), Some(key)),
            ("ANTHROPIC_API_KEY".to_string(), None),
        ];
        // Claude Code also uses its Haiku/Sonnet/Opus aliases for background work and /model; point
        // them all at the provider's model, likely the only one it serves under that name.
        if !m.is_empty() {
            for name in ["ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL"] {
                env.push((name.to_string(), Some(m.clone())));
            }
        }
        let args = effort.map(|e| vec!["--effort".to_string(), e.to_string()]).unwrap_or_default();
        return Some(ProviderLaunch { cli: "claude".into(), args, env });
    }
    let mut args = Vec::new();
    if !m.is_empty() {
        args.extend(["-c".to_string(), format!("model={m}")]);
    }
    if let Some(e) = effort {
        args.extend(["-c".to_string(), format!("model_reasoning_effort={e}")]);
    }
    Some(ProviderLaunch { cli: "codex".into(), args, env: vec![("CODEX_HOME".into(), Some(home)), ("CREW_PROVIDER_API_KEY".into(), Some(key))] })
}

/// Applies a launch's changes, ignoring case as Windows does.
pub fn apply_env(env: &mut HashMap<String, String>, changes: &[(String, Option<String>)]) {
    for (k, v) in changes {
        env.retain(|existing, _| !existing.eq_ignore_ascii_case(k));
        if let Some(v) = v {
            env.insert(k.clone(), v.clone());
        }
    }
}

// ---------- editing the list ----------

/// What the provider editor saves. An empty key leaves the saved one as it is.
#[derive(Clone, Debug, Default)]
pub struct ProviderInput {
    /// Empty for a new provider.
    pub original_id: String,
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub protocol: String,
    pub key: String,
    pub models: Vec<ProviderModel>,
    pub default_model: String,
    pub emoji: Option<String>,
    pub image: Option<String>,
}

/// The ID made valid as it's typed: "DeepSeek Relay" -> "deepseek-relay".
pub fn to_id(text: &str) -> String {
    static NOT_ALLOWED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9-]+").unwrap());
    static DASHES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-{2,}").unwrap());
    let lower = text.to_lowercase();
    let cleaned = NOT_ALLOWED.replace_all(&lower, "-");
    let from_letter = cleaned.trim_start_matches(|c: char| !c.is_ascii_lowercase());
    DASHES.replace_all(from_letter, "-").chars().take(40).collect()
}

/// A readable name from a model ID: "workbuddy-ai/deepseek-v4.1-flash" -> "Deepseek-V4.1-Flash".
pub fn pretty_name(model_id: &str) -> String {
    let last = model_id.rsplit('/').next().unwrap_or(model_id);
    let mut out = String::new();
    let mut at_start = true;
    for c in last.chars() {
        out.extend(if at_start && c.is_ascii_lowercase() { c.to_uppercase().collect::<Vec<_>>() } else { vec![c] });
        at_start = matches!(c, '-' | '_' | ' ' | '.');
    }
    out
}

fn read_raw(paths: &Paths) -> Vec<Value> {
    match std::fs::read_to_string(paths.data_dir.join("providers.json")).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(Value::Array(list)) => list,
        _ => Vec::new(),
    }
}

fn write_raw(paths: &Paths, list: &[Value]) -> Result<(), String> {
    std::fs::create_dir_all(&paths.data_dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(paths.data_dir.join("providers.json"), text).map_err(|e| e.to_string())
}

fn write_bindings(paths: &Paths, map: &serde_json::Map<String, Value>) {
    let _ = std::fs::write(paths.data_dir.join("provider-sessions.json"), serde_json::to_string_pretty(map).unwrap_or_default());
}

pub fn save(paths: &Paths, input: &ProviderInput) -> Result<Provider, String> {
    static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9-]{0,39}$").unwrap());
    static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^https?://\S+$").unwrap());
    let id = input.id.trim();
    if !ID_RE.is_match(id) {
        return Err("The provider ID starts with a lowercase letter and uses only a-z, 0-9 and -.".into());
    }
    if !matches!(input.protocol.as_str(), "anthropic" | "responses") {
        return Err("Choose an API protocol.".into());
    }
    let base_url = input.base_url.trim();
    if !URL_RE.is_match(base_url) {
        return Err("Enter the API URL, starting with https://.".into());
    }
    let mut list = read_raw(paths);
    let id_of = |v: &Value| v.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    if input.original_id != id && list.iter().any(|p| id_of(p) == id) {
        return Err(format!("A provider with the ID \"{id}\" already exists."));
    }
    let lookup = if input.original_id.is_empty() { id } else { input.original_id.as_str() };
    let at = match list.iter().position(|p| id_of(p) == lookup) {
        Some(i) => i,
        None => {
            list.push(json!({}));
            list.len() - 1
        }
    };
    let old_id = id_of(&list[at]);
    let old_protocol = list[at].get("protocol").and_then(Value::as_str).unwrap_or("").to_string();
    let home_of = |pid: &str| paths.provider_homes().join(pid);
    let mut bindings = read_bindings(paths);
    if !old_id.is_empty() && old_id != id {
        for b in bindings.values_mut() {
            if b.as_str() == Some(old_id.as_str()) {
                *b = json!(id);
            } else if b.get("provider").and_then(Value::as_str) == Some(old_id.as_str()) {
                b["provider"] = json!(id);
            }
        }
        let _ = std::fs::rename(home_of(&old_id), home_of(id)); // its sessions move with it
    }
    if !old_protocol.is_empty() && old_protocol != input.protocol {
        // The other CLI keeps its files differently: start the home over, keeping the old one aside.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let _ = std::fs::rename(home_of(id), home_of(&format!("{id}-{old_protocol}-{stamp}")));
    }
    let mut models: Vec<Value> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for m in &input.models {
        let mid = m.id.trim();
        if !mid.is_empty() && !ids.contains(&mid) {
            ids.push(mid);
            let name = m.name.trim();
            models.push(json!({"id": mid, "name": if name.is_empty() { mid } else { name }}));
        }
    }
    let default_model = if ids.contains(&input.default_model.as_str()) { input.default_model.as_str() } else { ids.first().copied().unwrap_or("") };
    let icon = match (input.emoji.as_deref().filter(|e| !e.is_empty()), input.image.as_deref()) {
        (Some(e), _) => json!({"emoji": e.chars().take(8).collect::<String>()}),
        (None, Some(i)) if i.starts_with("data:image/") && i.len() < 717 * 1024 => json!({"image": i}),
        _ => Value::Null,
    };
    let name = input.name.trim();
    let p = &mut list[at];
    p["id"] = json!(id);
    p["name"] = json!(if name.is_empty() { id } else { name });
    p["baseUrl"] = json!(trim_url(base_url));
    p["protocol"] = json!(input.protocol);
    p["models"] = json!(models);
    p["defaultModel"] = json!(default_model);
    p["icon"] = icon;
    let key = input.key.trim();
    if !key.is_empty() {
        let sealed = secrets::seal(paths, key).ok_or("This system can't encrypt the API key, so it can't be saved.")?;
        p["key"] = json!(sealed);
    }
    let saved = parse(p).ok_or("Couldn't read the provider back.")?;
    write_raw(paths, &list)?;
    write_bindings(paths, &bindings);
    let _ = write_home(paths, &saved); // written again at the next session otherwise
    Ok(saved)
}

/// Its sessions are kept, but resume on the agent's own account.
pub fn delete(paths: &Paths, id: &str) -> Result<(), String> {
    let mut list = read_raw(paths);
    list.retain(|p| p.get("id").and_then(Value::as_str) != Some(id));
    write_raw(paths, &list)
}

/// The models a provider lists: GET <root>/models, as OpenAI and Anthropic both serve it. Blocks.
pub fn fetch_models(paths: &Paths, saved_id: &str, base_url: &str, protocol: &str, key: &str) -> Result<Vec<String>, String> {
    static VERSIONED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/v\d+$").unwrap());
    let saved = list(paths).into_iter().find(|p| p.id == saved_id);
    let url = if base_url.trim().is_empty() { saved.as_ref().map_or("", |p| p.base_url.as_str()) } else { base_url };
    if url.trim().is_empty() {
        return Err("Enter the API URL first.".into());
    }
    let protocol = if protocol.is_empty() { saved.as_ref().map_or("", |p| p.protocol.as_str()) } else { protocol };
    let token = if key.trim().is_empty() {
        saved.as_ref().and_then(|p| p.sealed_key.as_deref()).and_then(|k| secrets::unseal(paths, k)).unwrap_or_default()
    } else {
        key.trim().to_string()
    };
    let root = trim_url(url);
    let target = if VERSIONED.is_match(root) { format!("{root}/models") } else { format!("{root}/v1/models") };
    let agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(15))).build().new_agent();
    let mut request = agent.get(&target).header("accept", "application/json");
    if !token.is_empty() {
        request = request.header("authorization", &format!("Bearer {token}")).header("x-api-key", &token);
    }
    if protocol == "anthropic" {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let mut response = request.call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => format!("The provider answered {code}."),
        ureq::Error::Timeout(_) => "The provider didn't answer within 15 seconds.".to_string(),
        other => other.to_string(),
    })?;
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let body: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let items = body.as_array().or_else(|| body.get("data").and_then(Value::as_array)).or_else(|| body.get("models").and_then(Value::as_array));
    let mut names: Vec<String> = items
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().or_else(|| m.get("id").and_then(Value::as_str)).or_else(|| m.get("name").and_then(Value::as_str)))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(providers: Value) -> Paths {
        let d = std::env::temp_dir().join(format!("crew-providers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("providers.json"), providers.to_string()).unwrap();
        Paths { home: d.clone(), data_dir: d.clone(), codex_home: d.join(".codex") }
    }

    #[test]
    fn a_responses_provider_gets_a_codex_home_and_keeps_what_codex_wrote() {
        let paths = scratch(json!([{"id":"gw","name":"Gateway","protocol":"responses","baseUrl":"https://x/v1/","models":[{"id":"m1","name":"M1"}],"defaultModel":"m1"}]));
        let home = paths.provider_homes().join("gw");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.toml"), "model = \"old\"\nmine = 1\n\n[projects.'C:\\x']\ntrust_level = \"trusted\"\n\n[model_providers.crew]\nname = \"old\"\n").unwrap();
        let l = launch(&paths, "gw", None, Some("high")).unwrap();
        assert_eq!(l.cli, "codex");
        assert_eq!(l.args, ["-c", "model=m1", "-c", "model_reasoning_effort=high"]);
        let text = std::fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(text.contains("mine = 1") && text.contains("trust_level") && text.contains("base_url = \"https://x/v1\""));
        assert!(!text.contains("\"old\""), "Crew's own settings are rewritten");
        assert_eq!(text.matches("[model_providers.crew]").count(), 1);
        assert!(home.join("models.json").exists());
    }

    #[test]
    fn an_anthropic_provider_points_claude_at_it() {
        let paths = scratch(json!([{"id":"a","protocol":"anthropic","baseUrl":"https://x/v1","models":[{"id":"m"}]}]));
        let l = launch(&paths, "a", Some("other"), None).unwrap();
        assert_eq!(l.cli, "claude");
        let mut env: HashMap<String, String> = [("anthropic_api_key".to_string(), "mine".to_string())].into();
        apply_env(&mut env, &l.env);
        assert!(!env.contains_key("anthropic_api_key"), "the user's own key is removed");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "https://x");
        assert_eq!(env["ANTHROPIC_MODEL"], "other");
        assert!(paths.provider_homes().join("a").join(".claude.json").exists());
    }

    #[test]
    fn ids_and_names_are_made_readable() {
        assert_eq!(to_id("DeepSeek Relay"), "deepseek-relay");
        assert_eq!(to_id("9 Go!!"), "go-");
        assert_eq!(pretty_name("workbuddy-ai/deepseek-v4.1-flash"), "Deepseek-V4.1-Flash");
    }

    #[test]
    fn saving_keeps_unknown_keys_and_moves_a_renamed_providers_sessions() {
        let paths = scratch(json!([{"id":"old","name":"Old","protocol":"responses","baseUrl":"https://x/v1","key":"sealed","mine":1}]));
        std::fs::write(paths.data_dir.join("provider-sessions.json"), r#"{"s1":{"provider":"old"}}"#).unwrap();
        std::fs::create_dir_all(paths.provider_homes().join("old").join("sessions")).unwrap();
        let input = ProviderInput {
            original_id: "old".into(),
            id: "new".into(),
            name: "New".into(),
            base_url: "https://y/v1/".into(),
            protocol: "responses".into(),
            models: vec![ProviderModel { id: "m".into(), name: String::new() }, ProviderModel { id: "m".into(), name: "dup".into() }],
            ..ProviderInput::default()
        };
        let saved = save(&paths, &input).unwrap();
        assert_eq!((saved.id.as_str(), saved.base_url.as_str(), saved.default_model.as_str(), saved.models.len()), ("new", "https://y/v1", "m", 1));
        assert!(saved.has_key(), "an empty key leaves the saved one");
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(paths.data_dir.join("providers.json")).unwrap()).unwrap();
        assert_eq!(raw[0]["mine"], 1);
        assert_eq!(binding(&paths, "s1").unwrap().provider, "new");
        assert!(paths.provider_homes().join("new").join("sessions").exists());
        delete(&paths, "new").unwrap();
        assert!(list(&paths).is_empty());
    }

    #[test]
    fn saving_refuses_what_wouldnt_work() {
        let paths = scratch(json!([{"id":"taken","protocol":"responses","baseUrl":"https://x"}]));
        let good = ProviderInput { id: "a".into(), base_url: "https://x".into(), protocol: "responses".into(), ..ProviderInput::default() };
        assert!(save(&paths, &ProviderInput { id: "9a".into(), ..good.clone() }).is_err());
        assert!(save(&paths, &ProviderInput { protocol: "x".into(), ..good.clone() }).is_err());
        assert!(save(&paths, &ProviderInput { base_url: "ftp://x".into(), ..good.clone() }).is_err());
        assert!(save(&paths, &ProviderInput { id: "taken".into(), ..good.clone() }).is_err());
        assert!(save(&paths, &good).is_ok());
    }

    #[test]
    fn bindings_follow_a_session_and_read_the_old_form() {
        let paths = scratch(json!([{"id":"a","protocol":"anthropic","baseUrl":"https://x"}]));
        std::fs::write(paths.data_dir.join("provider-sessions.json"), r#"{"s1":"a","s2":"gone"}"#).unwrap();
        assert_eq!(binding(&paths, "s1").unwrap().provider, "a");
        assert_eq!(binding(&paths, "s2"), None);
        bind(&paths, "new", &Binding { provider: "a".into(), model: Some("m".into()), effort: None });
        rebind(&paths, "new", "real");
        assert_eq!(binding(&paths, "real").unwrap().model.as_deref(), Some("m"));
        assert_eq!(binding(&paths, "new"), None);
    }
}
