//! Everything the UI knows about the team at one moment. Fast parts (files in the data
//! folder) refresh every few seconds; slow parts (task/unit, tools, GitHub) are `None`
//! until first checked.

use super::agent_files::{HistoryEntry, Heartbeat, LogFindings};
use super::logs::{LogFile, PassEntry};
use super::setup::SetupCheck;
use crate::sys::agent_service::ServiceInfo;
use crate::sys::github::GithubData;
use chrono::{DateTime, Local};
use std::collections::BTreeMap;
use std::path::PathBuf;
use team_core::config::TeamConfig;
use team_core::paths::Paths;
use team_core::state::{PoState, UxState};

/// A value that is checked asynchronously: `None` = not checked yet.
pub type Probe<T> = Option<Result<T, String>>;

#[derive(Clone)]
pub struct Snapshot {
    pub paths: Paths,
    /// Err when team.toml is missing (`config_missing`) or invalid
    pub config: Result<TeamConfig, String>,
    pub config_missing: bool,
    pub heartbeats: BTreeMap<String, Heartbeat>,
    /// role → reason (control.json)
    pub pauses: BTreeMap<String, String>,
    pub history: Vec<HistoryEntry>,
    pub discovered: Vec<String>,
    pub po: Option<PoState>,
    pub ux: Option<UxState>,
    pub logs: Vec<LogFile>,
    pub passes: Vec<PassEntry>,
    pub agent_running: bool,
    pub agent_exe: Option<PathBuf>,
    pub service: Probe<ServiceInfo>,
    pub setup: Option<Vec<SetupCheck>>,
    pub github: Probe<GithubData>,
    /// repo → roles enabled by its .agent.toml (Err: not readable / no file)
    pub agent_tomls: BTreeMap<String, Result<Vec<String>, String>>,
    pub agent_toml_at: BTreeMap<String, std::time::Instant>,
    pub log: Option<LogFindings>,
    pub autostart: bool,
    pub refreshed: DateTime<Local>,
    pub slow_refreshed: Option<DateTime<Local>>,
}

impl Snapshot {
    pub fn empty(paths: Paths) -> Self {
        Self {
            paths,
            config: Err("noch nicht geladen".into()),
            config_missing: false,
            heartbeats: BTreeMap::new(),
            pauses: BTreeMap::new(),
            history: Vec::new(),
            discovered: Vec::new(),
            po: None,
            ux: None,
            logs: Vec::new(),
            passes: Vec::new(),
            agent_running: false,
            agent_exe: None,
            service: None,
            setup: None,
            github: None,
            agent_tomls: BTreeMap::new(),
            agent_toml_at: BTreeMap::new(),
            log: None,
            autostart: false,
            refreshed: Local::now(),
            slow_refreshed: None,
        }
    }

    pub fn cfg(&self) -> Option<&TeamConfig> {
        self.config.as_ref().ok()
    }

    /// The repo the GitHub page shows: the PO's, else the first configured one.
    pub fn github_repo(&self) -> String {
        let Some(c) = self.cfg() else { return String::new() };
        if !c.product_owner.repo.is_empty() {
            return c.product_owner.repo.clone();
        }
        c.repos.first().map(|r| r.name.clone()).unwrap_or_default()
    }

    /// Roles switched on in team.toml (all four when the config can't be read).
    pub fn enabled_roles(&self) -> Vec<&'static str> {
        super::team::ROLES.iter().map(|(r, _)| *r).filter(|r| self.cfg().is_none_or(|c| super::team::role_enabled(c, r))).collect()
    }

    /// Every enabled role is paused in control.json.
    pub fn all_paused(&self) -> bool {
        let roles = self.enabled_roles();
        !roles.is_empty() && roles.iter().all(|r| self.pauses.contains_key(*r))
    }
}

/// "vor 5 Min." / "vor 3 Std." / "vor 2 Tagen" / "in 12 Min."
pub fn relative(t: DateTime<Local>, now: DateTime<Local>) -> String {
    let secs = (now - t).num_seconds();
    let (future, s) = (secs < 0, secs.unsigned_abs());
    let text = match s {
        0..=59 => return if future { "gleich".into() } else { "gerade eben".into() },
        60..=3599 => format!("{} Min.", s / 60),
        3600..=172_799 => format!("{} Std.", s / 3600),
        _ => format!("{} Tagen", s / 86_400),
    };
    if future { format!("in {text}") } else { format!("vor {text}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn relative_times_read_naturally() {
        let now = Local::now();
        assert_eq!(relative(now - Duration::minutes(5), now), "vor 5 Min.");
        assert_eq!(relative(now - Duration::hours(50), now), "vor 2 Tagen");
        assert_eq!(relative(now + Duration::minutes(12), now), "in 12 Min.");
        assert_eq!(relative(now, now), "gerade eben");
    }

    #[test]
    fn pause_all_counts_only_enabled_roles() {
        let mut s = Snapshot::empty(Paths::new("x"));
        let mut cfg = TeamConfig::default();
        cfg.roles.product_owner = false;
        s.config = Ok(cfg);
        for r in ["coder", "qa"] {
            s.pauses.insert(r.into(), "x".into());
        }
        assert!(!s.all_paused());
        s.pauses.insert("pr-feedback".into(), "x".into());
        assert!(s.all_paused());
    }

    #[test]
    fn github_repo_prefers_the_product_owner() {
        let mut s = Snapshot::empty(Paths::new("x"));
        let mut cfg = TeamConfig { repos: vec![team_core::config::RepoConfig { name: "o/first".into(), ..Default::default() }], ..Default::default() };
        s.config = Ok(cfg.clone());
        assert_eq!(s.github_repo(), "o/first");
        cfg.product_owner.repo = "o/po".into();
        s.config = Ok(cfg);
        assert_eq!(s.github_repo(), "o/po");
    }
}
