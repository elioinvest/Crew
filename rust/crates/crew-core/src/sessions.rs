//! Sessions on disk: Claude Code's project folders and Codex's rollout files, grouped into projects.
//! Files can be tens of MB, so only the head (cwd, first prompt) and tail (latest title) are read,
//! and results are kept by modified time and size.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde_json::Value;

use crate::config::{Config, Paths};
use crate::worktrees::{Registry, SessionWorktree};

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub file: PathBuf,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub title: Option<String>,
    pub mtime: f64,
    pub size: u64,
    pub provider: Option<String>,
    pub worktree: Option<SessionWorktree>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub path: PathBuf,
    pub name: String,
    pub sessions: Vec<Session>,
    pub mtime: f64,
    pub exists: bool,
}

const HEAD: u64 = 128 * 1024;
const TAIL: u64 = 512 * 1024;
const CODEX_HEAD: u64 = 256 * 1024;

fn read_slice(f: &mut File, start: u64, len: u64) -> String {
    let mut buf = vec![0u8; len as usize];
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    buf.truncate(got);
    String::from_utf8_lossy(&buf).into_owned()
}

fn parse_lines(text: &str, drop_first: bool, drop_last: bool) -> Vec<Value> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if drop_first && !lines.is_empty() {
        lines.remove(0);
    }
    if drop_last {
        lines.pop();
    }
    lines.iter().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn text_of_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .find(|c| c.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

static TAG_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]+>.*?</[^>]+>").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static IMAGE_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[Image #\d+\]\s*").unwrap());
static CWD_RAW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""cwd":"((?:[^"\\]|\\.)*)""#).unwrap());
static BRANCH_RAW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""gitBranch":"((?:[^"\\]|\\.)*)""#).unwrap());
static PROMPT_RAW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""role":"user","content":(?:"((?:[^"\\]|\\.)*)"|\[\{"type":"text","text":"((?:[^"\\]|\\.)*)")"#).unwrap()
});
static ID_RAW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""id":"((?:[^"\\]|\\.)*)""#).unwrap());
static GIT_BRANCH_RAW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""git":\{[^{}]*?"branch":"((?:[^"\\]|\\.)*)""#).unwrap());
static SOURCE_RAW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""source":(\{|"([^"]*)")"#).unwrap());

fn clean_prompt(s: &str) -> String {
    let t = TAG_BLOCK.replace_all(s, " ");
    SPACES.replace_all(&t, " ").trim().to_string()
}

fn first_80(s: &str) -> String {
    // JavaScript's slice(0, 80) counts UTF-16 units; titles must match the other app's.
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        units += c.len_utf16();
        if units > 80 {
            break;
        }
        out.push(c);
    }
    out
}

fn json_string(re: &Regex, text: &str) -> Option<String> {
    let caps = re.captures(text)?;
    let raw = caps.get(1).or_else(|| caps.get(2))?.as_str();
    serde_json::from_str::<String>(&format!("\"{raw}\"")).ok()
}

fn file_times(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as f64 * 1000.0 + d.subsec_nanos() as f64 / 1e6)
        .unwrap_or(0.0)
}

fn read_claude_meta(file: &Path, meta: &std::fs::Metadata) -> Option<Session> {
    let size = meta.len();
    let mut f = File::open(file).ok()?;
    let head = read_slice(&mut f, 0, HEAD.min(size));
    let (mut cwd, mut first_prompt, mut title, mut custom, mut branch) = (None::<String>, None::<String>, None::<String>, None::<String>, None::<String>);
    let scan_titles = |o: &Value, title: &mut Option<String>, custom: &mut Option<String>| {
        let ty = o.get("type").and_then(Value::as_str).unwrap_or("");
        if ty == "custom-title" {
            if let Some(t) = o.get("customTitle").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                *custom = Some(t.to_string());
            }
        }
        if ty == "ai-title" {
            if let Some(t) = o.get("aiTitle").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                *title = Some(t.to_string());
            }
        }
    };
    for o in parse_lines(&head, false, size > HEAD) {
        if cwd.is_none() {
            cwd = o.get("cwd").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        }
        if let Some(b) = o.get("gitBranch").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            branch = Some(b.to_string());
        }
        if first_prompt.is_none()
            && o.get("type").and_then(Value::as_str) == Some("user")
            && !o.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
            && !o.get("isSidechain").and_then(Value::as_bool).unwrap_or(false)
        {
            if let Some(content) = o.get("message").and_then(|m| m.get("content")) {
                let t = clean_prompt(&text_of_content(content));
                if !t.is_empty() && !t.starts_with("Caveat:") {
                    first_prompt = Some(t);
                }
            }
        }
        scan_titles(&o, &mut title, &mut custom);
        if o.get("type").and_then(Value::as_str) == Some("summary") && title.is_none() {
            if let Some(t) = o.get("summary").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                title = Some(t.to_string());
            }
        }
    }
    // A line with an inline image can be larger than the head and gets dropped as partial;
    // pull the fields straight out of the raw text instead.
    if cwd.is_none() {
        cwd = json_string(&CWD_RAW, &head);
    }
    if first_prompt.is_none() {
        if let Some(t) = json_string(&PROMPT_RAW, &head) {
            let t = clean_prompt(&IMAGE_PREFIX.replace(&t, ""));
            if !t.is_empty() {
                first_prompt = Some(t);
            }
        }
    }
    if size > HEAD {
        let start = HEAD.max(size.saturating_sub(TAIL));
        let tail = read_slice(&mut f, start, size - start);
        for o in parse_lines(&tail, true, false) {
            scan_titles(&o, &mut title, &mut custom);
            if cwd.is_none() {
                cwd = o.get("cwd").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            }
            if let Some(b) = o.get("gitBranch").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                branch = Some(b.to_string());
            }
        }
    }
    if branch.is_none() {
        branch = json_string(&BRANCH_RAW, &head).filter(|b| !b.is_empty());
    }
    Some(Session {
        id: file.file_stem()?.to_string_lossy().to_string(),
        agent: "claude".into(),
        file: file.to_path_buf(),
        cwd: cwd.map(PathBuf::from),
        branch: branch.filter(|b| b != "HEAD"),
        title: custom.or(title).or_else(|| first_prompt.map(|p| first_80(&p))),
        mtime: file_times(meta),
        size,
        provider: None,
        worktree: None,
    })
}

struct CodexMeta {
    session: Session,
    first_prompt: Option<String>,
}

fn read_codex_meta(file: &Path, meta: &std::fs::Metadata) -> Option<CodexMeta> {
    let size = meta.len();
    if size == 0 {
        return None;
    }
    let mut f = File::open(file).ok()?;
    let head = read_slice(&mut f, 0, CODEX_HEAD.min(size));
    let mut info: Option<Value> = None;
    let mut first_prompt: Option<String> = None;
    for o in parse_lines(&head, false, size > CODEX_HEAD) {
        let payload = o.get("payload");
        if o.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(p) = payload.filter(|p| !p.is_null()) {
                info = Some(p.clone());
            }
        }
        if first_prompt.is_some() {
            continue;
        }
        let Some(pl) = payload else { continue };
        let ty = o.get("type").and_then(Value::as_str);
        if ty == Some("event_msg") && pl.get("type").and_then(Value::as_str) == Some("user_message") {
            first_prompt = pl.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()).map(str::to_string);
        }
        if first_prompt.is_none()
            && ty == Some("response_item")
            && pl.get("type").and_then(Value::as_str) == Some("message")
            && pl.get("role").and_then(Value::as_str) == Some("user")
        {
            first_prompt = pl.get("content").and_then(Value::as_array).and_then(|c| {
                c.iter()
                    .find(|x| {
                        x.get("type").and_then(Value::as_str) == Some("input_text")
                            && x.get("text").and_then(Value::as_str).is_some_and(|t| !t.is_empty() && !t.trim_start().starts_with('<'))
                    })
                    .and_then(|x| x.get("text").and_then(Value::as_str))
                    .map(str::to_string)
            });
        }
    }
    // session_meta carries the full system prompt and can be longer than the head.
    let (id, cwd, source_ok, git_branch);
    match &info {
        Some(p) => {
            id = p.get("id").and_then(Value::as_str).map(str::to_string);
            cwd = p.get("cwd").and_then(Value::as_str).map(str::to_string);
            source_ok = p.get("source").and_then(Value::as_str).is_some_and(|s| s != "exec");
            git_branch = p.get("git").and_then(|g| g.get("branch")).and_then(Value::as_str).map(str::to_string);
        }
        None => {
            id = json_string(&ID_RAW, &head);
            cwd = json_string(&CWD_RAW, &head);
            source_ok = SOURCE_RAW
                .captures(&head)
                .is_some_and(|c| c.get(2).is_some_and(|s| s.as_str() != "exec"));
            git_branch = json_string(&GIT_BRANCH_RAW, &head);
        }
    }
    // Sub-agent threads and headless `codex exec` runs aren't conversations to resume.
    let (id, cwd) = (id.filter(|s| !s.is_empty())?, cwd.filter(|s| !s.is_empty())?);
    if !source_ok {
        return None;
    }
    Some(CodexMeta {
        session: Session {
            id,
            agent: "codex".into(),
            file: file.to_path_buf(),
            cwd: Some(PathBuf::from(cwd)),
            branch: git_branch.filter(|b| !b.is_empty()),
            title: None,
            mtime: file_times(meta),
            size,
            provider: None,
            worktree: None,
        },
        first_prompt: first_prompt.map(|p| first_80(&clean_prompt(&p))).filter(|p| !p.is_empty()),
    })
}

// Results by file, kept while the file's time and size stay the same.
#[derive(Clone)]
enum Cached {
    Claude(Option<Session>),
    Codex(Option<(Session, Option<String>)>),
}
static META_CACHE: LazyLock<Mutex<HashMap<PathBuf, (f64, u64, Cached)>>> = LazyLock::new(Default::default);

fn cached_claude(file: &Path) -> Option<Session> {
    let meta = std::fs::metadata(file).ok()?;
    let (m, s) = (file_times(&meta), meta.len());
    let mut cache = META_CACHE.lock().unwrap();
    if let Some((cm, cs, Cached::Claude(v))) = cache.get(file) {
        if *cm == m && *cs == s {
            return v.clone();
        }
    }
    let v = read_claude_meta(file, &meta);
    cache.insert(file.to_path_buf(), (m, s, Cached::Claude(v.clone())));
    v
}

fn cached_codex(file: &Path) -> Option<(Session, Option<String>)> {
    let meta = std::fs::metadata(file).ok()?;
    let (m, s) = (file_times(&meta), meta.len());
    let mut cache = META_CACHE.lock().unwrap();
    if let Some((cm, cs, Cached::Codex(v))) = cache.get(file) {
        if *cm == m && *cs == s {
            return v.clone();
        }
    }
    let v = read_codex_meta(file, &meta).map(|c| (c.session, c.first_prompt));
    cache.insert(file.to_path_buf(), (m, s, Cached::Codex(v.clone())));
    v
}

fn read_codex_names(index: &Path) -> HashMap<String, String> {
    let mut names = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(index) {
        for o in parse_lines(&text, false, false) {
            if let (Some(id), Some(name)) = (o.get("id").and_then(Value::as_str), o.get("thread_name").and_then(Value::as_str)) {
                if !id.is_empty() && !name.is_empty() {
                    names.insert(id.to_string(), name.to_string());
                }
            }
        }
    }
    names
}

fn list_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => list_files(&p, out),
            Ok(_) if p.extension().is_some_and(|x| x == "jsonl") => out.push(p),
            _ => {}
        }
    }
}

/// Fallback when no session records a folder: "C--Users-foo-bar" -> "C:\Users\foo\bar" (lossy).
fn decode_dir_name(name: &str) -> String {
    let b = name.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && name.get(1..3) == Some("--") {
        return format!("{}:\\{}", name[..1].to_uppercase(), name[3..].replace('-', "\\"));
    }
    name.replace('-', "/")
}

fn norm(p: &Path) -> String {
    let full = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    full.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
}

fn base_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string())
}

fn is_inside(child: &Path, parent: &Path) -> bool {
    let c = norm(child);
    let p = norm(parent);
    c == p || c.starts_with(&format!("{p}\\")) || c.starts_with(&format!("{p}/"))
}

/// A custom provider's own folder, whose sessions carry its id.
#[derive(Clone, Debug)]
pub struct ProviderHome {
    pub id: String,
    pub cli: String,
    pub home: PathBuf,
}

/// Every project with its sessions, in the order the user set. May add newly found projects to the
/// order in `config`; the bool says whether it changed and should be saved.
pub fn scan_projects(paths: &Paths, config: &mut Config, providers: &[ProviderHome]) -> (Vec<Project>, bool) {
    let mut projects: HashMap<String, Project> = HashMap::new();
    let mut claude_roots = vec![(paths.claude_projects(), None)];
    claude_roots.extend(providers.iter().filter(|h| h.cli == "claude").map(|h| (h.home.join("projects"), Some(h.id.clone()))));
    let mut codex_roots = vec![(paths.codex_home.clone(), None)];
    codex_roots.extend(providers.iter().filter(|h| h.cli == "codex").map(|h| (h.home.clone(), Some(h.id.clone()))));

    for (root, provider) in &claude_roots {
        let Ok(dirs) = std::fs::read_dir(root) else { continue };
        for d in dirs.flatten() {
            if !d.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(d.path()) else { continue };
            let mut sessions = Vec::new();
            for f in files.flatten() {
                let p = f.path();
                if !p.extension().is_some_and(|x| x == "jsonl") {
                    continue;
                }
                if let Some(mut s) = cached_claude(&p) {
                    if s.size == 0 {
                        continue;
                    }
                    s.provider = provider.clone();
                    sessions.push(s);
                }
            }
            let cwd = sessions
                .iter()
                .find_map(|s| s.cwd.clone())
                .unwrap_or_else(|| PathBuf::from(decode_dir_name(&d.file_name().to_string_lossy())));
            let key = norm(&cwd);
            let proj = projects.entry(key).or_insert_with(|| Project {
                name: base_name(&cwd),
                path: cwd.clone(),
                sessions: Vec::new(),
                mtime: 0.0,
                exists: false,
            });
            proj.sessions.extend(sessions.into_iter().filter(|s| s.title.is_some())); // no prompt yet
        }
    }

    let chats = paths.codex_chats();
    for (home, provider) in &codex_roots {
        let names = read_codex_names(&home.join("session_index.jsonl"));
        let mut files = Vec::new();
        list_files(&home.join("sessions"), &mut files);
        for f in files {
            let Some((mut s, first)) = cached_codex(&f) else { continue };
            let Some(title) = names.get(&s.id).cloned().or(first) else { continue }; // nothing typed yet
            s.title = Some(title);
            s.provider = provider.clone();
            let cwd = s.cwd.clone().unwrap_or_default();
            let chat = is_inside(&cwd, &chats);
            let proj_path = if chat { chats.clone() } else { cwd };
            let name = if chat { "Codex chats".to_string() } else { base_name(&proj_path) };
            let key = norm(&proj_path);
            let proj = projects.entry(key).or_insert_with(|| Project {
                name,
                path: proj_path.clone(),
                sessions: Vec::new(),
                mtime: 0.0,
                exists: false,
            });
            proj.sessions.push(s);
        }
    }

    // Sessions in a linked worktree are listed under their repository's project.
    let registry = Registry::load(paths);
    let keys: Vec<String> = projects.keys().cloned().collect();
    for key in keys {
        let (empty, path) = {
            let p = &projects[&key];
            (p.sessions.is_empty(), p.path.clone())
        };
        if empty {
            continue;
        }
        let Some(wt) = registry.session_worktree(&path) else { continue };
        let moved = projects.remove(&key).unwrap();
        let main_key = norm(&wt.main);
        let main = projects.entry(main_key).or_insert_with(|| Project {
            name: base_name(&wt.main),
            path: wt.main.clone(),
            sessions: Vec::new(),
            mtime: 0.0,
            exists: false,
        });
        main.sessions.extend(moved.sessions.into_iter().map(|mut s| {
            s.worktree = Some(wt.clone());
            s
        }));
    }

    for p in config.list("extraProjects") {
        let path = PathBuf::from(&p);
        projects.entry(norm(&path)).or_insert_with(|| Project {
            name: base_name(&path),
            path,
            sessions: Vec::new(),
            mtime: 0.0,
            exists: false,
        });
    }

    let mut hidden: Vec<String> = config.list("hiddenProjects").iter().map(|p| norm(Path::new(p))).collect();
    hidden.push(norm(&paths.data_dir.join("usage-probe"))); // the usage check's own folder
    let mut list: Vec<Project> = projects.into_iter().filter(|(k, _)| !hidden.contains(k)).map(|(_, p)| p).collect();
    for p in &mut list {
        p.sessions.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        p.mtime = p.sessions.first().map_or(0.0, |s| s.mtime);
        p.exists = p.path.exists();
    }
    // Projects keep the order the user set. Ones not placed yet go at the end, newest first, and
    // from then on stay where they are.
    list.sort_by(|a, b| b.mtime.total_cmp(&a.mtime).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let mut saved = config.list("projectOrder");
    let mut order: Vec<String> = saved.iter().map(|p| norm(Path::new(p))).collect();
    let mut changed = false;
    for p in &list {
        let k = norm(&p.path);
        if !order.contains(&k) {
            saved.push(p.path.to_string_lossy().to_string());
            order.push(k);
            changed = true;
        }
    }
    if changed {
        config.set("projectOrder", Value::from(saved));
    }
    list.sort_by_key(|p| order.iter().position(|o| *o == norm(&p.path)).unwrap_or(usize::MAX));
    (list, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (Paths, PathBuf) {
        let d = std::env::temp_dir().join(format!("crew-sessions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join(".claude/projects")).unwrap();
        std::fs::create_dir_all(d.join(".codex/sessions/2026/11/05")).unwrap();
        (Paths { home: d.clone(), data_dir: d.join("data"), codex_home: d.join(".codex") }, d)
    }

    #[test]
    fn claude_titles_prefer_custom_then_ai_then_prompt() {
        let (paths, d) = scratch();
        let dir = d.join(".claude/projects/C--x");
        std::fs::create_dir_all(&dir).unwrap();
        let line = |o: serde_json::Value| format!("{o}\n");
        let cwd = d.join("work").to_string_lossy().to_string();
        std::fs::write(dir.join("s1.jsonl"), line(serde_json::json!({"type":"user","cwd":cwd,"gitBranch":"main","message":{"content":"Fix  <x>hidden</x>  the bug"}}))).unwrap();
        std::fs::write(
            dir.join("s2.jsonl"),
            line(serde_json::json!({"type":"user","cwd":cwd,"message":{"content":"first"}})) + &line(serde_json::json!({"type":"ai-title","aiTitle":"AI name"})),
        )
        .unwrap();
        std::fs::write(
            dir.join("s3.jsonl"),
            line(serde_json::json!({"type":"user","cwd":cwd,"message":{"content":"first"}}))
                + &line(serde_json::json!({"type":"ai-title","aiTitle":"AI name"}))
                + &line(serde_json::json!({"type":"custom-title","customTitle":"Mine"})),
        )
        .unwrap();
        std::fs::write(dir.join("s4.jsonl"), line(serde_json::json!({"type":"user","cwd":cwd,"isMeta":true,"message":{"content":"meta only"}}))).unwrap();
        let mut c = Config::load(&paths);
        let (list, changed) = scan_projects(&paths, &mut c, &[]);
        assert!(changed, "the new project joins the order");
        assert_eq!(list.len(), 1);
        let by = |id: &str| list[0].sessions.iter().find(|s| s.id == id).cloned();
        assert_eq!(by("s1").unwrap().title.as_deref(), Some("Fix the bug"));
        assert_eq!(by("s1").unwrap().branch.as_deref(), Some("main"));
        assert_eq!(by("s2").unwrap().title.as_deref(), Some("AI name"));
        assert_eq!(by("s3").unwrap().title.as_deref(), Some("Mine"));
        assert!(by("s4").is_none(), "no prompt yet");
    }

    #[test]
    fn codex_skips_exec_and_subagents_and_uses_names() {
        let (paths, d) = scratch();
        let cwd = d.join("work").to_string_lossy().to_string();
        let dir = d.join(".codex/sessions/2026/11/05");
        let meta = |id: &str, source: serde_json::Value| {
            format!(
                "{}\n{}\n",
                serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":cwd,"source":source,"git":{"branch":"dev"}}}),
                serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"do the thing"}})
            )
        };
        std::fs::write(dir.join("rollout-a.jsonl"), meta("a", "cli".into())).unwrap();
        std::fs::write(dir.join("rollout-b.jsonl"), meta("b", "exec".into())).unwrap();
        std::fs::write(dir.join("rollout-c.jsonl"), meta("c", serde_json::json!({"subagent":{}}))).unwrap();
        std::fs::write(dir.join("rollout-d.jsonl"), meta("d", "cli".into())).unwrap();
        std::fs::write(d.join(".codex/session_index.jsonl"), format!("{}\n", serde_json::json!({"id":"d","thread_name":"Named"}))).unwrap();
        let mut c = Config::load(&paths);
        let (list, _) = scan_projects(&paths, &mut c, &[]);
        let ids: Vec<&str> = list[0].sessions.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"a") && ids.contains(&"d") && ids.len() == 2, "{ids:?}");
        let a = list[0].sessions.iter().find(|s| s.id == "a").unwrap();
        assert_eq!(a.title.as_deref(), Some("do the thing"));
        assert_eq!(a.branch.as_deref(), Some("dev"));
        assert_eq!(list[0].sessions.iter().find(|s| s.id == "d").unwrap().title.as_deref(), Some("Named"));
    }

    #[test]
    fn a_linked_worktrees_sessions_join_its_repository() {
        let (paths, d) = scratch();
        let main = d.join("repo");
        let wt = d.join("repo-feature");
        std::fs::create_dir_all(main.join(".git/worktrees/feature")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}
", main.join(".git/worktrees/feature").display())).unwrap();
        let dir = d.join(".claude/projects/C--wt");
        std::fs::create_dir_all(&dir).unwrap();
        let line = serde_json::json!({"type":"user","cwd":wt.to_string_lossy(),"message":{"content":"in the worktree"}});
        std::fs::write(dir.join("w1.jsonl"), format!("{line}
")).unwrap();
        let mut c = Config::load(&paths);
        let (list, _) = scan_projects(&paths, &mut c, &[]);
        assert_eq!(list.len(), 1, "the worktree isn't a project of its own");
        assert_eq!(list[0].name, "repo");
        let s = &list[0].sessions[0];
        let w = s.worktree.as_ref().expect("marked as in a worktree");
        assert_eq!((w.name.as_str(), s.title.as_deref()), ("repo-feature", Some("in the worktree")));
    }

    #[test]
    fn hidden_and_ordered() {
        let (paths, d) = scratch();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let a = d.join("aaa").to_string_lossy().to_string();
        let b = d.join("bbb").to_string_lossy().to_string();
        let h = d.join("hid").to_string_lossy().to_string();
        std::fs::write(
            paths.config_file(),
            serde_json::json!({"extraProjects":[a,b,h],"hiddenProjects":[h],"projectOrder":[b,a]}).to_string(),
        )
        .unwrap();
        let mut c = Config::load(&paths);
        let (list, changed) = scan_projects(&paths, &mut c, &[]);
        assert!(!changed);
        assert_eq!(list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["bbb", "aaa"]);
    }
}
