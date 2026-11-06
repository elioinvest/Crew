//! The image behind an "[Image #N]" in a session's terminal. Claude Code and Codex show a pasted image
//! only as that placeholder; the image itself is in the session's transcript. Gives a file to show:
//! the original when it still exists, else the image written out of the transcript into a cache folder.
//!   Claude: numbers images across the session; a message's imagePasteIds name its image blocks in order.
//!   Codex: numbers them per message, as <image name=[Image #N] path="…"> followed by the image. When
//!     several messages have an #N, the one whose text best matches the terminal line wins.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use regex::Regex;
use serde_json::Value;

use crate::config::Paths;
use crate::{composer, providers};

const MAX_TEXT: usize = 2000; // of a message's text, kept for matching it to a terminal line

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    n: u32,
    text: String,
    start: u64,
    len: usize,
    block: usize,
    path: Option<String>,
    ext: &'static str,
}

#[derive(Default)]
struct Index {
    scanned: u64,
    entries: Vec<Entry>,
}

// Transcripts can be hundreds of MB and only grow, so each file is scanned once, and later only what
// was added.
static INDEXES: LazyLock<Mutex<HashMap<PathBuf, Index>>> = LazyLock::new(Mutex::default);
static WORTH_PARSING: LazyLock<regex::bytes::Regex> = LazyLock::new(|| regex::bytes::Regex::new(r#"imagePasteIds|"input_image""#).unwrap());
static CODEX_IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^<image name=\[Image #(\d+)\](?: path="((?:[^"\\]|\\.)*)")?"#).unwrap());
static FILES_MENTIONED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*# Files mentioned").unwrap());
static DATA_MIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^data:([^;]+);").unwrap());
static DATA_BODY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)^data:[^;]+;base64,(.*)$").unwrap());

fn text_of(content: &[Value]) -> String {
    let parts: Vec<&str> = content
        .iter()
        .filter(|b| matches!(b.get("type").and_then(Value::as_str), Some("text" | "input_text")))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .filter(|t| !t.is_empty() && !t.starts_with("<image") && !t.starts_with("</image") && !FILES_MENTIONED.is_match(t))
        .collect();
    parts.join(" ").chars().take(MAX_TEXT).collect()
}

fn ext_of(mime: Option<&str>) -> &'static str {
    match mime {
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        _ => "png",
    }
}

/// Adds the pasted images in one transcript line.
fn images_in_line(line: &[u8], start: u64, entries: &mut Vec<Entry>) {
    if !WORTH_PARSING.is_match(line) {
        return;
    }
    let Ok(o) = serde_json::from_slice::<Value>(line) else { return };
    let len = line.len();
    if let (Some(ids), Some(content)) = (o.get("imagePasteIds").and_then(Value::as_array), o.pointer("/message/content").and_then(Value::as_array)) {
        // Claude
        let text = text_of(content);
        let mut k = 0;
        for (block, b) in content.iter().enumerate() {
            if b.get("type").and_then(Value::as_str) != Some("image") {
                continue;
            }
            if let Some(n) = ids.get(k).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))) {
                entries.push(Entry { n: n as u32, text: text.clone(), start, len, block, path: None, ext: ext_of(b.pointer("/source/media_type").and_then(Value::as_str)) });
            }
            k += 1;
        }
        return;
    }
    // Codex
    let is_user = o.get("type").and_then(Value::as_str) == Some("response_item") && o.pointer("/payload/role").and_then(Value::as_str) == Some("user");
    let Some(content) = o.pointer("/payload/content").and_then(Value::as_array).filter(|_| is_user) else { return };
    let text = text_of(content);
    let mut pending: Option<(u32, Option<String>)> = None;
    for (block, b) in content.iter().enumerate() {
        let kind = b.get("type").and_then(Value::as_str);
        let named = (kind == Some("input_text")).then(|| CODEX_IMAGE.captures(b.get("text").and_then(Value::as_str).unwrap_or(""))).flatten();
        if let Some(m) = named {
            pending = m[1].parse().ok().map(|n| (n, m.get(2).map(|p| p.as_str().replace("\\\\", "\\"))));
            continue;
        }
        if kind == Some("input_image") {
            if let Some((n, path)) = pending.take() {
                let url = b.get("image_url").and_then(Value::as_str).unwrap_or("");
                let mime = DATA_MIME.captures(url).map(|c| c[1].to_string());
                entries.push(Entry { n, text: text.clone(), start, len, block, path, ext: ext_of(mime.as_deref()) });
            }
        }
    }
}

fn scan(file: &Path, ix: &mut Index) -> std::io::Result<()> {
    let size = std::fs::metadata(file)?.len();
    if size < ix.scanned {
        // Rewritten.
        *ix = Index::default();
    }
    if size == ix.scanned {
        return Ok(());
    }
    let mut f = std::fs::File::open(file)?;
    f.seek(SeekFrom::Start(ix.scanned))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, f);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        // A line still being written is read again next time.
        if read == 0 || line.last() != Some(&b'\n') {
            return Ok(());
        }
        line.pop();
        images_in_line(&line, ix.scanned, &mut ix.entries);
        ix.scanned += read as u64;
    }
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() > 1).map(str::to_lowercase)
}

/// How many words of the terminal line the message's text shares.
fn overlap(text: &str, context: &str) -> usize {
    let known: HashSet<String> = words(text).collect();
    words(context).filter(|w| known.contains(w)).count()
}

fn best<'a>(all: &[&'a Entry], context: &str) -> Option<&'a Entry> {
    // The best match for the line's text; among equals, the latest.
    let mut found = *all.last()?;
    let mut score = overlap(&found.text, context);
    for e in all.iter().rev().skip(1) {
        let s = overlap(&e.text, context);
        if s > score {
            found = e;
            score = s;
        }
    }
    Some(found)
}

/// Writes the image out of the transcript, reading only its line; once per image.
fn write_out(file: &Path, e: &Entry, cache: &Path) -> Option<PathBuf> {
    let stem = file.file_stem()?.to_string_lossy();
    let out = cache.join(format!("{stem}-{}-{}.{}", e.start, e.block, e.ext));
    if out.exists() {
        return Some(out);
    }
    let mut f = std::fs::File::open(file).ok()?;
    f.seek(SeekFrom::Start(e.start)).ok()?;
    let mut line = vec![0; e.len];
    f.read_exact(&mut line).ok()?;
    let o: Value = serde_json::from_slice(&line).ok()?;
    let content = o.pointer("/message/content").or_else(|| o.pointer("/payload/content"))?;
    let b = content.get(e.block)?;
    let data = match b.pointer("/source/data").and_then(Value::as_str) {
        Some(d) => d.to_string(),
        None => DATA_BODY.captures(b.get("image_url")?.as_str()?)?[1].to_string(),
    };
    let bytes = STANDARD.decode(data.as_bytes()).ok()?;
    std::fs::create_dir_all(cache).ok()?;
    std::fs::write(&out, bytes).ok()?;
    Some(out)
}

fn find_in(file: &Path, n: u32, context: &str, cache: &Path) -> Option<PathBuf> {
    let mut indexes = INDEXES.lock().unwrap();
    let ix = indexes.entry(file.to_path_buf()).or_default();
    scan(file, ix).ok()?;
    let all: Vec<&Entry> = ix.entries.iter().filter(|e| e.n == n).collect();
    let e = best(&all, context)?.clone();
    drop(indexes);
    if let Some(p) = e.path.as_deref().map(PathBuf::from).filter(|p| p.exists()) {
        return Some(p);
    }
    write_out(file, &e, cache)
}

/// The file showing a session's image number n; `context` is the terminal line it's on. Reads the
/// transcript, which can take a while the first time.
pub fn find(paths: &Paths, session: &str, n: u32, context: &str) -> Option<PathBuf> {
    let homes = providers::homes(paths);
    let (file, _) = composer::session_transcript(paths, &homes, session)?;
    find_in(&file, n, context, &paths.data_dir.join("image-cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crew-images-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_a_claude_image_out_of_the_transcript() {
        let dir = scratch();
        let file = dir.join("s.jsonl");
        let line = r#"{"imagePasteIds":[2],"message":{"content":[{"type":"text","text":"look at this"},{"type":"image","source":{"media_type":"image/jpeg","data":"aGVsbG8="}}]}}"#;
        std::fs::write(&file, format!("{{\"type\":\"other\"}}\n{line}\n")).unwrap();
        let cache = dir.join("cache");
        assert_eq!(find_in(&file, 1, "", &cache), None);
        let out = find_in(&file, 2, "", &cache).unwrap();
        assert_eq!(out.extension().unwrap(), "jpg");
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_codex_image_is_matched_by_its_message_and_found_as_it_grows() {
        let dir = scratch();
        let file = dir.join("s.jsonl");
        let original = dir.join("kept.png");
        std::fs::write(&original, b"x").unwrap();
        let path = original.to_string_lossy().replace('\\', "\\\\\\\\");
        let message = |text: &str, tag: &str| format!(r#"{{"type":"response_item","payload":{{"role":"user","content":[{tag},{{"type":"input_image","image_url":"data:image/png;base64,eA=="}},{{"type":"input_text","text":"{text}"}}]}}}}"#);
        let first = message("the red button", &format!(r#"{{"type":"input_text","text":"<image name=[Image #1] path=\"{path}\">"}}"#));
        std::fs::write(&file, format!("{first}\n")).unwrap();
        let cache = dir.join("cache");
        assert_eq!(find_in(&file, 1, "", &cache).unwrap(), original);
        let second = message("the blue chart", r#"{"type":"input_text","text":"<image name=[Image #1]>"}"#);
        std::fs::write(&file, format!("{first}\n{second}\n")).unwrap();
        // The latest, unless the line says otherwise.
        assert_ne!(find_in(&file, 1, "something else", &cache).unwrap(), original);
        assert_eq!(find_in(&file, 1, "about the red button", &cache).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlap_counts_shared_words() {
        assert_eq!(overlap("The red button", "a RED button here"), 2);
        assert_eq!(overlap("好的 按钮", "按钮"), 1);
    }
}
