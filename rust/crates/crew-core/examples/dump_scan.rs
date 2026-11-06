//! Prints the scan as JSON, to compare with the Electron app's on the same files.

use crew_core::{Config, Paths, providers, scan_projects};
use serde_json::{Value, json};

fn main() {
    let paths = Paths::system();
    let mut config = Config::load(&paths);
    let homes = providers::homes(&paths);
    let start = std::time::Instant::now();
    let (list, _) = scan_projects(&paths, &mut config, &homes);
    eprintln!("first scan {:?}", start.elapsed());
    let start = std::time::Instant::now();
    let _ = scan_projects(&paths, &mut config, &homes);
    eprintln!("second scan {:?}", start.elapsed());
    let path = |p: &std::path::Path| p.to_string_lossy().to_string();
    let out: Vec<Value> = list
        .iter()
        .map(|p| {
            json!({
                "path": path(&p.path), "name": p.name, "mtime": p.mtime, "exists": p.exists,
                "sessions": p.sessions.iter().map(|s| json!({
                    "id": s.id, "agent": s.agent, "file": path(&s.file), "cwd": s.cwd.as_deref().map(path),
                    "branch": s.branch, "title": s.title, "mtime": s.mtime, "size": s.size, "provider": s.provider,
                    "worktree": s.worktree.as_ref().map(|w| json!({"main": path(&w.main), "path": path(&w.path), "name": w.name, "archivedId": w.archived_id})),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string(&out).unwrap());
}
