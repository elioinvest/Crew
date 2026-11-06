//! The real git CLI, and reading what it says. Output is plain text (no color, no pager, paths
//! unquoted) and git never waits on a terminal prompt: credentials go through the configured credential
//! helper (Git Credential Manager on Windows) or the command fails.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::env::clean_env;

const FLAGS: [&str; 6] = ["-c", "core.quotepath=false", "-c", "color.ui=false", "-c", "i18n.logOutputEncoding=UTF-8"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// What it said, for showing when it failed.
    pub fn message(&self) -> String {
        let err = self.stderr.trim();
        if err.is_empty() { self.stdout.trim().to_string() } else { err.to_string() }
    }
}

/// Runs git and waits. Blocks.
pub fn run_with(cwd: &Path, args: &[&str], input: Option<&str>, extra_env: &[(&str, &str)]) -> Output {
    if !cwd.exists() {
        return Output { code: -1, stdout: String::new(), stderr: "Repository folder not found.".into() };
    }
    let mut env = clean_env();
    env.remove("FORCE_COLOR");
    // GIT_EDITOR=true accepts default messages (merges, reverts) instead of waiting on an editor.
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    env.insert("GIT_PAGER".into(), String::new());
    env.insert("GIT_EDITOR".into(), "true".into());
    env.insert("LC_ALL".into(), "C.UTF-8".into());
    for (k, v) in extra_env {
        env.insert((*k).into(), (*v).into());
    }
    let mut cmd = Command::new("git");
    cmd.args(FLAGS).args(args).current_dir(cwd).env_clear().envs(env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Output { code: -1, stdout: String::new(), stderr: "Git is not installed or not on PATH.".into() },
        Err(e) => return Output { code: -1, stdout: String::new(), stderr: e.to_string() },
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(text) = input {
            let bytes = text.as_bytes().to_vec();
            // On its own thread: a big input and a big output would otherwise wait on each other.
            std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
    }
    match child.wait_with_output() {
        Ok(o) => Output { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() },
        Err(e) => Output { code: -1, stdout: String::new(), stderr: e.to_string() },
    }
}

pub fn run(cwd: &Path, args: &[&str]) -> Output {
    run_with(cwd, args, None, &[])
}

/// The repository a folder is in.
pub fn root_of(folder: &Path) -> Option<PathBuf> {
    let out = run(folder, &["rev-parse", "--show-toplevel"]);
    out.ok().then(|| PathBuf::from(out.stdout.trim().replace('/', "\\")))
}

/// Appends a pattern to the repository's .gitignore, starting it on a new line.
pub fn ignore(root: &Path, pattern: &str) -> std::io::Result<()> {
    let file = root.join(".gitignore");
    let current = std::fs::read_to_string(&file).unwrap_or_default();
    if current.lines().any(|l| l == pattern) {
        return Ok(());
    }
    let eol = if current.contains("\r\n") { "\r\n" } else { "\n" };
    let lead = if !current.is_empty() && !current.ends_with('\n') { eol } else { "" };
    std::fs::write(&file, format!("{current}{lead}{pattern}{eol}"))
}

// ---------- status ----------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub orig: Option<String>,
    pub status: char,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    /// The HEAD commit, or None before the first commit.
    pub head: Option<String>,
    /// Empty when HEAD is detached.
    pub branch: String,
    /// What says whether the branch or commit moved.
    pub signature: String,
}

/// git status in its detailed form, which also says the branch, commit and upstream, so one process
/// answers most of what a refresh needs.
pub fn parse_status(out: &str) -> Status {
    let parts: Vec<&str> = out.split('\0').collect();
    let mut s = Status::default();
    let mut fields: HashMap<&str, &str> = HashMap::new();
    let after = |text: &str, n: usize| -> String { text.splitn(n + 1, ' ').nth(n).unwrap_or("").to_string() };
    let mut i = 0;
    while i < parts.len() {
        let p = parts[i];
        if let Some(rest) = p.strip_prefix("# branch.") {
            if let Some((k, v)) = rest.split_once(' ') {
                fields.insert(k, v);
            }
        } else if let Some(path) = p.strip_prefix("? ") {
            s.unstaged.push(Change { path: path.to_string(), orig: None, status: '?' });
        } else if p.starts_with("u ") {
            s.unstaged.push(Change { path: after(p, 10), orig: None, status: 'U' });
        } else if p.starts_with("1 ") || p.starts_with("2 ") {
            let renamed = p.starts_with("2 ");
            let mut xy = p[2..].chars();
            let (x, y) = (xy.next().unwrap_or('.'), xy.next().unwrap_or('.'));
            let path = after(p, if renamed { 9 } else { 8 });
            let orig = if renamed {
                i += 1;
                parts.get(i).map(|o| (*o).to_string())
            } else {
                None
            };
            if x != '.' {
                s.staged.push(Change { path: path.clone(), orig, status: x });
            }
            if y != '.' {
                s.unstaged.push(Change { path, orig: None, status: y });
            }
        }
        i += 1;
    }
    s.staged.sort_by(|a, b| a.path.cmp(&b.path));
    s.unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    let oid = fields.get("oid").copied().unwrap_or("");
    let head = fields.get("head").copied().unwrap_or("");
    s.head = (!oid.is_empty() && oid != "(initial)").then(|| oid.to_string());
    s.branch = if head == "(detached)" { String::new() } else { head.to_string() };
    s.signature = format!("{oid}|{head}|{}|{}", fields.get("upstream").copied().unwrap_or(""), fields.get("ab").copied().unwrap_or(""));
    s
}

/// One row per changed file, with what's staged and what isn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub status: char,
    pub orig: Option<String>,
    pub staged: Option<Change>,
    pub unstaged: Option<Change>,
}

pub fn entries(status: &Status) -> Vec<Entry> {
    let mut out: Vec<Entry> = status.unstaged.iter().map(|f| Entry { path: f.path.clone(), status: f.status, orig: f.orig.clone(), staged: None, unstaged: Some(f.clone()) }).collect();
    for f in &status.staged {
        if let Some(e) = out.iter_mut().find(|e| e.path == f.path) {
            e.staged = Some(f.clone());
            if e.orig.is_none() {
                e.orig = f.orig.clone();
            }
        } else {
            out.push(Entry { path: f.path.clone(), status: f.status, orig: f.orig.clone(), staged: Some(f.clone()), unstaged: None });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Lines added / removed per file. Binary files and untracked ones have none.
pub fn parse_numstat(out: &str) -> HashMap<String, (u64, u64)> {
    let mut map = HashMap::new();
    for line in out.split('\0').filter(|l| !l.is_empty()) {
        let mut f = line.splitn(3, '\t');
        if let (Some(a), Some(d), Some(p)) = (f.next(), f.next(), f.next()) {
            if a != "-" && !p.is_empty() {
                map.insert(p.to_string(), (a.parse().unwrap_or(0), d.parse().unwrap_or(0)));
            }
        }
    }
    map
}

// ---------- refs ----------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalBranch {
    pub name: String,
    pub hash: String,
    pub upstream: String,
    pub ahead: u64,
    pub behind: u64,
    pub gone: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteBranch {
    pub name: String,
    pub remote: String,
    pub hash: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub hash: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stash {
    pub reference: String,
    pub message: String,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastCommit {
    pub subject: String,
    pub at: i64,
    pub has_parent: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub locals: Vec<LocalBranch>,
    pub remote_branches: Vec<RemoteBranch>,
    pub remotes: Vec<String>,
    pub tags: Vec<Tag>,
    pub stashes: Vec<Stash>,
    pub last: Option<LastCommit>,
    pub operation: Option<&'static str>,
}

fn number_after(text: &str, word: &str) -> u64 {
    text.split(word).nth(1).map(|r| r.trim_start().chars().take_while(char::is_ascii_digit).collect::<String>()).and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// Numbers inside a name compare as numbers: v1.10 is after v1.9.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| -> Vec<(u8, u64, String)> {
        let mut out = Vec::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if let Some(d) = c.to_digit(10) {
                let mut n = u64::from(d);
                while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                    n = n.saturating_mul(10).saturating_add(u64::from(d));
                    chars.next();
                }
                out.push((0, n, String::new()));
            } else {
                out.push((1, 0, c.to_string()));
            }
        }
        out
    };
    key(a).cmp(&key(b))
}

pub fn parse_refs(for_each_ref: &str, stash: &str, remotes: &str, last: &Output) -> Refs {
    let mut r = Refs::default();
    for line in for_each_ref.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\0').collect();
        let get = |i: usize| f.get(i).copied().unwrap_or("");
        let (full, name, obj, deref, upstream, track) = (get(0), get(1), get(2), get(3), get(4), get(5));
        if full.starts_with("refs/heads/") {
            r.locals.push(LocalBranch { name: name.into(), hash: obj.into(), upstream: upstream.into(), ahead: number_after(track, "ahead"), behind: number_after(track, "behind"), gone: track.contains("gone") });
        } else if full.starts_with("refs/remotes/") {
            if !full.ends_with("/HEAD") {
                r.remote_branches.push(RemoteBranch { name: name.into(), remote: name.split('/').next().unwrap_or("").into(), hash: obj.into() });
            }
        } else if full.starts_with("refs/tags/") {
            r.tags.push(Tag { name: name.into(), hash: if deref.is_empty() { obj.into() } else { deref.into() } });
        }
    }
    r.tags.sort_by(|a, b| natural_cmp(&b.name, &a.name));
    r.stashes = stash
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\0').collect();
            Stash { reference: f.first().copied().unwrap_or("").into(), message: f.get(1).copied().unwrap_or("").into(), at: f.get(2).and_then(|a| a.parse().ok()).unwrap_or(0) }
        })
        .collect();
    r.remotes = remotes.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
    let text = last.stdout.trim();
    if last.ok() && !text.is_empty() {
        let f: Vec<&str> = text.split('\0').collect();
        r.last = Some(LastCommit { subject: f.first().copied().unwrap_or("").into(), at: f.get(1).and_then(|a| a.parse().ok()).unwrap_or(0), has_parent: f.get(2).is_some_and(|p| !p.is_empty()) });
    }
    r
}

/// Everything about branches, tags, stashes and any operation in progress. Blocks.
pub fn load_refs(root: &Path, git_dir: Option<&Path>) -> Refs {
    let refs = run(root, &["for-each-ref", "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track)", "refs/heads", "refs/remotes", "refs/tags"]);
    let stash = run(root, &["stash", "list", "--format=%gd%x00%s%x00%at"]);
    let remotes = run(root, &["remote"]);
    let last = run(root, &["log", "-1", "--format=%s%x00%at%x00%P"]);
    let mut out = parse_refs(&refs.stdout, &stash.stdout, &remotes.stdout, &last);
    if let Some(dir) = git_dir {
        let ops = [("MERGE_HEAD", "merge"), ("rebase-merge", "rebase"), ("rebase-apply", "rebase"), ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert")];
        out.operation = ops.iter().find(|(file, _)| dir.join(file).exists()).map(|(_, op)| *op);
    }
    out
}

// ---------- history ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Detached,
    Tag,
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefName {
    pub kind: RefKind,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub col: usize,
    pub before: Vec<Option<String>>,
    pub parent_cols: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<RefName>,
    pub author: String,
    pub email: String,
    pub at: i64,
    pub subject: String,
    pub graph: GraphRow,
}

pub const LOG_FORMAT: &str = "--format=%x1e%H%x00%P%x00%D%x00%an%x00%ae%x00%at%x00%s";

pub fn parse_log(out: &str, locals: &HashSet<String>) -> Vec<Commit> {
    out.split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .map(|record| {
            let f: Vec<&str> = record.trim_end_matches('\n').split('\0').collect();
            let get = |i: usize| f.get(i).copied().unwrap_or("");
            let refs = get(2)
                .split(", ")
                .filter(|s| !s.is_empty())
                .map(|s| {
                    if let Some(n) = s.strip_prefix("HEAD -> ") {
                        RefName { kind: RefKind::Head, name: n.into() }
                    } else if s == "HEAD" {
                        RefName { kind: RefKind::Detached, name: "HEAD".into() }
                    } else if let Some(n) = s.strip_prefix("tag: ") {
                        RefName { kind: RefKind::Tag, name: n.into() }
                    } else {
                        RefName { kind: if locals.contains(s) { RefKind::Local } else { RefKind::Remote }, name: s.into() }
                    }
                })
                .collect();
            Commit {
                hash: get(0).into(),
                parents: get(1).split(' ').filter(|p| !p.is_empty()).map(str::to_string).collect(),
                refs,
                author: get(3).into(),
                email: get(4).into(),
                at: get(5).parse().unwrap_or(0),
                subject: get(6).into(),
                graph: GraphRow::default(),
            }
        })
        .collect()
}

/// Lays the commits out in lanes: each lane holds the commit it's waiting for. A commit takes the
/// first lane waiting for it (others waiting for it join in), its first parent continues in that
/// lane, and further parents (merges) get their own lane. Returns how many lanes it took.
pub fn build_graph(commits: &mut [Commit]) -> usize {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut most = 1;
    for c in commits.iter_mut() {
        let before = lanes.clone();
        let col = before
            .iter()
            .position(|h| h.as_deref() == Some(c.hash.as_str()))
            .or_else(|| before.iter().position(Option::is_none))
            .unwrap_or(before.len());
        let mut after: Vec<Option<String>> = before.iter().map(|h| if h.as_deref() == Some(c.hash.as_str()) { None } else { h.clone() }).collect();
        if col >= after.len() {
            after.resize(col + 1, None);
        }
        after[col] = c.parents.first().cloned();
        let mut parent_cols = if c.parents.is_empty() { Vec::new() } else { vec![col] };
        for p in c.parents.iter().skip(1) {
            let j = after.iter().position(|h| h.as_deref() == Some(p.as_str())).unwrap_or_else(|| {
                let free = after.iter().position(Option::is_none).unwrap_or(after.len());
                if free >= after.len() {
                    after.resize(free + 1, None);
                }
                after[free] = Some(p.clone());
                free
            });
            parent_cols.push(j);
        }
        while after.last().is_some_and(Option::is_none) {
            after.pop();
        }
        most = most.max(before.len()).max(after.len()).max(col + 1);
        c.graph = GraphRow { col, before, parent_cols };
        lanes = after;
    }
    most
}

// ---------- diffs ----------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunk {
    pub head: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
}

pub fn parse_diff(text: &str) -> FileDiff {
    let mut d = FileDiff::default();
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for l in lines {
        if l.starts_with("@@") {
            d.hunks.push(Hunk { head: l.to_string(), lines: Vec::new() });
        } else if let Some(h) = d.hunks.last_mut() {
            h.lines.push(l.to_string());
        } else {
            d.header.push(l.to_string());
        }
    }
    d
}

fn path_of_diff(header: &[String]) -> Option<String> {
    let clean = |s: &str| s.trim_matches('"').split('\t').next().unwrap_or("").to_string();
    for l in header {
        if let Some(p) = l.strip_prefix("rename to ").or_else(|| l.strip_prefix("copy to ")) {
            return Some(clean(p));
        }
    }
    for l in header {
        if let Some(p) = l.strip_prefix("+++ ").filter(|p| *p != "/dev/null") {
            let c = clean(p);
            return Some(c.strip_prefix("b/").unwrap_or(&c).to_string());
        }
    }
    for l in header {
        if let Some(p) = l.strip_prefix("--- ").filter(|p| *p != "/dev/null") {
            let c = clean(p);
            return Some(c.strip_prefix("a/").unwrap_or(&c).to_string());
        }
    }
    let first = header.first()?.strip_prefix("diff --git ")?;
    first.rsplit_once(" b/").map(|(_, p)| p.trim_matches('"').to_string())
}

/// A diff of several files, split per file and parsed.
pub fn diffs_by_file(text: &str) -> HashMap<String, FileDiff> {
    let mut out = HashMap::new();
    let mut block = String::new();
    let mut flush = |block: &str| {
        if block.starts_with("diff --git ") {
            let parsed = parse_diff(block);
            if let Some(p) = path_of_diff(&parsed.header) {
                out.insert(p, parsed);
            }
        }
    };
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            flush(&block);
            block.clear();
        }
        block.push_str(line);
    }
    flush(&block);
    out
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Words {
    pub pre: String,
    pub mid: String,
    pub post: String,
}

fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    let class = |c: char| if c.is_whitespace() { 0 } else if c.is_ascii_alphanumeric() || c == '_' { 1 } else { 2 };
    let mut prev = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        match start {
            Some(st) if k == 2 || prev != Some(k) => {
                out.push(&s[st..i]);
                start = Some(i);
            }
            None => start = Some(i),
            _ => {}
        }
        prev = Some(k);
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

/// What changed inside a replaced line: the words between the common start and end.
pub fn word_spans(old: &str, new: &str) -> Option<(Words, Words)> {
    let (a, b) = (tokens(old), tokens(new));
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end = 0;
    while end < a.len() - start && end < b.len() - start && a[a.len() - 1 - end] == b[b.len() - 1 - end] {
        end += 1;
    }
    // Replaced outright: highlighting the whole line says nothing.
    if start == 0 && end == 0 {
        return None;
    }
    let part = |t: &[&str]| Words { pre: t[..start].concat(), mid: t[start..t.len() - end].concat(), post: t[t.len() - end..].concat() };
    Some((part(&a), part(&b)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Add,
    Del,
    Context,
    Meta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u64>,
    pub new: Option<u64>,
    pub text: String,
    pub words: Option<Words>,
}

/// A hunk's lines with their numbers, and the words changed in replaced lines.
pub fn hunk_rows(hunk: &Hunk) -> Vec<DiffLine> {
    // "@@ -a,b +c,d @@", or "@@@ -a,b -a2,b2 +c,d @@@" for a conflicted file
    let combined = hunk.head.starts_with("@@@");
    let number_after_sign = |sign: char| -> u64 { hunk.head.split(' ').find(|p| p.starts_with(sign)).map(|p| p[1..].split(',').next().unwrap_or("").to_string()).and_then(|n| n.parse().ok()).unwrap_or(0) };
    let (mut old_n, mut new_n) = (number_after_sign('-'), number_after_sign('+'));
    let mut rows = Vec::with_capacity(hunk.lines.len());
    for l in &hunk.lines {
        if l.starts_with('\\') {
            rows.push(DiffLine { kind: LineKind::Meta, old: None, new: None, text: l.clone(), words: None });
            continue;
        }
        let prefix: String = l.chars().take(if combined { 2 } else { 1 }).collect();
        let kind = if prefix.contains('+') { '+' } else if prefix.contains('-') { '-' } else { ' ' };
        let text = l[prefix.len()..].trim_end_matches('\r').to_string();
        rows.push(match kind {
            '+' => {
                new_n += 1;
                DiffLine { kind: LineKind::Add, old: None, new: Some(new_n - 1), text, words: None }
            }
            '-' => {
                old_n += 1;
                DiffLine { kind: LineKind::Del, old: Some(old_n - 1), new: None, text, words: None }
            }
            _ => {
                old_n += 1;
                new_n += 1;
                DiffLine { kind: LineKind::Context, old: Some(old_n - 1), new: Some(new_n - 1), text, words: None }
            }
        });
    }
    // Same-sized runs of removed then added lines are the same lines, edited.
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind != LineKind::Del {
            i += 1;
            continue;
        }
        let mut d = i;
        while rows.get(d + 1).is_some_and(|r| r.kind == LineKind::Del) {
            d += 1;
        }
        let mut e = d;
        while rows.get(e + 1).is_some_and(|r| r.kind == LineKind::Add) {
            e += 1;
        }
        if e - d == d - i + 1 {
            for k in 0..=(d - i) {
                if let Some((a, b)) = word_spans(&rows[i + k].text, &rows[d + 1 + k].text) {
                    rows[i + k].words = Some(a);
                    rows[d + 1 + k].words = Some(b);
                }
            }
        }
        i = e + 1;
    }
    rows
}

/// The patch that applies one hunk.
pub fn hunk_patch(header: &[String], hunk: &Hunk) -> String {
    let mut out = header.join("\n");
    out.push('\n');
    out.push_str(&hunk.head);
    out.push('\n');
    for l in &hunk.lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

// ---------- merge conflicts ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictPart {
    Plain(String),
    Conflict { ours_label: String, theirs_label: String, ours: String, theirs: String, raw: String },
}

/// A file with conflict markers, as plain parts and conflicts.
pub fn parse_conflicts(text: &str) -> Vec<ConflictPart> {
    #[derive(PartialEq, Clone, Copy)]
    enum Side {
        Ours,
        Base,
        Theirs,
    }
    let mut parts = Vec::new();
    let mut plain = String::new();
    let mut open: Option<(String, String, String, String, Side)> = None; // ours label, ours, theirs, raw, at
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n').trim_end_matches('\r');
        match &mut open {
            None if bare.starts_with("<<<<<<<") => {
                if !plain.is_empty() {
                    parts.push(ConflictPart::Plain(std::mem::take(&mut plain)));
                }
                open = Some((bare[7..].trim().to_string(), String::new(), String::new(), line.to_string(), Side::Ours));
            }
            None => plain.push_str(line),
            Some((_, _, _, raw, at)) if *at == Side::Ours && bare.starts_with("|||||||") => {
                *at = Side::Base;
                raw.push_str(line);
            }
            Some((_, _, _, raw, at)) if (*at == Side::Ours || *at == Side::Base) && bare.starts_with("=======") => {
                *at = Side::Theirs;
                raw.push_str(line);
            }
            Some((_, _, _, _, at)) if *at == Side::Theirs && bare.starts_with(">>>>>>>") => {
                if let Some((ours_label, ours, theirs, mut raw, _)) = open.take() {
                    raw.push_str(line);
                    parts.push(ConflictPart::Conflict { ours_label, theirs_label: bare[7..].trim().to_string(), ours, theirs, raw });
                }
            }
            Some((_, ours, theirs, raw, at)) => {
                raw.push_str(line);
                match at {
                    Side::Ours => ours.push_str(line),
                    Side::Theirs => theirs.push_str(line),
                    Side::Base => {}
                }
            }
        }
    }
    // An unfinished marker is just text.
    if let Some((_, _, _, raw, _)) = open {
        parts.push(ConflictPart::Plain(raw));
    }
    if !plain.is_empty() {
        parts.push(ConflictPart::Plain(plain));
    }
    parts
}

/// The file's text with one conflict settled: "ours", "theirs" or "both".
pub fn resolve_conflict(text: &str, index: usize, choice: &str) -> Option<String> {
    let parts = parse_conflicts(text);
    if !matches!(parts.get(index), Some(ConflictPart::Conflict { .. })) {
        return None;
    }
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        match p {
            ConflictPart::Plain(t) => out.push_str(t),
            ConflictPart::Conflict { ours, theirs, raw, .. } => {
                if i == index {
                    match choice {
                        "ours" => out.push_str(ours),
                        "theirs" => out.push_str(theirs),
                        _ => {
                            out.push_str(ours);
                            out.push_str(theirs);
                        }
                    }
                } else {
                    out.push_str(raw);
                }
            }
        }
    }
    Some(out)
}

// ---------- commit details ----------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitDetail {
    pub rev: String,
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub committer: String,
    pub at: i64,
    pub body: String,
    pub files: Vec<Change>,
    pub diffs: HashMap<String, FileDiff>,
}

pub fn parse_name_status(out: &str) -> Vec<Change> {
    let tok: Vec<&str> = out.split('\0').filter(|s| !s.is_empty()).collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < tok.len() {
        let st = tok[i].chars().next().unwrap_or('M');
        if st == 'R' || st == 'C' {
            files.push(Change { status: st, orig: tok.get(i + 1).map(|s| (*s).to_string()), path: tok.get(i + 2).copied().unwrap_or("").to_string() });
            i += 3;
        } else {
            files.push(Change { status: st, orig: None, path: tok.get(i + 1).copied().unwrap_or("").to_string() });
            i += 2;
        }
    }
    files
}

/// A commit or stash, all its changes. Blocks.
pub fn load_commit(root: &Path, rev: &str, only: Option<&str>) -> Option<CommitDetail> {
    let meta = run(root, &["show", "-s", "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B", rev]);
    if !meta.ok() {
        return None;
    }
    let f: Vec<&str> = meta.stdout.split('\0').collect();
    let get = |i: usize| f.get(i).copied().unwrap_or("");
    let hash = get(0).to_string();
    let parents: Vec<String> = get(1).split(' ').filter(|p| !p.is_empty()).map(str::to_string).collect();
    let tail: Vec<&str> = only.map_or(Vec::new(), |p| vec!["--", p]);
    let with_tail = |base: Vec<&'_ str>| -> Vec<String> { base.into_iter().chain(tail.iter().copied()).map(str::to_string).collect() };
    let (names_args, patch_args) = match parents.first() {
        Some(parent) => (with_tail(vec!["diff", "--name-status", "-z", "-M", parent, &hash]), with_tail(vec!["diff", "-M", parent, &hash])),
        None => (with_tail(vec!["diff-tree", "--root", "-r", "--name-status", "-z", "-M", "--no-commit-id", &hash]), with_tail(vec!["diff-tree", "-p", "--root", "-M", "--no-commit-id", &hash])),
    };
    let call = |args: &[String]| run(root, &args.iter().map(String::as_str).collect::<Vec<_>>());
    let (names, patch) = (call(&names_args), call(&patch_args));
    Some(CommitDetail {
        rev: rev.to_string(),
        hash,
        parents,
        author: format!("{} <{}>", get(2), get(3)),
        committer: format!("{} <{}>", get(5), get(6)),
        at: get(4).parse().unwrap_or(0),
        body: get(8).trim().to_string(),
        files: parse_name_status(&names.stdout),
        diffs: diffs_by_file(&patch.stdout),
    })
}

// ---------- little things ----------

/// "Push" -> "Pushing", "Check out main" -> "Checking out main", for the loading overlay.
pub fn progressive(label: &str) -> String {
    let (word, rest) = label.split_once(' ').map_or((label, None), |(w, r)| (w, Some(r)));
    let ing = match word {
        "Commit" => "Committing".to_string(),
        "Pop" => "Popping".to_string(),
        "Reset" => "Resetting".to_string(),
        "Stop" => "Stopping".to_string(),
        "Uncommit" => "Uncommitting".to_string(),
        "Cherry-pick" => "Cherry-picking".to_string(),
        w if w.ends_with('e') && !w.ends_with("ee") => format!("{}ing", &w[..w.len() - 1]),
        w => format!("{w}ing"),
    };
    rest.map_or(ing.clone(), |r| format!("{ing} {r}"))
}

/// A permanent link to a file at a commit, on the host a remote points to.
pub fn permalink(remote_url: &str, commit: &str, path: &str) -> Option<String> {
    let url = remote_url.trim();
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).or_else(|| url.strip_prefix("ssh://")).unwrap_or(url);
    let rest = rest.split_once('@').map_or(rest, |(user, r)| if user.contains('/') { rest } else { r });
    let at = rest.find([':', '/'])?;
    let (host, repo) = (&rest[..at], &rest[at + 1..]);
    if host.is_empty() || repo.is_empty() {
        return None;
    }
    let repo = repo.trim_end_matches('/').trim_end_matches(".git");
    let lower = host.to_lowercase();
    let middle = if lower.contains("bitbucket") { "src" } else if lower.contains("gitlab") { "-/blob" } else { "blob" };
    let encoded: Vec<String> = path.split('/').map(percent_encode).collect();
    Some(format!("https://{host}/{repo}/{middle}/{commit}/{}", encoded.join("/")))
}

fn percent_encode(part: &str) -> String {
    part.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        let d = std::env::temp_dir().join(format!("crew-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(run(&d, &["init", "-q", "-b", "main"]).ok());
        run(&d, &["config", "user.email", "t@example.com"]);
        run(&d, &["config", "user.name", "T"]);
        run(&d, &["config", "core.autocrlf", "false"]);
        d
    }

    fn commit(d: &Path, file: &str, text: &str, message: &str) {
        std::fs::write(d.join(file), text).unwrap();
        run(d, &["add", "-A"]);
        assert!(run(d, &["commit", "-q", "-m", message]).ok());
    }

    #[test]
    fn status_of_a_real_repository() {
        let d = repo();
        commit(&d, "a.txt", "one\n", "first");
        commit(&d, "gone.txt", "x\n", "second");
        commit(&d, "old.txt", "keep this text the same\nfor rename detection\n", "third");
        std::fs::write(d.join("a.txt"), "changed\n").unwrap();
        std::fs::write(d.join("new file.txt"), "n\n").unwrap();
        std::fs::remove_file(d.join("gone.txt")).unwrap();
        run(&d, &["mv", "old.txt", "renamed.txt"]);
        let s = parse_status(&run(&d, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]).stdout);
        assert_eq!(s.branch, "main");
        assert!(s.head.is_some());
        let unstaged: Vec<(&str, char)> = s.unstaged.iter().map(|c| (c.path.as_str(), c.status)).collect();
        assert_eq!(unstaged, [("a.txt", 'M'), ("gone.txt", 'D'), ("new file.txt", '?')]);
        assert_eq!(s.staged.len(), 1);
        assert_eq!((s.staged[0].path.as_str(), s.staged[0].orig.as_deref(), s.staged[0].status), ("renamed.txt", Some("old.txt"), 'R'));
        let e = entries(&s);
        assert_eq!(e.len(), 4);
    }

    #[test]
    fn before_the_first_commit() {
        let d = repo();
        let s = parse_status(&run(&d, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]).stdout);
        assert_eq!(s.head, None);
        assert_eq!(s.branch, "main");
    }

    #[test]
    fn a_merge_conflict_end_to_end() {
        let d = repo();
        commit(&d, "f.txt", "base\n", "base");
        run(&d, &["switch", "-q", "-c", "other"]);
        commit(&d, "f.txt", "theirs\n", "theirs");
        run(&d, &["switch", "-q", "main"]);
        commit(&d, "f.txt", "ours\n", "ours");
        assert!(!run(&d, &["merge", "--no-edit", "other"]).ok());
        let s = parse_status(&run(&d, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]).stdout);
        assert_eq!(s.unstaged.iter().map(|c| (c.path.as_str(), c.status)).collect::<Vec<_>>(), [("f.txt", 'U')]);
        let git_dir = PathBuf::from(run(&d, &["rev-parse", "--absolute-git-dir"]).stdout.trim());
        assert_eq!(load_refs(&d, Some(&git_dir)).operation, Some("merge"));
        let text = std::fs::read_to_string(d.join("f.txt")).unwrap();
        let parts = parse_conflicts(&text);
        assert!(matches!(&parts[0], ConflictPart::Conflict { ours, theirs, .. } if ours == "ours\n" && theirs == "theirs\n"));
        assert_eq!(resolve_conflict(&text, 0, "theirs").unwrap(), "theirs\n");
        assert_eq!(resolve_conflict(&text, 0, "both").unwrap(), "ours\ntheirs\n");
        assert_eq!(resolve_conflict(&text, 3, "ours"), None);
    }

    #[test]
    fn refs_and_tracking() {
        let d = repo();
        commit(&d, "a", "1\n", "one");
        let hash = run(&d, &["rev-parse", "HEAD"]).stdout.trim().to_string();
        run(&d, &["tag", "v1.9"]);
        run(&d, &["tag", "-a", "v1.10", "-m", "annotated"]);
        run(&d, &["branch", "side"]);
        let r = load_refs(&d, None);
        assert_eq!(r.locals.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["main", "side"]);
        assert_eq!(r.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["v1.10", "v1.9"], "newest number first");
        assert_eq!(r.tags[0].hash, hash, "an annotated tag points at its commit");
        assert_eq!(r.last.unwrap().has_parent, false);
        let refs = "refs/heads/m\0m\0abc\0\0origin/m\0[ahead 2, behind 3]\nrefs/remotes/origin/HEAD\0origin\0abc\0\0\0\nrefs/remotes/origin/m\0origin/m\0abd\0\0\0\n";
        let parsed = parse_refs(refs, "", "origin\n", &Output::default());
        assert_eq!((parsed.locals[0].ahead, parsed.locals[0].behind), (2, 3));
        assert_eq!(parsed.remote_branches.len(), 1, "origin/HEAD isn't a branch");
    }

    #[test]
    fn the_graph_gives_a_merge_its_own_lane() {
        let c = |hash: &str, parents: &[&str]| Commit { hash: hash.into(), parents: parents.iter().map(|p| (*p).to_string()).collect(), ..Commit::default() };
        let mut list = vec![c("m", &["a", "b"]), c("b", &["r"]), c("a", &["r"]), c("r", &[])];
        let lanes = build_graph(&mut list);
        assert_eq!(lanes, 2);
        assert_eq!(list[0].graph.parent_cols, [0, 1]);
        assert_eq!(list[1].graph.col, 1);
        assert_eq!(list[2].graph.col, 0);
        assert_eq!(list[3].graph.col, 0, "they join again at the root");
    }

    #[test]
    fn log_reads_refs_and_parents() {
        let locals: HashSet<String> = ["main".to_string(), "side".to_string()].into();
        let log = "\x1eabc\0p1 p2\0HEAD -> main, origin/main, tag: v1, side\0Ann\0a@x\0100\0subject\n";
        let c = &parse_log(log, &locals)[0];
        assert_eq!(c.parents, ["p1", "p2"]);
        assert_eq!(c.refs.iter().map(|r| r.kind).collect::<Vec<_>>(), [RefKind::Head, RefKind::Remote, RefKind::Tag, RefKind::Local]);
        assert_eq!((c.at, c.subject.as_str()), (100, "subject"));
    }

    #[test]
    fn hunks_stage_and_number_lines() {
        let d = repo();
        commit(&d, "f.txt", "a\nb\nc\n", "base");
        std::fs::write(d.join("f.txt"), "a\nB changed\nc\nnew\n").unwrap();
        let diffs = diffs_by_file(&run(&d, &["diff"]).stdout);
        let f = &diffs["f.txt"];
        let rows = hunk_rows(&f.hunks[0]);
        assert_eq!(rows[0].old, Some(1));
        let add = rows.iter().find(|r| r.kind == LineKind::Add).unwrap();
        assert_eq!(add.new, Some(2));
        assert!(rows.iter().any(|r| r.kind == LineKind::Del && r.old == Some(2)));
        // Staging by patch, exactly as the panel does.
        let patch = hunk_patch(&f.header, &f.hunks[0]);
        assert!(run_with(&d, &["apply", "--cached", "--whitespace=nowarn", "-"], Some(&patch), &[]).ok());
        let s = parse_status(&run(&d, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]).stdout);
        assert_eq!(s.staged.len(), 1);
        assert!(s.unstaged.is_empty());
    }

    #[test]
    fn changed_words_in_a_line() {
        let (a, b) = word_spans("let x = 1;", "let x = 22;").unwrap();
        assert_eq!((a.pre.as_str(), a.mid.as_str(), a.post.as_str()), ("let x = ", "1", ";"));
        assert_eq!(b.mid, "22");
        assert_eq!(word_spans("abc", "xyz"), None);
    }

    #[test]
    fn a_commits_details() {
        let d = repo();
        commit(&d, "a.txt", "1\n", "first\n\nthe body");
        commit(&d, "a.txt", "2\n", "second");
        let root = load_commit(&d, "HEAD~1", None).unwrap();
        assert!(root.parents.is_empty());
        assert_eq!(root.body, "first\n\nthe body");
        assert_eq!(root.files[0].status, 'A');
        assert!(!root.diffs["a.txt"].hunks.is_empty());
        let next = load_commit(&d, "HEAD", None).unwrap();
        assert_eq!(next.parents.len(), 1);
        assert_eq!(next.files[0].status, 'M');
    }

    #[test]
    fn labels_links_and_ignoring() {
        assert_eq!(progressive("Push"), "Pushing");
        assert_eq!(progressive("Check out main"), "Checking out main");
        assert_eq!(progressive("Stage hunk"), "Staging hunk");
        assert_eq!(progressive("Commit"), "Committing");
        assert_eq!(progressive("Merge x"), "Merging x");
        assert_eq!(permalink("git@github.com:o/r.git", "abc", "a b/c.rs").unwrap(), "https://github.com/o/r/blob/abc/a%20b/c.rs");
        assert_eq!(permalink("https://user@gitlab.com/o/r/", "abc", "f").unwrap(), "https://gitlab.com/o/r/-/blob/abc/f");
        assert_eq!(permalink("nothing", "abc", "f"), None);
        let d = repo();
        std::fs::write(d.join(".gitignore"), "one").unwrap();
        ignore(&d, "/two").unwrap();
        ignore(&d, "/two").unwrap();
        assert_eq!(std::fs::read_to_string(d.join(".gitignore")).unwrap(), "one\n/two\n");
    }
}
