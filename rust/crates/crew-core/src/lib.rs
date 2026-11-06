//! Crew's data and logic, with no UI: config, the environment agents run in, sessions on disk, how
//! agents start. It keeps its settings and lists in plain files in the data folder.

pub mod agents;
pub mod browser;
pub mod composer;
pub mod config;
pub mod env;
pub mod files;
pub mod git;
pub mod hosts;
pub mod images;
pub mod links;
pub mod providers;
pub mod roster;
pub mod secrets;
pub mod sessions;
pub mod transcript;
pub mod usage;
pub mod worktrees;

pub use config::{Config, Paths};
pub use sessions::{Project, Session, scan_projects};
