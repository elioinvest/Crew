//! Antigravity's CLI, `agy`: its conversations are listed in a SQLite index it keeps, and one is
//! resumed with `agy --conversation <id>`. Like Codex it names a new conversation itself, so a new
//! session is matched to its row later by folder and time.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::config::Paths;
use crate::sessions::Session;

pub fn index(paths: &Paths) -> PathBuf {
    paths.home.join(".gemini").join("antigravity-cli").join("conversation_summaries.db")
}

/// The folder a workspace URI names: "file:///c%3A/Users/me/app" is C:\Users\me\app.
fn folder_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.trim().strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                bytes.push(b);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let path = String::from_utf8(bytes).ok()?;
    // "/c:/Users/me" on Windows.
    let path = match path.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => &path[1..],
        _ => path.as_str(),
    };
    let path = path.trim_end_matches('/');
    (!path.is_empty()).then(|| PathBuf::from(path.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

/// The first workspace folder: the column holds a JSON list of URIs, or URIs one after another.
fn first_folder(uris: &str) -> Option<PathBuf> {
    if let Ok(list) = serde_json::from_str::<Vec<String>>(uris) {
        return list.iter().find_map(|u| folder_of(u));
    }
    uris.split(|c: char| c.is_whitespace() || c == ',' || c == ';').find_map(folder_of)
}

/// Milliseconds since 1970 for the index's times, which are written as text.
fn millis(text: &str) -> Option<f64> {
    let t = text.trim();
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
        return Some(d.timestamp_millis() as f64);
    }
    for f in ["%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%d %H:%M:%S%.f%z", "%Y-%m-%d %H:%M:%S%.f %z"] {
        if let Ok(d) = chrono::DateTime::parse_from_str(t, f) {
            return Some(d.timestamp_millis() as f64);
        }
    }
    chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S%.f").ok().map(|d| d.and_utc().timestamp_millis() as f64)
}

/// A model as agy's /model shows it, with the effort levels it comes in (none for some).
#[derive(Clone, Debug, PartialEq)]
pub struct AgyModel {
    pub label: String,
    pub efforts: Vec<String>,
}

/// `agy models` prints "gemini-3.8-flash-high<tab>Gemini 3.8 Flash (High)", one line per model and
/// effort; /model lists each model once, with the effort beside it.
fn group_models(text: &str) -> Vec<AgyModel> {
    let mut out: Vec<AgyModel> = Vec::new();
    for line in text.lines() {
        let Some((id, label)) = line.split_once('\t') else { continue };
        let (id, label) = (id.trim(), label.trim());
        let effort = ["low", "medium", "high"].into_iter().find(|e| id.ends_with(&format!("-{e}")) && label.to_lowercase().ends_with(&format!("({e})")));
        let base = match effort {
            Some(e) => label[..label.len() - e.len() - 2].trim_end().to_string(),
            None => label.to_string(),
        };
        let at = match out.iter().position(|m| m.label == base) {
            Some(i) => i,
            None => {
                out.push(AgyModel { label: base, efforts: Vec::new() });
                out.len() - 1
            }
        };
        if let Some(e) = effort {
            out[at].efforts.push(e.to_string());
        }
    }
    for m in &mut out {
        m.efforts.sort_by_key(|e| ["low", "medium", "high"].iter().position(|x| x == e));
    }
    out
}

/// agy's models, from `agy models`; it asks Antigravity's servers, so it takes a moment.
pub fn models(command: &str) -> Option<Vec<AgyModel>> {
    use std::os::windows::process::CommandExt;
    let command = if command.is_empty() { "agy" } else { command };
    let launch = crate::agents::shell_command(&[command.to_string(), "models".to_string()]);
    let mut env = crate::env::clean_env();
    env.extend(launch.env);
    let out = std::process::Command::new(&launch.program)
        .args(&launch.args)
        .env_clear()
        .envs(env)
        .creation_flags(0x0800_0000) // no console window
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let list = group_models(&String::from_utf8_lossy(&out.stdout));
    (!list.is_empty()).then_some(list)
}

/// The conversations to list: the top-level ones that were started from a folder and have had a
/// prompt. Sub-agents' conversations aren't ones to resume.
pub fn sessions(paths: &Paths) -> Vec<Session> {
    let db = index(paths);
    read(&db).unwrap_or_default()
}

fn read(db: &Path) -> rusqlite::Result<Vec<Session>> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let size = std::fs::metadata(db).map_or(0, |m| m.len());
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let mut q = conn.prepare(
        "SELECT conversation_id, title, preview, CAST(last_modified_time AS TEXT), workspace_uris
         FROM conversation_summaries
         WHERE nesting_depth = 0 AND parent_conversation_id = '' AND step_count > 0",
    )?;
    let rows = q.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?))
    })?;
    let mut out = Vec::new();
    for (id, title, preview, modified, uris) in rows.flatten() {
        let Some(cwd) = first_folder(&uris) else { continue };
        let title = [title, preview].into_iter().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).find(|t| !t.is_empty());
        out.push(Session {
            id,
            agent: "agy".into(),
            file: db.to_path_buf(),
            cwd: Some(cwd),
            branch: None,
            title: Some(title.unwrap_or_else(|| "agy".into())),
            mtime: modified.as_deref().and_then(millis).unwrap_or(0.0),
            size,
            provider: None,
            worktree: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_uris_become_folders() {
        assert_eq!(folder_of("file:///c%3A/Users/me/my%20app"), Some(PathBuf::from(r"c:\Users\me\my app")));
        assert_eq!(first_folder(r#"["file:///d:/work/x/"]"#), Some(PathBuf::from(r"d:\work\x")));
        assert_eq!(first_folder("file:///d:/a file:///d:/b"), Some(PathBuf::from(r"d:\a")));
        assert_eq!(first_folder(""), None);
        assert_eq!(folder_of("https://example.com"), None);
    }

    #[test]
    fn models_are_grouped_with_their_efforts() {
        let text = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\ngemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)\ngemini-3.8-flash-low\tGemini 3.8 Flash (Low)\nclaude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\ngpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n";
        let got = group_models(text);
        let want = |label: &str, efforts: &[&str]| AgyModel { label: label.into(), efforts: efforts.iter().map(|e| e.to_string()).collect() };
        assert_eq!(got, [want("Gemini 3.8 Flash", &["low", "medium", "high"]), want("Claude Sonnet 4.6 (Thinking)", &[]), want("GPT-OSS 120B", &["medium"])]);
    }

    #[test]
    fn times_in_the_forms_sqlite_keeps_them() {
        let want = 1_795_100_993_000.0;
        assert_eq!(millis("2026-11-19T23:09:53+08:00"), Some(want));
        assert_eq!(millis("2026-11-19 23:09:53+08:00"), Some(want));
        assert_eq!(millis("2026-11-19 23:09:53.000+08:00"), Some(want));
        assert_eq!(millis("2026-11-19 15:09:53"), Some(want));
        assert_eq!(millis("soon"), None);
    }

    #[test]
    fn lists_top_level_conversations_with_a_folder() {
        let d = std::env::temp_dir().join(format!("crew-agy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let db = d.join("conversation_summaries.db");
        let c = Connection::open(&db).unwrap();
        c.execute_batch(
            "CREATE TABLE conversation_summaries (conversation_id text, title text NOT NULL DEFAULT '', preview text NOT NULL DEFAULT '',
               step_count integer NOT NULL DEFAULT 0, last_modified_time datetime NOT NULL, workspace_uris text NOT NULL,
               parent_conversation_id text NOT NULL DEFAULT '', nesting_depth integer NOT NULL DEFAULT 0, PRIMARY KEY (conversation_id));
             INSERT INTO conversation_summaries VALUES ('a', 'Fix the build', '', 4, '2026-11-19 23:09:53+08:00', '[\"file:///d:/work/app\"]', '', 0);
             INSERT INTO conversation_summaries VALUES ('b', '', 'what does  this do', 2, '2026-11-19 23:10:00+08:00', 'file:///d:/work/app', '', 0);
             INSERT INTO conversation_summaries VALUES ('sub', 'x', '', 3, '2026-11-19 23:10:00+08:00', 'file:///d:/work/app', 'a', 1);
             INSERT INTO conversation_summaries VALUES ('empty', '', '', 0, '2026-11-19 23:10:00+08:00', 'file:///d:/work/app', '', 0);
             INSERT INTO conversation_summaries VALUES ('nowhere', 'y', '', 1, '2026-11-19 23:10:00+08:00', '', '', 0);",
        )
        .unwrap();
        drop(c);
        let mut got = read(&db).unwrap();
        got.sort_by(|a, b| a.id.cmp(&b.id));
        let summary: Vec<(&str, &str, &str)> = got.iter().map(|s| (s.id.as_str(), s.title.as_deref().unwrap(), s.agent.as_str())).collect();
        assert_eq!(summary, [("a", "Fix the build", "agy"), ("b", "what does this do", "agy")]);
        assert_eq!(got[0].cwd, Some(PathBuf::from(r"d:\work\app")));
        assert_eq!(got[0].mtime, 1_795_100_993_000.0);
        assert!(read(&d.join("missing.db")).unwrap().is_empty());
    }
}
