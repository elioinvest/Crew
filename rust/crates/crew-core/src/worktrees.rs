//! Which repository a folder's sessions belong to. A linked worktree's sessions are listed under
//! the repository's project. (Creating and archiving worktrees comes with the Git panel.)

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::Paths;
use crate::git;

pub fn norm(p: &Path) -> String {
    let full = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    // Long form: Windows may hand out C:\PROGRA~1 where git says C:\Program Files.
    let long = long_path(&full).unwrap_or(full);
    long.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
}

#[cfg(windows)]
fn long_path(p: &Path) -> Option<PathBuf> {
    let full = std::fs::canonicalize(p).ok()?;
    let s = full.to_string_lossy();
    Some(PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()))
}

#[cfg(not(windows))]
fn long_path(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

fn linked_git_dir(root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(root.join(".git")).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?;
    std::path::absolute(root.join(line.trim())).ok()
}

/// <main>/.git/worktrees/<id> -> <main>
fn main_from_git_dir(git_dir: &Path) -> Option<PathBuf> {
    let worktrees = git_dir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let common = worktrees.parent()?;
    if common.file_name()? != ".git" {
        return None; // a bare repository has no main folder
    }
    common.parent().map(Path::to_path_buf)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionWorktree {
    pub main: PathBuf,
    pub path: PathBuf,
    pub name: String,
    pub archived_id: Option<String>,
}

/// What Crew recorded about the worktrees it made.
#[derive(Default)]
pub struct Registry {
    created: Vec<(String, PathBuf)>,
    archived: Vec<(String, PathBuf, String)>,
}

impl Registry {
    pub fn load(paths: &Paths) -> Self {
        let mut r = Self::default();
        let Ok(text) = std::fs::read_to_string(paths.data_dir.join("worktrees.json")) else { return r };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { return r };
        if let Some(created) = v.get("created").and_then(Value::as_object) {
            for (k, e) in created {
                if let Some(main) = e.get("main").and_then(Value::as_str) {
                    r.created.push((k.clone(), PathBuf::from(main)));
                }
            }
        }
        if let Some(list) = v.get("archived").and_then(Value::as_array) {
            for a in list {
                if let (Some(p), Some(main), Some(id)) = (
                    a.get("path").and_then(Value::as_str),
                    a.get("main").and_then(Value::as_str),
                    a.get("id").and_then(Value::as_str),
                ) {
                    r.archived.push((norm(Path::new(p)), PathBuf::from(main), id.to_string()));
                }
            }
        }
        r
    }

    /// For the sidebar: the worktree a folder is, or was before it was archived.
    pub fn session_worktree(&self, folder: &Path) -> Option<SessionWorktree> {
        let key = norm(folder);
        let archived = self.archived.iter().find(|a| a.0 == key);
        let main = self
            .created
            .iter()
            .find(|c| c.0 == key)
            .map(|c| c.1.clone())
            .or_else(|| archived.map(|a| a.1.clone()))
            .or_else(|| linked_git_dir(folder).and_then(|d| main_from_git_dir(&d)))?;
        Some(SessionWorktree {
            main,
            path: folder.to_path_buf(),
            name: folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            archived_id: archived.map(|a| a.2.clone()),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub root: PathBuf,
    pub main: PathBuf,
    pub linked: bool,
    pub name: String,
    pub branch: Option<String>,
    pub sha: Option<String>,
}

/// Which checkout a folder is in, on which branch. Reads files only, so it can run often.
pub fn head_of(cwd: &Path) -> Option<Head> {
    if !cwd.exists() {
        return None;
    }
    let start = std::path::absolute(cwd).ok()?;
    let mut dir = Some(start.as_path());
    let (root, is_file) = loop {
        let d = dir?;
        let dot = d.join(".git");
        if let Ok(meta) = std::fs::metadata(&dot) {
            break (d.to_path_buf(), meta.is_file());
        }
        dir = d.parent();
    };
    let linked = if is_file { Some(linked_git_dir(&root)?) } else { None };
    let git_dir = linked.clone().unwrap_or_else(|| root.join(".git"));
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let branch = head.strip_prefix("ref: refs/heads/").map(str::to_string);
    let sha = branch.is_none().then(|| head.chars().take(7).collect());
    let main = linked.as_deref().and_then(main_from_git_dir).unwrap_or_else(|| root.clone());
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    Some(Head { root, main, linked: linked.is_some(), name, branch, sha })
}

// ---------- managing worktrees ----------
//
// The way Zed's work (behaviour only; written for Crew). New worktrees start on a detached HEAD, so
// no branch ends up shared; a branch is picked after. Crew records the ones it created: only those can
// be archived, and only while the folder is still the one it created. When in doubt, nothing is
// deleted. Archiving saves the staged and unstaged state as two detached commits kept by a ref, then
// removes the folder; restoring puts the folder, the branch and that state back.

const DEFAULT_DIR: &str = "../worktrees";
const ARCHIVE_REF: &str = "refs/crew-archived-worktrees";
const WIP_ENV: [(&str, &str); 4] = [("GIT_AUTHOR_NAME", "Crew"), ("GIT_AUTHOR_EMAIL", "crew@localhost"), ("GIT_COMMITTER_NAME", "Crew"), ("GIT_COMMITTER_EMAIL", "crew@localhost")];
const ADJECTIVES: [&str; 60] = ["amber", "ashen", "azure", "bold", "brisk", "calm", "cedar", "clear", "cobalt", "coral", "crisp", "dawn", "deep", "dusky", "eager", "early", "fern", "fleet", "fresh", "gentle", "golden", "green", "hazel", "honest", "indigo", "ivory", "jade", "keen", "kind", "lively", "lucid", "maple", "mellow", "misty", "mossy", "noble", "olive", "pale", "pearl", "plain", "quick", "quiet", "rapid", "rosy", "rustic", "sandy", "silver", "snowy", "solid", "spruce", "steady", "still", "sunny", "swift", "tidy", "topaz", "vivid", "warm", "wild", "windy"];
const NOUNS: [&str; 60] = ["anchor", "aspen", "badger", "basin", "beacon", "birch", "bison", "brook", "canyon", "cedar", "cliff", "cloud", "comet", "crane", "creek", "delta", "dune", "eagle", "ember", "falcon", "fjord", "forest", "fox", "garden", "glade", "grove", "harbor", "hawk", "heron", "hill", "island", "lagoon", "lake", "lark", "lynx", "meadow", "mesa", "moon", "oak", "orchard", "otter", "owl", "peak", "pine", "pond", "prairie", "quartz", "raven", "reef", "ridge", "river", "robin", "shore", "sparrow", "spring", "stone", "stream", "summit", "valley", "willow"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub name: String,
    pub sha: String,
    pub branch: Option<String>,
    pub is_main: bool,
    pub prunable: bool,
    pub locked: bool,
    /// Crew made it, so it can be archived.
    pub crew: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Archived {
    pub id: String,
    pub path: PathBuf,
    pub main: PathBuf,
    pub name: String,
    pub branch: Option<String>,
    pub head: String,
    pub staged: String,
    pub unstaged: String,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeList {
    pub worktrees: Vec<Worktree>,
    pub main: PathBuf,
    pub default_branch: Option<String>,
    pub archived: Vec<Archived>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    pub name: String,
    pub main: PathBuf,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoveError {
    /// Git refuses because of changes in the folder; the caller asks, then forces.
    Dirty,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    pub path: PathBuf,
    pub name: String,
    pub main: PathBuf,
    pub note: Option<String>,
}

fn registry_file(paths: &Paths) -> PathBuf {
    paths.data_dir.join("worktrees.json")
}

fn read_registry(paths: &Paths) -> Value {
    let mut v: Value = std::fs::read_to_string(registry_file(paths)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    if !v.is_object() {
        v = serde_json::json!({});
    }
    if !v.get("created").is_some_and(Value::is_object) {
        v["created"] = serde_json::json!({});
    }
    if !v.get("archived").is_some_and(Value::is_array) {
        v["archived"] = serde_json::json!([]);
    }
    v
}

fn write_registry(paths: &Paths, v: &Value) {
    let _ = std::fs::create_dir_all(&paths.data_dir);
    if let Ok(text) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(registry_file(paths), text);
    }
}

fn windows_path(p: &str) -> PathBuf {
    PathBuf::from(p.replace('/', "\\"))
}

pub fn parse_list(out: &str) -> Vec<Worktree> {
    let normalized = out.replace("\r\n", "\n");
    let mut items = Vec::new();
    for block in normalized.split("\n\n") {
        let mut w = Worktree::default();
        for line in block.lines() {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            match k {
                "worktree" => w.path = windows_path(v),
                "HEAD" => w.sha = v.to_string(),
                "branch" => w.branch = Some(v.strip_prefix("refs/heads/").unwrap_or(v).to_string()),
                "prunable" => w.prunable = true,
                "locked" => w.locked = true,
                _ => {}
            }
        }
        if !w.path.as_os_str().is_empty() {
            w.is_main = items.is_empty();
            w.name = w.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            items.push(w);
        }
    }
    items
}

pub fn list_worktrees(paths: &Paths, cwd: &Path) -> Result<WorktreeList, String> {
    let r = git::run(cwd, &["worktree", "list", "--porcelain"]);
    if !r.ok() {
        return Err(r.message());
    }
    let mut worktrees = parse_list(&r.stdout);
    let main = worktrees.first().map(|w| w.path.clone()).unwrap_or_default();
    let registry = read_registry(paths);
    let created = &registry["created"];
    for w in &mut worktrees {
        w.crew = created.get(norm(&w.path)).is_some();
    }
    // origin/HEAD, when it's known, is what a new worktree is best based on.
    let from = if main.as_os_str().is_empty() { cwd } else { main.as_path() };
    let dflt = git::run(from, &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"]);
    let default_branch = dflt.ok().then(|| dflt.stdout.trim().to_string()).filter(|s| !s.is_empty());
    let main_key = norm(&main);
    let archived = registry["archived"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let text = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let item = Archived {
                id: text("id"),
                path: PathBuf::from(text("path")),
                main: PathBuf::from(text("main")),
                name: text("name"),
                branch: a.get("branch").and_then(Value::as_str).map(str::to_string),
                head: text("head"),
                staged: text("staged"),
                unstaged: text("unstaged"),
                at: a.get("at").and_then(Value::as_i64).unwrap_or(0),
            };
            (norm(&item.main) == main_key).then_some(item)
        })
        .collect();
    Ok(WorktreeList { worktrees, main, default_branch, archived })
}

/// Where new worktrees of this repository go: the setting, relative to the main checkout. When that's
/// outside the project, the project's name is added so sibling repositories don't collide.
fn base_dir(setting: &str, main: &Path) -> PathBuf {
    let trimmed = setting.trim().trim_end_matches(['\\', '/']);
    let setting = if trimmed.is_empty() { DEFAULT_DIR } else { trimmed };
    let dir = std::path::absolute(main.join(setting)).unwrap_or_else(|_| main.join(setting));
    if dir.starts_with(main) { dir } else { dir.join(main.file_name().unwrap_or_default()) }
}

fn generate_name(taken: &[String]) -> Option<String> {
    for _ in 0..12 {
        let bytes = uuid::Uuid::new_v4().into_bytes();
        let name = format!("{}-{}", ADJECTIVES[bytes[0] as usize % ADJECTIVES.len()], NOUNS[bytes[1] as usize % NOUNS.len()]);
        if !taken.contains(&name) {
            return Some(name);
        }
    }
    None
}

fn name_problem(name: &str) -> Option<&'static str> {
    let good_start = name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    let good_chars = name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    (!(good_start && good_chars && !name.contains("..") && !name.ends_with(".lock") && !name.ends_with('.'))).then_some("Use letters, numbers, dots, dashes and underscores in the name.")
}

/// When the metadata folder git made for this worktree was created. It identifies this particular
/// worktree: one removed and made again outside Crew has a different time.
fn git_dir_born(worktree: &Path) -> Option<f64> {
    let dir = linked_git_dir(worktree)?;
    let t = std::fs::metadata(dir).ok()?.created().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64() * 1000.)
}

fn remember(paths: &Paths, worktree: &Path, main: &Path) {
    let mut reg = read_registry(paths);
    reg["created"][norm(worktree)] = serde_json::json!({
        "path": worktree.to_string_lossy(),
        "main": main.to_string_lossy(),
        "name": worktree.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        "gitDirBorn": git_dir_born(worktree),
    });
    write_registry(paths, &reg);
}

/// `base`: a remote and branch (fetched first; the copy already here is used, with a warning, when
/// that fails), or nothing for the current HEAD.
pub fn create_worktree(paths: &Paths, setting: &str, cwd: &Path, name: Option<&str>, base: Option<(&str, &str)>) -> Result<Created, String> {
    let info = list_worktrees(paths, cwd)?;
    if info.main.as_os_str().is_empty() {
        return Err("Not a Git repository.".into());
    }
    let dir = base_dir(setting, &info.main);
    let mut taken: Vec<String> = info.worktrees.iter().map(|w| w.name.clone()).collect();
    taken.extend(std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()));
    let wanted = name.unwrap_or("").split_whitespace().collect::<Vec<_>>().join("-");
    if !wanted.is_empty() {
        if let Some(problem) = name_problem(&wanted) {
            return Err(problem.into());
        }
        if taken.contains(&wanted) {
            return Err(format!("A worktree named \"{wanted}\" already exists."));
        }
    }
    let final_name = if wanted.is_empty() { generate_name(&taken).ok_or("Couldn't find an unused name. Type one instead.")? } else { wanted };
    let target = dir.join(&final_name);
    let mut reference = "HEAD".to_string();
    let mut warning = None;
    if let Some((remote, branch)) = base {
        reference = format!("{remote}/{branch}");
        let f = git::run(&info.main, &["fetch", remote, branch]);
        if !f.ok() {
            warning = Some(format!("Couldn't fetch {reference}, so this uses the copy already here. {}", f.stderr.trim()));
        }
    }
    // HEAD of the folder the request came from, which may itself be a linked worktree.
    let from = if reference == "HEAD" { cwd } else { info.main.as_path() };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let target_text = target.to_string_lossy().to_string();
    let r = git::run(from, &["worktree", "add", "--detach", "--", &target_text, &reference]);
    if !r.ok() {
        return Err(r.message());
    }
    remember(paths, &target, &info.main);
    Ok(Created { path: target, name: final_name, main: info.main, warning })
}

/// The setup command runs in the new worktree; a failure doesn't undo it. Blocks. Err is its output.
pub fn run_setup(command: &str, worktree: &Path, main: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()));
    cmd.args(["/d", "/c", command]).current_dir(worktree).env("CREW_WORKTREE_ROOT", worktree).env("CREW_MAIN_GIT_WORKTREE", main);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let tail: Vec<char> = text.chars().collect();
    Err(tail[tail.len().saturating_sub(2000)..].iter().collect::<String>().trim().to_string())
}

/// Removes the worktree's now empty parent folders, up to the worktree directory's own parent.
fn tidy_parents(setting: &str, worktree: &Path, main: &Path) {
    let Some(stop) = base_dir(setting, main).parent().map(Path::to_path_buf) else { return };
    let mut dir = worktree.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == stop || d.as_os_str().len() <= stop.as_os_str().len() || std::fs::remove_dir(&d).is_err() {
            return;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
}

pub fn remove_worktree(paths: &Paths, setting: &str, cwd: &Path, target: &Path, force: bool) -> Result<(), RemoveError> {
    let info = list_worktrees(paths, cwd).map_err(RemoveError::Other)?;
    let key = norm(target);
    let w = info.worktrees.iter().find(|x| norm(&x.path) == key).ok_or_else(|| RemoveError::Other("That worktree no longer exists.".into()))?;
    if w.is_main {
        return Err(RemoveError::Other("The main checkout can't be removed.".into()));
    }
    let path_text = w.path.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", &path_text]);
    let r = git::run(&info.main, &args);
    if !r.ok() {
        let msg = r.message();
        let low = msg.to_lowercase();
        return Err(if low.contains("contains modified or untracked files") && low.contains("--force") { RemoveError::Dirty } else { RemoveError::Other(msg) });
    }
    let mut reg = read_registry(paths);
    if let Some(created) = reg["created"].as_object_mut() {
        created.remove(&key);
    }
    write_registry(paths, &reg);
    tidy_parents(setting, &w.path, &info.main);
    Ok(())
}

/// Is this still the worktree Crew made? Anything uncertain means no.
fn verify_created(paths: &Paths, worktree: &Path) -> Option<&'static str> {
    let mut reg = read_registry(paths);
    let key = norm(worktree);
    let Some(rec) = reg["created"].get(&key) else { return Some("Crew didn't create this worktree, so it won't archive it. You can still delete it.") };
    let (now, then) = (git_dir_born(worktree), rec.get("gitDirBorn").and_then(Value::as_f64));
    let (Some(now), Some(then)) = (now, then) else { return Some("Couldn't confirm this is the worktree Crew created.") };
    if (now - then).abs() > 1000. {
        if let Some(created) = reg["created"].as_object_mut() {
            created.remove(&key);
        }
        write_registry(paths, &reg);
        return Some("This folder was recreated outside Crew, so it won't be archived.");
    }
    None
}

/// Two commits that no branch points at: the index, then everything in the folder on top of it
/// (through a temporary index, so the real one isn't touched).
fn snapshot(worktree: &Path, head: &str) -> Result<(String, String), String> {
    let tree1 = git::run(worktree, &["write-tree"]);
    if !tree1.ok() {
        let m = tree1.stderr.trim();
        return Err(if m.is_empty() { "Couldn't read the staged changes. Finish any merge first.".to_string() } else { m.to_string() });
    }
    let staged = git::run_with(worktree, &["commit-tree", tree1.stdout.trim(), "-p", head, "-m", "WIP staged (Crew archive)"], None, &WIP_ENV);
    if !staged.ok() {
        return Err(staged.stderr.trim().to_string());
    }
    let staged_hash = staged.stdout.trim().to_string();
    let index_file = std::env::temp_dir().join(format!("crew-archive-{}.index", uuid::Uuid::new_v4()));
    let real = git::run(worktree, &["rev-parse", "--git-path", "index"]);
    let result = (|| {
        std::fs::copy(worktree.join(real.stdout.trim()), &index_file).map_err(|e| e.to_string())?;
        let index_text = index_file.to_string_lossy().to_string();
        let env = [("GIT_INDEX_FILE", index_text.as_str())];
        let add = git::run_with(worktree, &["add", "-A"], None, &env);
        if !add.ok() {
            return Err(add.stderr.trim().to_string());
        }
        let tree2 = git::run_with(worktree, &["write-tree"], None, &env);
        if !tree2.ok() {
            return Err(tree2.stderr.trim().to_string());
        }
        let unstaged = git::run_with(worktree, &["commit-tree", tree2.stdout.trim(), "-p", &staged_hash, "-m", "WIP unstaged (Crew archive)"], None, &WIP_ENV);
        if !unstaged.ok() {
            return Err(unstaged.stderr.trim().to_string());
        }
        Ok((staged_hash.clone(), unstaged.stdout.trim().to_string()))
    })();
    let _ = std::fs::remove_file(&index_file);
    result
}

pub fn archive_worktree(paths: &Paths, setting: &str, cwd: &Path, target: &Path) -> Result<String, String> {
    let info = list_worktrees(paths, cwd)?;
    let key = norm(target);
    let w = info.worktrees.iter().find(|x| norm(&x.path) == key).ok_or("That worktree no longer exists.")?;
    if w.is_main {
        return Err("The main checkout can't be archived.".into());
    }
    if w.locked {
        return Err("This worktree is locked.".into());
    }
    if let Some(problem) = verify_created(paths, &w.path) {
        return Err(problem.into());
    }
    let (staged, unstaged) = snapshot(&w.path, &w.sha)?;
    // Without this ref, git's cleanup would eventually collect the commits and restoring would fail.
    let id = uuid::Uuid::new_v4().to_string();
    let reference = format!("{ARCHIVE_REF}/{id}");
    let made = git::run(&info.main, &["update-ref", &reference, &unstaged]);
    if !made.ok() {
        return Err(made.stderr.trim().to_string());
    }
    let path_text = w.path.to_string_lossy().to_string();
    let r = git::run(&info.main, &["worktree", "remove", "--force", "--", &path_text]);
    if !r.ok() {
        git::run(&info.main, &["update-ref", "-d", &reference]);
        return Err(r.message());
    }
    let mut reg = read_registry(paths);
    if let Some(created) = reg["created"].as_object_mut() {
        created.remove(&key);
    }
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    if let Some(list) = reg["archived"].as_array_mut() {
        list.push(serde_json::json!({
            "id": id,
            "path": w.path.to_string_lossy(),
            "main": info.main.to_string_lossy(),
            "name": w.name,
            "branch": w.branch,
            "head": w.sha,
            "staged": staged,
            "unstaged": unstaged,
            "at": at,
        }));
    }
    write_registry(paths, &reg);
    tidy_parents(setting, &w.path, &info.main);
    Ok(id)
}

fn take_archived(reg: &mut Value, id: &str) {
    if let Some(list) = reg["archived"].as_array_mut() {
        list.retain(|a| a.get("id").and_then(Value::as_str) != Some(id));
    }
}

pub fn restore_worktree(paths: &Paths, id: &str) -> Result<Restored, String> {
    let reg = read_registry(paths);
    let found = reg["archived"].as_array().and_then(|l| l.iter().find(|a| a.get("id").and_then(Value::as_str) == Some(id))).cloned().ok_or("That archived worktree is gone.")?;
    let text = |k: &str| found.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (path, main, head, staged, unstaged, name) = (PathBuf::from(text("path")), PathBuf::from(text("main")), text("head"), text("staged"), text("unstaged"), text("name"));
    let branch = found.get("branch").and_then(Value::as_str).map(str::to_string);
    if path.exists() {
        return Err(format!("{} already exists. Move it away to restore this worktree.", path.display()));
    }
    git::run(&main, &["worktree", "prune"]);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let path_text = path.to_string_lossy().to_string();
    let add = git::run(&main, &["worktree", "add", "--detach", "--", &path_text, &head]);
    if !add.ok() {
        return Err(add.message());
    }
    // Back on its branch, unless that moved on since: the saved state belongs on the old commit.
    let mut note = None;
    if let Some(b) = &branch {
        let refname = format!("refs/heads/{b}");
        let at = git::run(&main, &["rev-parse", "-q", "--verify", &refname]);
        if !at.ok() {
            if !git::run(&path, &["switch", "-c", b]).ok() {
                note = Some(format!("Couldn't recreate the branch {b}, so this stays on a detached HEAD."));
            }
        } else if at.stdout.trim() == head {
            if !git::run(&path, &["switch", b]).ok() {
                note = Some(format!("{b} is checked out somewhere else, so this stays on a detached HEAD."));
            }
        } else {
            note = Some(format!("{b} has moved since this was archived, so this stays on a detached HEAD at the saved commit."));
        }
    }
    // The folder's contents (deletions too), then the index, leaving the folder alone.
    let files = git::run(&path, &["read-tree", "--reset", "-u", &unstaged]);
    let index = if files.ok() { git::run(&path, &["read-tree", &staged]) } else { files };
    if !index.ok() {
        return Err(format!("The worktree is back, but its saved changes couldn't be restored: {}", index.stderr.trim()));
    }
    let reference = format!("{ARCHIVE_REF}/{id}");
    git::run(&main, &["update-ref", "-d", &reference]);
    let mut reg = read_registry(paths);
    take_archived(&mut reg, id);
    write_registry(paths, &reg);
    remember(paths, &path, &main);
    Ok(Restored { path, name, main, note })
}

/// Removes an archived worktree for good, saved changes included.
pub fn forget_archived(paths: &Paths, id: &str) {
    let mut reg = read_registry(paths);
    let main = reg["archived"].as_array().and_then(|l| l.iter().find(|a| a.get("id").and_then(Value::as_str) == Some(id))).and_then(|a| a.get("main")).and_then(Value::as_str).map(PathBuf::from);
    if let Some(main) = main {
        let reference = format!("{ARCHIVE_REF}/{id}");
        git::run(&main, &["update-ref", "-d", &reference]);
    }
    take_archived(&mut reg, id);
    write_registry(paths, &reg);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Paths, PathBuf) {
        let d = std::env::temp_dir().join(format!("crew-worktrees-{}", uuid::Uuid::new_v4()));
        let repo = d.join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = Paths { home: d.clone(), data_dir: d.join("data"), codex_home: d.join(".codex") };
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "T"]] {
            assert!(git::run(&repo, args).ok());
        }
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git::run(&repo, &["add", "-A"]);
        assert!(git::run(&repo, &["commit", "-q", "-m", "first"]).ok());
        (paths, repo)
    }

    #[test]
    fn creating_goes_detached_under_the_setting_with_the_projects_name() {
        let (paths, repo) = setup();
        let made = create_worktree(&paths, "", &repo, Some("my tree"), None).unwrap();
        assert_eq!(made.name, "my-tree");
        assert!(made.path.ends_with(Path::new("worktrees").join("proj").join("my-tree")));
        assert!(made.path.join("a.txt").exists());
        let info = list_worktrees(&paths, &repo).unwrap();
        assert_eq!(info.worktrees.len(), 2);
        assert!(info.worktrees[0].is_main && !info.worktrees[0].crew);
        assert!(info.worktrees[1].crew && info.worktrees[1].branch.is_none());
        assert!(create_worktree(&paths, "", &repo, Some("my-tree"), None).is_err(), "the name is taken");
        assert!(create_worktree(&paths, "", &repo, Some("bad/name"), None).is_err());
        let generated = create_worktree(&paths, "", &repo, None, None).unwrap();
        assert!(generated.name.contains('-'));
    }

    #[test]
    fn removing_asks_about_changes_first() {
        let (paths, repo) = setup();
        let made = create_worktree(&paths, "", &repo, Some("t"), None).unwrap();
        std::fs::write(made.path.join("new.txt"), "x").unwrap();
        assert_eq!(remove_worktree(&paths, "", &repo, &made.path, false), Err(RemoveError::Dirty));
        assert!(made.path.exists());
        remove_worktree(&paths, "", &repo, &made.path, true).unwrap();
        assert!(!made.path.exists());
        assert!(matches!(remove_worktree(&paths, "", &repo, &repo, true), Err(_)), "the main checkout stays");
    }

    #[test]
    fn archiving_and_restoring_keeps_staged_and_unstaged_changes() {
        let (paths, repo) = setup();
        let made = create_worktree(&paths, "", &repo, Some("t"), None).unwrap();
        git::run(&made.path, &["switch", "-q", "-c", "feature"]);
        std::fs::write(made.path.join("staged.txt"), "s\n").unwrap();
        git::run(&made.path, &["add", "staged.txt"]);
        std::fs::write(made.path.join("a.txt"), "edited\n").unwrap();
        std::fs::write(made.path.join("untracked.txt"), "u\n").unwrap();
        let id = archive_worktree(&paths, "", &repo, &made.path).unwrap();
        assert!(!made.path.exists());
        let listed = list_worktrees(&paths, &repo).unwrap();
        assert_eq!(listed.archived.len(), 1);
        assert_eq!(listed.archived[0].branch.as_deref(), Some("feature"));
        let back = restore_worktree(&paths, &id).unwrap();
        assert_eq!(back.note, None);
        assert_eq!(std::fs::read_to_string(back.path.join("a.txt")).unwrap().replace("\r\n", "\n"), "edited\n");
        assert!(back.path.join("untracked.txt").exists());
        let status = git::parse_status(&git::run(&back.path, &["status", "--porcelain=v2", "--branch", "-z", "-uall"]).stdout);
        assert_eq!(status.branch, "feature");
        assert_eq!(status.staged.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), ["staged.txt"]);
        assert!(status.unstaged.iter().any(|c| c.path == "a.txt"));
        assert!(status.unstaged.iter().any(|c| c.path == "untracked.txt" && c.status == '?'));
        assert!(list_worktrees(&paths, &repo).unwrap().archived.is_empty());
    }

    #[test]
    fn only_what_crew_made_is_archived_and_archives_can_be_dropped() {
        let (paths, repo) = setup();
        let outside = repo.parent().unwrap().join("by-hand");
        let text = outside.to_string_lossy().to_string();
        assert!(git::run(&repo, &["worktree", "add", "--detach", "--", &text]).ok());
        assert!(archive_worktree(&paths, "", &repo, &outside).is_err());
        assert!(outside.exists(), "nothing was deleted");
        let made = create_worktree(&paths, "", &repo, Some("t"), None).unwrap();
        let id = archive_worktree(&paths, "", &repo, &made.path).unwrap();
        forget_archived(&paths, &id);
        assert!(list_worktrees(&paths, &repo).unwrap().archived.is_empty());
        assert!(restore_worktree(&paths, &id).is_err());
    }

    #[test]
    fn reads_the_porcelain_list() {
        let out = "worktree C:/a/main\nHEAD abc\nbranch refs/heads/main\n\nworktree C:/a/other\nHEAD def\ndetached\nprunable gone\n\n";
        let items = parse_list(out);
        assert_eq!(items.len(), 2);
        assert!(items[0].is_main);
        assert_eq!(items[0].branch.as_deref(), Some("main"));
        assert_eq!(items[1].name, "other");
        assert!(items[1].prunable && items[1].branch.is_none());
    }
}
