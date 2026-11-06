//! The file browser's side: a folder's tree, read one level at a time, the files Git ignores marked,
//! finding files by name or by content, and a file's content. All of it blocks.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::env::clean_env;

const SKIP: [&str; 2] = [".git", "node_modules"]; // never walked when searching
const MAX_TEXT: u64 = 5 * 1024 * 1024;
const MAX_IMAGE: u64 = 15 * 1024 * 1024;
const MAX_RESULTS: usize = 500;
const IMAGES: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub rel: String,
    pub dir: bool,
    pub ignored: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image(PathBuf),
    /// Why it isn't shown.
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rel: String,
    pub line: Option<usize>,
    pub text: Option<String>,
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Of these paths (relative to root), the ones Git ignores. Nothing outside a repository.
fn ignored_of(root: &Path, rels: &[String]) -> HashSet<String> {
    if rels.is_empty() {
        return HashSet::new();
    }
    let mut cmd = Command::new("git");
    cmd.args(["check-ignore", "--stdin"]).current_dir(root).env_clear().envs(clean_env()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = hidden(&mut cmd).spawn() else { return HashSet::new() };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(rels.join("\n").as_bytes());
    }
    let Ok(out) = child.wait_with_output() else { return HashSet::new() };
    if !out.status.success() {
        return HashSet::new();
    }
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(|l| l.replace('\\', "/")).collect()
}

/// Case-blind, with runs of digits compared as numbers: file2 before file10.
fn natural_key(name: &str) -> Vec<(u8, u64, String)> {
    let mut out = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(d) = c.to_digit(10) {
            let mut n = u64::from(d);
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                n = n.saturating_mul(10).saturating_add(u64::from(d));
                chars.next();
            }
            out.push((0, n, String::new()));
        } else {
            out.push((1, 0, c.to_lowercase().collect()));
        }
    }
    out
}

/// One folder's entries: folders first, then files, by name.
pub fn list(root: &Path, rel: &str) -> Result<Vec<Item>, String> {
    let entries = std::fs::read_dir(root.join(rel)).map_err(|e| e.to_string())?;
    let mut items: Vec<Item> = entries
        .flatten()
        .filter(|d| d.file_name() != ".git")
        .map(|d| {
            let name = d.file_name().to_string_lossy().to_string();
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let dir = d.file_type().is_ok_and(|t| t.is_dir());
            Item { name, rel: path, dir, ignored: false }
        })
        .collect();
    items.sort_by_key(|i| (!i.dir, natural_key(&i.name)));
    let asked: Vec<String> = items.iter().map(|i| if i.dir { format!("{}/", i.rel) } else { i.rel.clone() }).collect();
    let ignored = ignored_of(root, &asked);
    for (item, key) in items.iter_mut().zip(&asked) {
        item.ignored = ignored.contains(key) || ignored.contains(&item.rel);
    }
    Ok(items)
}

pub fn read(root: &Path, rel: &str) -> Content {
    let file = root.join(rel);
    let meta = match std::fs::metadata(&file) {
        Ok(m) => m,
        Err(e) => return Content::Unavailable(e.to_string()),
    };
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if IMAGES.contains(&ext.as_str()) {
        return if meta.len() > MAX_IMAGE { Content::Unavailable("The image is too large to show.".into()) } else { Content::Image(file) };
    }
    if meta.len() > MAX_TEXT {
        return Content::Unavailable(format!("The file is {:.1} MB, too large to show.", meta.len() as f64 / 1_048_576.));
    }
    match std::fs::read(&file) {
        Ok(bytes) if bytes[..bytes.len().min(8000)].contains(&0) => Content::Unavailable("This is a binary file.".into()),
        Ok(bytes) => Content::Text(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) => Content::Unavailable(e.to_string()),
    }
}

/// Saves an edited file (the caller keeps its line endings). Never outside the folder.
pub fn write(root: &Path, rel: &str, text: &str) -> Result<(), String> {
    if Path::new(rel).components().any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
        return Err("The file is outside the folder.".into());
    }
    std::fs::write(root.join(rel), text).map_err(|e| e.to_string())
}

fn walk(root: &Path, rel: &str, on_file: &mut dyn FnMut(&str, &Path) -> bool) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else { return true };
    for d in entries.flatten() {
        let name = d.file_name().to_string_lossy().to_string();
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if d.file_type().is_ok_and(|t| t.is_dir()) {
            if !SKIP.contains(&name.as_str()) && !walk(root, &path, on_file) {
                return false;
            }
        } else if !on_file(&path, &d.path()) {
            return false;
        }
    }
    true
}

fn has_ripgrep() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| hidden(Command::new("rg").arg("--version").stdout(Stdio::null()).stderr(Stdio::null())).status().is_ok_and(|s| s.success()))
}

/// Parses `path:line:text`; the path itself may hold a colon (`C:\...` doesn't appear: it's relative).
fn parse_ripgrep(out: &str) -> Vec<Hit> {
    out.lines()
        .filter_map(|l| {
            let (rel, rest) = l.split_once(':')?;
            let (line, text) = rest.split_once(':')?;
            let line = line.parse().ok()?;
            let rel = rel.strip_prefix(".\\").or_else(|| rel.strip_prefix("./")).unwrap_or(rel).replace('\\', "/");
            Some(Hit { rel, line: Some(line), text: Some(text.trim().to_string()) })
        })
        .take(MAX_RESULTS)
        .collect()
}

/// Lines containing the query.
pub fn search_content(root: &Path, query: &str) -> Vec<Hit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    if has_ripgrep() {
        let mut cmd = Command::new("rg");
        cmd.args(["-n", "--no-heading", "--color", "never", "-F", "-S", "--max-count", "20", "--max-columns", "240", "--", q, "."]).current_dir(root).stdin(Stdio::null());
        return hidden(&mut cmd).output().map(|o| parse_ripgrep(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default();
    }
    let needle = q.to_lowercase();
    let mut hits = Vec::new();
    walk(root, "", &mut |rel, full| {
        let small = std::fs::metadata(full).is_ok_and(|m| m.len() <= 2 * 1024 * 1024);
        if let Some(bytes) = small.then(|| std::fs::read(full).ok()).flatten().filter(|b| !b[..b.len().min(8000)].contains(&0)) {
            for (i, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
                if hits.len() < MAX_RESULTS && line.to_lowercase().contains(&needle) {
                    hits.push(Hit { rel: rel.to_string(), line: Some(i + 1), text: Some(line.trim().chars().take(240).collect()) });
                }
            }
        }
        hits.len() < MAX_RESULTS
    });
    hits
}

/// Files whose path contains the query: the whole tree, as Git sees it when it's a repository.
pub fn search_names(root: &Path, query: &str) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut cmd = Command::new("git");
    cmd.args(["ls-files", "-co", "--exclude-standard"]).current_dir(root).env_clear().envs(clean_env()).stdin(Stdio::null()).stderr(Stdio::null());
    let listed = hidden(&mut cmd).output().ok().filter(|o| o.status.success());
    let rels: Vec<String> = match listed {
        Some(o) => String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.is_empty()).map(str::to_string).collect(),
        None => {
            let mut all = Vec::new();
            walk(root, "", &mut |rel, _| {
                all.push(rel.to_string());
                all.len() < 50_000
            });
            all
        }
    };
    rels.into_iter().filter(|p| p.to_lowercase().contains(&needle)).take(MAX_RESULTS).map(|rel| Hit { rel, line: None, text: None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!("crew-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("file10.txt"), "needle here\n").unwrap();
        std::fs::write(d.join("file2.txt"), "nothing\n").unwrap();
        std::fs::write(d.join("sub").join("deep.md"), "a\nNeedle again\n").unwrap();
        d
    }

    #[test]
    fn folders_first_then_numbers_in_order() {
        let names: Vec<String> = list(&scratch(), "").unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["sub", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn text_images_and_binaries() {
        let d = scratch();
        std::fs::write(d.join("x.bin"), [1u8, 0, 2]).unwrap();
        std::fs::write(d.join("pic.PNG"), [1u8, 0]).unwrap();
        assert_eq!(read(&d, "file2.txt"), Content::Text("nothing\n".into()));
        assert!(matches!(read(&d, "x.bin"), Content::Unavailable(_)));
        assert!(matches!(read(&d, "pic.PNG"), Content::Image(_)));
        assert!(matches!(read(&d, "missing"), Content::Unavailable(_)));
    }

    #[test]
    fn writes_stay_inside_the_folder() {
        let d = scratch();
        assert!(write(&d, "sub/deep.md", "new").is_ok());
        assert_eq!(std::fs::read_to_string(d.join("sub").join("deep.md")).unwrap(), "new");
        assert!(write(&d, "../outside.txt", "x").is_err());
        assert!(!d.parent().unwrap().join("outside.txt").exists());
        assert!(write(&d, r"C:\outside.txt", "x").is_err());
    }

    #[test]
    fn finds_by_name_and_content() {
        let d = scratch();
        assert_eq!(search_names(&d, "DEEP").iter().map(|h| h.rel.as_str()).collect::<Vec<_>>(), ["sub/deep.md"]);
        let mut hits = search_content(&d, "needle");
        hits.sort_by(|a, b| a.rel.cmp(&b.rel));
        assert_eq!(hits.iter().map(|h| (h.rel.as_str(), h.line)).collect::<Vec<_>>(), [("file10.txt", Some(1)), ("sub/deep.md", Some(2))]);
    }

    #[test]
    fn reads_ripgrep_lines() {
        let hits = parse_ripgrep(".\\src\\a.rs:12:  let x = 1:2;\nnot a hit\n");
        assert_eq!(hits, [Hit { rel: "src/a.rs".into(), line: Some(12), text: Some("let x = 1:2;".into()) }]);
    }
}
