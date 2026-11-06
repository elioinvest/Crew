//! The agents sessions can @-mention, in agents.json.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Paths;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RosterAgent {
    pub id: String,
    pub name: String,
    /// "claude", "codex" or "provider:<id>".
    pub cli: String,
    pub model: String,
    pub effort: String,
    pub instructions: String,
    /// "read", "edit" or "full".
    pub access: String,
}

fn file(paths: &Paths) -> PathBuf {
    paths.data_dir.join("agents.json")
}

pub fn clean_name(text: &str) -> String {
    text.trim().trim_start_matches('@').split_whitespace().collect()
}

pub fn list(paths: &Paths) -> Vec<RosterAgent> {
    let Ok(text) = std::fs::read_to_string(file(paths)) else { return Vec::new() };
    let mut agents: Vec<RosterAgent> = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

fn write(paths: &Paths, agents: &[RosterAgent]) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.data_dir)?;
    std::fs::write(file(paths), serde_json::to_string_pretty(agents)?)?;
    Ok(())
}

pub fn find(paths: &Paths, reference: &str) -> Option<RosterAgent> {
    let name = clean_name(reference).to_lowercase();
    list(paths).into_iter().find(|a| a.name.to_lowercase() == name || a.id == reference)
}

/// `known_provider` says whether a "provider:<id>" still exists. The error is for the user.
pub fn save(paths: &Paths, mut input: RosterAgent, known_provider: impl Fn(&str) -> bool) -> Result<RosterAgent, String> {
    let name = clean_name(&input.name);
    if name.is_empty() {
        return Err("Give the agent a name (no spaces), e.g. CodeReviewerAgent.".into());
    }
    let mut agents = list(paths);
    if agents.iter().any(|a| a.id != input.id && a.name.to_lowercase() == name.to_lowercase()) {
        return Err(format!("There's already an agent named {name}."));
    }
    input.name = name;
    if input.id.is_empty() {
        input.id = uuid::Uuid::new_v4().to_string();
    }
    input.cli = match input.cli.as_str() {
        "codex" => "codex".into(),
        c if c.strip_prefix("provider:").is_some_and(&known_provider) => c.into(),
        _ => "claude".into(),
    };
    input.model = input.model.trim().to_string();
    let effort_ok = (2..=10).contains(&input.effort.len()) && input.effort.chars().all(|c| c.is_ascii_lowercase());
    if !effort_ok {
        input.effort.clear();
    }
    input.instructions = input.instructions.trim().to_string();
    if !["read", "edit", "full"].contains(&input.access.as_str()) {
        input.access = "read".into();
    }
    match agents.iter_mut().find(|a| a.id == input.id) {
        Some(a) => *a = input.clone(),
        None => agents.push(input.clone()),
    }
    write(paths, &agents).map_err(|e| e.to_string())?;
    Ok(input)
}

pub fn delete(paths: &Paths, id: &str) -> anyhow::Result<()> {
    let mut agents = list(paths);
    agents.retain(|a| a.id != id);
    write(paths, &agents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_cleans_up_and_refuses_a_taken_name() {
        let d = std::env::temp_dir().join(format!("crew-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let paths = Paths { home: d.clone(), data_dir: d.clone(), codex_home: d.join(".codex") };
        let a = save(&paths, RosterAgent { name: "@Code Reviewer ".into(), cli: "provider:gone".into(), effort: "WRONG".into(), access: "x".into(), ..Default::default() }, |_| false).unwrap();
        assert_eq!((a.name.as_str(), a.cli.as_str(), a.effort.as_str(), a.access.as_str()), ("CodeReviewer", "claude", "", "read"));
        assert!(save(&paths, RosterAgent { name: "codereviewer".into(), ..Default::default() }, |_| true).is_err());
        assert!(save(&paths, RosterAgent { name: "  ".into(), ..Default::default() }, |_| true).is_err());
        assert_eq!(find(&paths, "@codereviewer").unwrap().id, a.id);
        delete(&paths, &a.id).unwrap();
        assert!(list(&paths).is_empty());
    }
}
