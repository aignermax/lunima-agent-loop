//! The Windows scheduled task that fires `lunima-agent-loop run` every hour.

use crate::core::state::parse_time;
use chrono::{DateTime, Local};
use serde::Deserialize;

pub const TASK_NAME: &str = "LunimaAgentLoop";

#[derive(Debug, Clone, Default)]
pub struct TaskInfo {
    pub exists: bool,
    /// Ready / Running / Disabled / Queued
    pub state: String,
    pub last_run: Option<DateTime<Local>>,
    pub next_run: Option<DateTime<Local>>,
    pub last_result: Option<i64>,
}

#[derive(Deserialize)]
struct Raw {
    exists: bool,
    state: Option<String>,
    #[serde(rename = "lastRun")]
    last_run: Option<String>,
    #[serde(rename = "nextRun")]
    next_run: Option<String>,
    #[serde(rename = "lastResult")]
    last_result: Option<i64>,
}

const QUERY: &str = r#"
$t = Get-ScheduledTask -TaskName 'LunimaAgentLoop' -ErrorAction SilentlyContinue
if (-not $t) { '{"exists":false}'; exit }
$i = $t | Get-ScheduledTaskInfo
[pscustomobject]@{
  exists = $true
  state = [string]$t.State
  lastRun = if ($i.LastRunTime -and $i.LastRunTime.Year -gt 2000) { $i.LastRunTime.ToString('o') } else { $null }
  nextRun = if ($i.NextRunTime) { $i.NextRunTime.ToString('o') } else { $null }
  lastResult = [int64]$i.LastTaskResult
} | ConvertTo-Json -Compress
"#;

pub fn parse(json: &str) -> Result<TaskInfo, String> {
    let raw: Raw = serde_json::from_str(json.trim()).map_err(|e| format!("Task-Abfrage: {e}"))?;
    Ok(TaskInfo {
        exists: raw.exists,
        state: raw.state.unwrap_or_default(),
        last_run: raw.last_run.as_deref().and_then(parse_time),
        next_run: raw.next_run.as_deref().and_then(parse_time),
        last_result: raw.last_result,
    })
}

pub fn query() -> Result<TaskInfo, String> {
    if !cfg!(windows) {
        return Ok(TaskInfo::default());
    }
    parse(&super::powershell(QUERY)?)
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let verb = if enabled { "Enable" } else { "Disable" };
    super::powershell(&format!("{verb}-ScheduledTask -TaskName '{TASK_NAME}' | Out-Null")).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_existing_task() {
        let t = parse(r#"{"exists":true,"state":"Ready","lastRun":"2026-10-09T08:12:54.0000000+02:00","nextRun":"2026-10-09T09:12:53.0000000+02:00","lastResult":0}"#).unwrap();
        assert!(t.exists);
        assert_eq!(t.state, "Ready");
        assert!(t.next_run.unwrap() > t.last_run.unwrap());
    }

    #[test]
    fn parses_missing_task() {
        let t = parse(r#"{"exists":false}"#).unwrap();
        assert!(!t.exists);
        assert!(t.next_run.is_none());
    }
}
