//! The commands sessions use, and what teaches Claude Code and Codex to use them. Their folder and
//! bridge file are this app's own: the Electron app can run beside it with its own pipe.

use std::collections::HashMap;
use std::path::PathBuf;

use crew_core::Paths;
use serde_json::json;

use crate::bridge::BridgeInfo;

fn bin_dir(paths: &Paths) -> PathBuf {
    paths.data_dir.join("bin-rs")
}

fn bridge_file(paths: &Paths) -> PathBuf {
    paths.data_dir.join("ssh-bridge-rs.json")
}

/// crew-cli.exe sits beside the app. Each command is a .cmd for cmd and PowerShell and a script for Git
/// Bash, as Codex's rules and the agents expect the names `crew-ssh` and `crew`.
pub fn install_cli(paths: &Paths, bridge: &BridgeInfo) -> bool {
    let cli = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("crew-cli.exe")));
    let Some(cli) = cli else { return false };
    let dir = bin_dir(paths);
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let file = bridge_file(paths);
    let _ = std::fs::write(&file, json!({"pipe": bridge.pipe, "token": bridge.token}).to_string());
    let fwd = |p: &PathBuf| p.to_string_lossy().replace('\\', "/");
    for (name, sub) in [("crew-ssh", "ssh"), ("crew", "crew")] {
        let cmd = format!("@echo off\r\nsetlocal\r\nset \"CREW_BRIDGE_FILE={}\"\r\n\"{}\" {sub} %*\r\nexit /b %errorlevel%\r\n", file.display(), cli.display());
        let sh = format!("#!/bin/sh\nCREW_BRIDGE_FILE=\"{}\" exec \"{}\" {sub} \"$@\"\n", fwd(&file), fwd(&cli));
        if std::fs::write(dir.join(format!("{name}.cmd")), cmd).is_err() || std::fs::write(dir.join(name), sh).is_err() {
            return false;
        }
    }
    true
}

/// What a session needs to use crew-ssh and crew: the commands on its PATH and the way to reach Crew.
pub fn session_env(paths: &Paths, bridge: &BridgeInfo, mut env: HashMap<String, String>, session: &str, agent: &str) -> HashMap<String, String> {
    let key = env.keys().find(|k| k.eq_ignore_ascii_case("PATH")).cloned().unwrap_or_else(|| "PATH".to_string());
    let current = env.get(&key).cloned().unwrap_or_default();
    env.insert(key, format!("{};{current}", bin_dir(paths).display()));
    env.insert("CREW_SSH_PIPE".into(), bridge.pipe.clone());
    env.insert("CREW_SSH_TOKEN".into(), bridge.token.clone());
    env.insert("CREW_SESSION_ID".into(), session.to_string());
    env.insert("CREW_AGENT".into(), agent.to_string());
    env
}

impl crate::Service {
    pub fn session_env(&self, env: HashMap<String, String>, session: &str, agent: &str) -> HashMap<String, String> {
        session_env(self.paths(), &self.0.bridge, env, session, agent)
    }

    pub fn install_skill(&self) -> Vec<PathBuf> {
        install_skill(self.paths())
    }

    pub fn write_codex_rules(&self) {
        write_codex_rules(self.paths(), true);
    }
}

const SKILL: &str = include_str!("skill.md");

fn skill_targets(paths: &Paths) -> [PathBuf; 2] {
    [paths.home.join(".claude").join("skills").join("crew-ssh").join("SKILL.md"), paths.codex_home.join("skills").join("crew-ssh").join("SKILL.md")]
}

pub fn install_skill(paths: &Paths) -> Vec<PathBuf> {
    skill_targets(paths)
        .into_iter()
        .filter(|t| t.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(t, SKILL).is_ok())
        .collect()
}

/// A skill an earlier Crew installed is brought up to date with this one (only Crew's own file).
pub fn refresh_skill(paths: &Paths) {
    for t in skill_targets(paths) {
        if let Ok(old) = std::fs::read_to_string(&t) {
            if old != SKILL && old.contains("# Remote hosts through Crew") {
                let _ = std::fs::write(&t, SKILL);
            }
        }
    }
}

const CODEX_RULES: &str = "# Written by Crew: its crew-ssh and crew commands talk to Crew over a private pipe, which\n# Codex's sandbox can't open. Crew asks you before anything that changes a host.\nprefix_rule(pattern=[[\"crew-ssh\", \"crew-ssh.cmd\"]], decision=\"allow\")\nprefix_rule(pattern=[[\"crew\", \"crew.cmd\"]], decision=\"allow\")\n";

/// Codex's sandbox can't open Crew's pipe, so its own rules let the two commands out of it.
pub fn write_codex_rules(paths: &Paths, always: bool) {
    if !always && !paths.codex_home.exists() {
        return;
    }
    let target = paths.codex_home.join("rules").join("crew.rules");
    if std::fs::read_to_string(&target).is_ok_and(|t| t == CODEX_RULES) {
        return;
    }
    if target.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) {
        let _ = std::fs::write(&target, CODEX_RULES);
    }
}
