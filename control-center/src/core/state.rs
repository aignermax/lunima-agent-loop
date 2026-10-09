//! state/state.json as written by the C# loop (PascalCase): pause, day counters, run history.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DayCounters {
    pub tasks: u32,
    pub owner_runs: u32,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RunRecord {
    pub timestamp: String,
    pub kind: String,
    pub issue: Option<u32>,
    pub branch: Option<String>,
    pub exit_code: i32,
    pub duration_sec: f64,
    pub note: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LoopState {
    pub days: BTreeMap<String, DayCounters>,
    pub last_runs: Vec<RunRecord>,
    pub last_owner_run: Option<String>,
    pub paused_until: Option<String>,
    pub pause_reason: Option<String>,
}

/// Pause as the UI needs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Pause {
    None,
    Until(DateTime<Local>),
    Indefinitely,
}

impl LoopState {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("state.json: {e}"))
    }

    pub fn pause(&self, now: DateTime<Local>) -> Pause {
        let Some(raw) = self.paused_until.as_deref() else { return Pause::None };
        if raw.starts_with("9999-") {
            return Pause::Indefinitely;
        }
        match parse_time(raw) {
            Some(t) if t > now => Pause::Until(t),
            Some(_) => Pause::None,
            None => Pause::Indefinitely,
        }
    }

    pub fn today(&self, now: DateTime<Local>) -> DayCounters {
        self.days.get(&now.format("%Y-%m-%d").to_string()).cloned().unwrap_or_default()
    }

    pub fn last_owner_run(&self) -> Option<DateTime<Local>> {
        self.last_owner_run.as_deref().and_then(parse_time)
    }

    /// Newest first.
    pub fn recent_runs(&self, n: usize) -> Vec<RunRecord> {
        self.last_runs.iter().rev().take(n).cloned().collect()
    }

    /// Consecutive failed runs of `kind` at the end of the history.
    pub fn failure_streak(&self, kind: &str) -> Vec<RunRecord> {
        self.last_runs
            .iter()
            .rev()
            .filter(|r| r.kind == kind)
            .take_while(|r| r.exit_code != 0)
            .cloned()
            .collect()
    }
}

/// Parses the loop's timestamps: ISO with offset, ISO without offset, or "yyyy-MM-dd HH:mm:ss" (local).
pub fn parse_time(raw: &str) -> Option<DateTime<Local>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Some(t.with_timezone(&Local));
    }
    let trimmed = raw.split('.').next().unwrap_or(raw);
    ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"]
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(trimmed, fmt).ok())
        .and_then(|n| Local.from_local_datetime(&n).single())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "Days": {"2026-10-07": {"Tasks": 3, "OwnerRuns": 1}},
      "LastRuns": [
        {"Timestamp": "2026-10-07 04:13:05", "Kind": "owner", "Issue": null, "Branch": null, "ExitCode": 1, "DurationSec": 6, "Note": null},
        {"Timestamp": "2026-10-07 05:12:58", "Kind": "task", "Issue": 1422, "Branch": "agent/x", "ExitCode": 1, "DurationSec": 0, "Note": "branch setup failed"},
        {"Timestamp": "2026-10-07 06:12:58", "Kind": "task", "Issue": 1422, "Branch": "agent/y", "ExitCode": 1, "DurationSec": 0, "Note": "branch setup failed"}
      ],
      "LastOwnerRun": "2026-10-07T04:12:56.123+02:00",
      "PausedUntil": "9999-12-31T23:59:59.9999999",
      "PauseReason": "switch"
    }"#;

    fn sample() -> LoopState {
        serde_json::from_str(SAMPLE).unwrap()
    }

    #[test]
    fn parses_loop_state_written_by_csharp() {
        let s = sample();
        assert_eq!(s.last_runs.len(), 3);
        assert_eq!(s.last_runs[1].issue, Some(1422));
        assert_eq!(s.pause(Local::now()), Pause::Indefinitely);
        assert!(s.last_owner_run().is_some());
    }

    #[test]
    fn failure_streak_counts_trailing_failures_per_kind() {
        let s = sample();
        assert_eq!(s.failure_streak("task").len(), 2);
        assert_eq!(s.failure_streak("owner").len(), 1);
    }

    #[test]
    fn elapsed_pause_is_not_a_pause() {
        let mut s = sample();
        s.paused_until = Some("2020-01-01T00:00:00+02:00".into());
        assert_eq!(s.pause(Local::now()), Pause::None);
    }

    #[test]
    fn parses_all_timestamp_styles() {
        assert!(parse_time("2026-10-07 05:12:58").is_some());
        assert!(parse_time("2026-10-09T08:30:43.8986204+02:00").is_some());
        assert!(parse_time("2026-10-09T08:30:43.8986204").is_some());
        assert!(parse_time("gestern").is_none());
    }
}
