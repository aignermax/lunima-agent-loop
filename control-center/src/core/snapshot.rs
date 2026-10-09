//! Everything the UI knows about the loop at one moment. Fast parts (files) refresh every
//! few seconds; slow parts (scheduled task, git, GitHub) are `None` until first checked.

use super::config::ConfigDoc;
use super::logs::{LogFile, PassEntry};
use super::state::LoopState;
use crate::sys::git::CloneStatus;
use crate::sys::github::GithubData;
use crate::sys::schedtask::TaskInfo;
use chrono::{DateTime, Local};
use std::path::PathBuf;

/// A value that is checked asynchronously: `None` = not checked yet.
pub type Probe<T> = Option<Result<T, String>>;

#[derive(Clone)]
pub struct Snapshot {
    pub root: PathBuf,
    pub cli: Option<PathBuf>,
    pub config: Result<ConfigDoc, String>,
    pub state: Result<LoopState, String>,
    pub logs: Vec<LogFile>,
    pub passes: Vec<PassEntry>,
    pub loop_running: bool,
    pub task: Probe<TaskInfo>,
    pub clone: Probe<CloneStatus>,
    pub gh_auth: Probe<()>,
    pub github: Probe<GithubData>,
    pub autostart: bool,
    pub refreshed: DateTime<Local>,
    pub slow_refreshed: Option<DateTime<Local>>,
    pub team: super::team::TeamSnapshot,
}

impl Snapshot {
    pub fn empty(root: PathBuf, cli: Option<PathBuf>) -> Self {
        Self {
            root,
            cli,
            config: Err("noch nicht geladen".into()),
            state: Ok(LoopState::default()),
            logs: Vec::new(),
            passes: Vec::new(),
            loop_running: false,
            task: None,
            clone: None,
            gh_auth: None,
            github: None,
            autostart: false,
            refreshed: Local::now(),
            slow_refreshed: None,
            team: Default::default(),
        }
    }

    pub fn config_str(&self, key: &str) -> String {
        self.config.as_ref().map(|c| c.str(key)).unwrap_or_default()
    }

    pub fn clone_path(&self) -> Option<PathBuf> {
        let p = self.config_str("clonePath");
        (!p.is_empty()).then(|| PathBuf::from(p))
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
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
}
