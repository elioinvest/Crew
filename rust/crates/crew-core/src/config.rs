//! Crew's settings: the same config.json as the Electron app, so the two share them. Keys this
//! version doesn't know yet are kept, so saving never loses what the other wrote.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// Where things live. Everything hangs off these, so tests can point them at a scratch folder.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub codex_home: PathBuf,
}

impl Paths {
    pub fn system() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"));
        let data_dir = std::env::var_os("CREW_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| {
            let app_data = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Roaming"));
            // Earlier versions were called Tandem and Claude Terminal; a folder in use stays in use.
            let current = app_data.join("Crew");
            if current.join("config.json").exists() {
                return current;
            }
            ["Tandem", "Claude Terminal"]
                .iter()
                .map(|n| app_data.join(n))
                .find(|d| d.join("config.json").exists())
                .unwrap_or(current)
        });
        Self { home, data_dir, codex_home }
    }

    pub fn claude_projects(&self) -> PathBuf {
        self.home.join(".claude").join("projects")
    }

    /// Codex Desktop puts project-less chats in dated folders here; they're grouped as one project.
    pub fn codex_chats(&self) -> PathBuf {
        self.home.join("Documents").join("Codex")
    }

    pub fn config_file(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    pub fn provider_homes(&self) -> PathBuf {
        self.data_dir.join("provider-homes")
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    value: Map<String, Value>,
}

fn defaults() -> Map<String, Value> {
    let v = json!({
        "claudeCommand": "claude",
        "extraArgs": "",
        "codexCommand": "codex",
        "codexArgs": "",
        "defaultAgent": "claude",
        "theme": "light",
        "fontSize": 14,
        "extraProjects": [],
        "hiddenProjects": [],
        "collapsed": {},
        "projectOrder": [],
        "closeToTray": true,
        "language": "auto",
        "showUsage": true,
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Self {
        Self::load_file(&paths.config_file())
    }

    pub fn load_file(file: &Path) -> Self {
        let mut value = defaults();
        if let Ok(text) = std::fs::read_to_string(file) {
            if let Ok(Value::Object(saved)) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
                value.extend(saved);
            }
        }
        Self { value }
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        std::fs::create_dir_all(&paths.data_dir)?;
        let text = serde_json::to_string_pretty(&Value::Object(self.value.clone()))?;
        // A half-written config must never replace a good one.
        let tmp = paths.config_file().with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, paths.config_file())?;
        Ok(())
    }

    pub fn str(&self, key: &str) -> &str {
        self.value.get(key).and_then(Value::as_str).unwrap_or("")
    }

    pub fn bool(&self, key: &str) -> bool {
        self.value.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn f32(&self, key: &str, default: f32) -> f32 {
        self.value.get(key).and_then(Value::as_f64).map(|v| v as f32).unwrap_or(default)
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        self.value
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.value.insert(key.to_string(), value);
    }

    pub fn is_collapsed(&self, key: &str) -> bool {
        self.value.get("collapsed").and_then(|c| c.get(key)).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn toggle_collapsed(&mut self, key: &str) {
        let entry = self.value.entry("collapsed").or_insert_with(|| json!({}));
        if let Value::Object(m) = entry {
            if m.remove(key).is_none() {
                m.insert(key.to_string(), Value::Bool(true));
            }
        }
    }

    pub fn raw(&self) -> &Map<String, Value> {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("crew-core-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keeps_keys_it_does_not_know() {
        let d = scratch("cfg");
        let paths = Paths { home: d.clone(), data_dir: d.clone(), codex_home: d.join(".codex") };
        std::fs::write(paths.config_file(), r#"{"theme":"dark","worktreeSetup":"x","usagePrices":{"m":{"input":1}}}"#).unwrap();
        let mut c = Config::load(&paths);
        assert_eq!(c.str("theme"), "dark");
        assert_eq!(c.str("claudeCommand"), "claude", "defaults fill the gaps");
        c.set("theme", json!("light"));
        c.toggle_collapsed("a");
        c.save(&paths).unwrap();
        let again = Config::load(&paths);
        assert_eq!(again.str("theme"), "light");
        assert_eq!(again.str("worktreeSetup"), "x");
        assert!(again.raw().contains_key("usagePrices"));
        assert!(again.is_collapsed("a"));
    }
}
