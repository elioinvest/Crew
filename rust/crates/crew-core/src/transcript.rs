//! A session's conversation for phones, read from the transcript file its agent writes: Claude Code's
//! projects/<dir>/<id>.jsonl, Codex's sessions/…/rollout-*-<id>.jsonl. Each line becomes zero or more
//! chat items: what you said, what the agent said, and the tools it ran.
//!
//! Item: { id, role, text, name?, ref?, error?, images?, ts }, role being one of
//!   user, assistant  what was said
//!   tool             a tool call (name, one-line summary); ref ties it to its result
//!   result           a tool's output (the start of it), for the call with the same ref
//!   thinking         a reasoning summary, when the agent writes one
//!   command, info    a slash command you ran, and notes: its output, an interruption, a compaction

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

const MAX_TEXT: usize = 20_000; // a very long message is cut for the phone
const MAX_RESULT: usize = 1500; // the start of a tool's output is enough on a phone

macro_rules! re {
    ($pattern:expr) => {{
        static R: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
        &*R
    }};
}

fn limit(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

fn cut(text: &str) -> String {
    limit(text, MAX_TEXT)
}

fn no_ansi(text: &str) -> String {
    re!(r"\x1b\[[0-9;]*m").replace_all(text, "").into_owned()
}

fn preview(text: &str) -> String {
    limit(no_ansi(text).trim(), MAX_RESULT)
}

/// A tool result's content: a string, or parts with text.
fn result_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                _ => match x.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                    Some(t) => Some(t.to_string()),
                    None => (x.get("type").and_then(Value::as_str) == Some("image")).then(|| "[image]".to_string()),
                },
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn first_line(text: &str) -> String {
    text.split('\n').next().unwrap_or("").chars().take(200).collect()
}

/// What a tool call did, in one line: the command it ran, the file it touched, what it searched.
pub fn tool_summary(input: &Value) -> String {
    let parsed;
    let o = match input {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => {
                parsed = v;
                &parsed
            }
            Err(_) => return first_line(s),
        },
        other => other,
    };
    if !o.is_object() {
        return String::new();
    }
    if let Some(parts) = o.get("command").and_then(Value::as_array) {
        return first_line(&parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "));
    }
    for key in ["command", "cmd", "description", "file_path", "path", "pattern", "url", "query", "prompt", "skill", "subject"] {
        if let Some(v) = o.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            return first_line(v);
        }
    }
    String::new()
}

/// Claude Code's own wrappers around slash commands and their output.
fn claude_user_text(text: &str) -> Option<(&'static str, String)> {
    if text.starts_with("<local-command-caveat>") || text.starts_with("Caveat:") {
        return None;
    }
    if let Some(cmd) = re!(r"<command-name>([^<]*)</command-name>").captures(text) {
        let args = re!(r"<command-args>([^<]*)</command-args>").captures(text).map_or(String::new(), |a| a[1].trim().to_string());
        let name = cmd[1].trim();
        return Some(("command", if args.is_empty() { name.to_string() } else { format!("{name} {args}") }));
    }
    if let Some(out) = re!(r"(?s)<local-command-stdout>(.*?)</local-command-stdout>").captures(text) {
        return (!out[1].trim().is_empty()).then(|| ("info", no_ansi(&out[1]).trim().to_string()));
    }
    let trimmed = text.trim();
    if trimmed.starts_with("[Request interrupted") {
        return Some(("info", trimmed.to_string()));
    }
    if let Some(bash) = re!(r"(?s)<bash-input>(.*?)</bash-input>").captures(text) {
        return Some(("command", format!("! {}", bash[1].trim())));
    }
    if let Some(out) = re!(r"(?s)<bash-stdout>(.*?)</bash-stdout>").captures(text) {
        return (!out[1].trim().is_empty()).then(|| ("info", preview(&out[1])));
    }
    if ["<bash-stderr>", "<system-reminder>", "<task-notification>"].iter().any(|p| trimmed.starts_with(p)) {
        return None;
    }
    let clean = re!(r"(?s)<system-reminder>.*?</system-reminder>").replace_all(text, "");
    let clean = clean.trim();
    (!clean.is_empty()).then(|| ("user", clean.to_string()))
}

fn timestamp(o: &Value) -> i64 {
    o.get("timestamp").and_then(Value::as_str).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map_or(0, |d| d.timestamp_millis())
}

fn item(id: String, ts: i64, role: &str, text: String) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(id));
    m.insert("ts".into(), json!(ts));
    m.insert("role".into(), json!(role));
    m.insert("text".into(), json!(text));
    m
}

fn is(o: &Value, key: &str) -> bool {
    o.get(key).is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)))
}

fn from_claude(o: &Value, n: usize) -> Vec<Value> {
    let ts = timestamp(o);
    let kind = o.get("type").and_then(Value::as_str).unwrap_or("");
    // Claude Code's own notes: "Conversation compacted", errors it shows in the session.
    if kind == "system" && !is(o, "isSidechain") && o.get("level").and_then(Value::as_str) != Some("debug") {
        if let Some(c) = o.get("content").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) {
            let plain = re!(r"<[^>]+>").replace_all(c, "");
            return vec![Value::Object(item(n.to_string(), ts, "info", preview(&plain)))];
        }
    }
    let Some(message) = o.get("message").filter(|m| !m.is_null()) else { return Vec::new() };
    if is(o, "isSidechain") || is(o, "isMeta") {
        return Vec::new();
    }
    let content = message.get("content");
    let mut out = Vec::new();
    if kind == "user" {
        let single;
        let parts: &[Value] = match content {
            Some(Value::String(s)) => {
                single = [json!({"type": "text", "text": s})];
                &single
            }
            Some(Value::Array(a)) => a,
            _ => &[],
        };
        let mut texts = Vec::new();
        let mut images = 0;
        for (i, p) in parts.iter().enumerate() {
            match p.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = p.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                        texts.push(t);
                    }
                }
                Some("image") => images += 1,
                Some("tool_result") => {
                    let body = p.get("content").map_or(String::new(), result_text);
                    let mut r = item(format!("{n}.r{i}"), ts, "result", preview(&body));
                    r.insert("ref".into(), p.get("tool_use_id").cloned().unwrap_or(Value::Null));
                    if is(p, "is_error") {
                        r.insert("error".into(), json!(true));
                    }
                    out.push(Value::Object(r));
                }
                _ => {}
            }
        }
        if texts.is_empty() && images == 0 {
            return out;
        }
        let said = claude_user_text(&texts.join("\n"));
        if said.is_some() || images > 0 {
            let (role, text) = said.unwrap_or(("user", String::new()));
            let mut m = item(n.to_string(), ts, role, text);
            if images > 0 {
                m.insert("images".into(), json!(images));
            }
            out.push(Value::Object(m));
        }
    } else if kind == "assistant" {
        for (i, p) in content.and_then(Value::as_array).map_or(&[][..], Vec::as_slice).iter().enumerate() {
            let id = format!("{n}.{i}");
            match p.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = p.get("text").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
                        out.push(Value::Object(item(id, ts, "assistant", cut(t))));
                    }
                }
                Some("thinking") => {
                    if let Some(t) = p.get("thinking").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
                        out.push(Value::Object(item(id, ts, "thinking", cut(t))));
                    }
                }
                Some("tool_use") => {
                    let mut m = item(id, ts, "tool", p.get("input").map_or(String::new(), tool_summary));
                    m.insert("name".into(), p.get("name").cloned().unwrap_or(Value::Null));
                    m.insert("ref".into(), p.get("id").cloned().unwrap_or(Value::Null));
                    out.push(Value::Object(m));
                }
                _ => {}
            }
        }
    }
    out
}

fn from_codex(o: &Value, n: usize) -> Vec<Value> {
    if o.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }
    let Some(p) = o.get("payload") else { return Vec::new() };
    let ts = timestamp(o);
    let kind = p.get("type").and_then(Value::as_str).unwrap_or("");
    let role = p.get("role").and_then(Value::as_str).unwrap_or("");
    match kind {
        "message" if role == "user" || role == "assistant" => {
            let Some(content) = p.get("content").and_then(Value::as_array) else { return Vec::new() };
            let texts: Vec<&str> = content
                .iter()
                .filter(|c| matches!(c.get("type").and_then(Value::as_str), Some("input_text" | "output_text")))
                .filter_map(|c| c.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()))
                .filter(|t| !re!(r"^\s*(<|# AGENTS\.md instructions)").is_match(t))
                .collect();
            let images = content.iter().filter(|c| c.get("type").and_then(Value::as_str) == Some("input_image")).count();
            if texts.is_empty() && images == 0 {
                return Vec::new();
            }
            let mut m = item(n.to_string(), ts, role, cut(texts.join("\n").trim()));
            if images > 0 {
                m.insert("images".into(), json!(images));
            }
            vec![Value::Object(m)]
        }
        "function_call" | "custom_tool_call" | "local_shell_call" => {
            let name = p.get("name").and_then(Value::as_str).unwrap_or(if kind == "local_shell_call" { "shell" } else { "tool" });
            let input = p.get("arguments").or_else(|| p.get("input")).or_else(|| p.get("action"));
            let mut m = item(n.to_string(), ts, "tool", input.map_or(String::new(), tool_summary));
            m.insert("name".into(), json!(name));
            m.insert("ref".into(), p.get("call_id").cloned().unwrap_or(Value::Null));
            vec![Value::Object(m)]
        }
        "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
            let mut out = p.get("output").cloned().unwrap_or(Value::Null);
            if let Some(text) = out.as_str() {
                if let Ok(j) = serde_json::from_str::<Value>(text) {
                    if let Some(inner) = j.get("output").or_else(|| j.get("content")).filter(|v| !v.is_null()) {
                        out = inner.clone();
                    }
                }
            }
            let mut m = item(n.to_string(), ts, "result", preview(&result_text(&out)));
            m.insert("ref".into(), p.get("call_id").cloned().unwrap_or(Value::Null));
            vec![Value::Object(m)]
        }
        "reasoning" => {
            let Some(summary) = p.get("summary").and_then(Value::as_array) else { return Vec::new() };
            let text = summary.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
            let text = text.trim();
            if text.is_empty() { Vec::new() } else { vec![Value::Object(item(n.to_string(), ts, "thinking", cut(text)))] }
        }
        _ => Vec::new(),
    }
}

/// Lines to items. `first` numbers the lines, so items keep their ids as the file grows.
pub fn parse(lines: &[&str], agent: &str, first: usize) -> Vec<Value> {
    let mut items = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let Ok(o) = serde_json::from_str::<Value>(l) else { continue };
        items.extend(if agent == "codex" { from_codex(&o, first + i) } else { from_claude(&o, first + i) });
    }
    items
}

/// Follows one session's file: [`Follower::read`] returns the items so far, then
/// [`Follower::poll`] the new ones.
pub struct Follower {
    file: PathBuf,
    agent: String,
    offset: u64,
    line: usize,
    rest: String,
}

impl Follower {
    pub fn new(file: &Path, agent: &str) -> Self {
        Self { file: file.to_path_buf(), agent: agent.to_string(), offset: 0, line: 0, rest: String::new() }
    }

    /// The end of the file, read further back until it holds enough messages: in a long session the
    /// last few MB can be a handful of huge lines (screenshots, request records).
    pub fn read(&mut self) -> Vec<Value> {
        const ENOUGH: usize = 200;
        const MOST: u64 = 64 * 1024 * 1024;
        let Ok(size) = std::fs::metadata(&self.file).map(|m| m.len()) else { return Vec::new() };
        let mut tail: u64 = 4 * 1024 * 1024;
        loop {
            let start = size.saturating_sub(tail);
            let mut text = self.slice(start, size);
            if start > 0 {
                // A partial first line.
                text = text.find('\n').map_or(String::new(), |i| text[i + 1..].to_string());
            }
            self.line = 0;
            self.rest.clear();
            let items = self.take(&text);
            if items.len() >= ENOUGH || start == 0 || tail >= MOST {
                self.offset = size;
                return items;
            }
            tail *= 4;
        }
    }

    pub fn poll(&mut self) -> Vec<Value> {
        let Ok(size) = std::fs::metadata(&self.file).map(|m| m.len()) else { return Vec::new() };
        if size < self.offset {
            // Rewritten.
            self.offset = 0;
            self.rest.clear();
        }
        if size == self.offset {
            return Vec::new();
        }
        let text = self.slice(self.offset, size);
        self.offset = size;
        self.take(&text)
    }

    fn take(&mut self, text: &str) -> Vec<Value> {
        let joined = std::mem::take(&mut self.rest) + text;
        let mut lines: Vec<&str> = joined.split('\n').collect();
        // Not finished yet.
        self.rest = lines.pop().unwrap_or("").to_string();
        let items = parse(&lines, &self.agent, self.line);
        self.line += lines.len();
        items
    }

    fn slice(&self, start: u64, end: u64) -> String {
        let mut buf = Vec::new();
        if let Ok(mut f) = std::fs::File::open(&self.file) {
            if f.seek(SeekFrom::Start(start)).is_ok() {
                let _ = f.take(end - start).read_to_end(&mut buf);
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(items: &[Value]) -> Vec<String> {
        items.iter().map(|i| i["role"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn reads_claude() {
        let lines = [
            r#"{"type":"user","timestamp":"2026-11-05T01:02:03.000Z","message":{"content":"Fix it<system-reminder>hidden</system-reminder>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":" Done "},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls\nrm"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"bad"}]}]}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/model</command-name><command-args>opus</command-args>"}}"#,
            r#"{"type":"user","message":{"content":"<local-command-stdout></local-command-stdout>"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"image"},{"type":"text","text":"see"}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":"no"}}"#,
            r#"{"type":"system","content":"Conversation <b>compacted</b>"}"#,
            "not json",
        ];
        let items = parse(&lines, "claude", 10);
        assert_eq!(roles(&items), ["user", "thinking", "assistant", "tool", "result", "command", "user", "info"]);
        assert_eq!(items[0]["text"], "Fix it");
        assert_eq!(items[0]["id"], "10");
        assert_eq!(items[0]["ts"], 1793840523000i64);
        assert_eq!(items[2]["text"], "Done");
        assert_eq!(items[3]["text"], "ls");
        assert_eq!(items[3]["ref"], "t1");
        assert_eq!(items[4]["error"], true);
        assert_eq!(items[4]["id"], "12.r0");
        assert_eq!(items[5]["text"], "/model opus");
        assert_eq!(items[6]["images"], 1);
        assert_eq!(items[7]["text"], "Conversation compacted");
    }

    #[test]
    fn reads_codex() {
        let lines = [
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<env>x</env>"},{"type":"input_text","text":"hello"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"command\":[\"git\",\"status\"]}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"{\"output\":\"clean\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"thought"}]}}"#,
            r#"{"type":"event_msg","payload":{}}"#,
        ];
        let items = parse(&lines, "codex", 0);
        assert_eq!(roles(&items), ["user", "tool", "result", "thinking"]);
        assert_eq!(items[0]["text"], "hello");
        assert_eq!(items[1]["text"], "git status");
        assert_eq!(items[2]["text"], "clean");
    }

    #[test]
    fn follows_a_growing_file() {
        let file = std::env::temp_dir().join(format!("crew-transcript-{}.jsonl", uuid::Uuid::new_v4()));
        let say = |t: &str| format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{t}\"}}}}\n");
        std::fs::write(&file, say("one")).unwrap();
        let mut f = Follower::new(&file, "claude");
        assert_eq!(f.read().len(), 1);
        assert!(f.poll().is_empty());
        // A line that's only half written waits for the rest.
        let two = say("two");
        let (a, b) = two.split_at(10);
        std::fs::write(&file, say("one") + a).unwrap();
        assert!(f.poll().is_empty());
        std::fs::write(&file, say("one") + a + b).unwrap();
        let items = f.poll();
        assert_eq!(items[0]["text"], "two");
        assert_eq!(items[0]["id"], "1", "ids stay the same as the file grows");
        let _ = std::fs::remove_file(&file);
    }
}
