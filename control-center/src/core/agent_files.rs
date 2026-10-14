//! The team agent's state folder as the UI sees it: role heartbeats, per-role pauses,
//! coder history, "is the daemon running" and recent problems in agent.log.
//! File contract: docs/TEAM-AGENT.md (same formats the Python agent used).

use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use team_core::state::{parse_time, write_atomic, CONTROL_FILE, HISTORY_FILE};

const LOG_TAIL_BYTES: u64 = 768 * 1024;
const PID_FILE: &str = "agent.pid";

/// What a role last reported in `state/status-<role>.json`.
#[derive(Debug, Clone, Default)]
pub struct Heartbeat {
    /// idle | working | paused
    pub state: String,
    pub detail: Map<String, Value>,
    pub since: Option<DateTime<Local>>,
    pub updated: Option<DateTime<Local>>,
}

impl Heartbeat {
    /// `detail.reason` of a paused heartbeat (usage limit, switched off, control.json).
    pub fn reason(&self) -> Option<&str> {
        self.detail.get("reason").and_then(Value::as_str)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryEntry {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub completed: bool,
    pub pr_url: Option<String>,
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub timestamp: String,
    pub duration_sec: Option<i64>,
    #[serde(default)]
    pub machine: Option<String>,
}

/// Problems found at the end of agent.log (all roles log there).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogFindings {
    /// Last "not logged in" / authentication error of Claude.
    pub not_logged_in: Option<DateTime<Local>>,
    /// Claude sessions that crashed or hung.
    pub claude_failures: Vec<DateTime<Local>>,
    /// Panics caught by the daemon.
    pub panics: Vec<DateTime<Local>>,
    /// Newest `[ERROR]` line: (time, message without timestamp).
    pub last_error: Option<(DateTime<Local>, String)>,
    pub log_bytes: u64,
}

impl LogFindings {
    pub fn count_since(times: &[DateTime<Local>], now: DateTime<Local>, hours: i64) -> usize {
        times.iter().filter(|t| now - **t < Duration::hours(hours)).count()
    }
}

pub fn read_heartbeats(state: &Path) -> BTreeMap<String, Heartbeat> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(state) else { return out };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(role) = name.strip_prefix("status-").and_then(|r| r.strip_suffix(".json")) else { continue };
        let Some(v) = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let time = |k: &str| v.get(k).and_then(Value::as_str).and_then(parse_time);
        out.insert(role.to_string(), Heartbeat {
            state: v.get("state").and_then(Value::as_str).unwrap_or_default().to_string(),
            detail: v.get("detail").and_then(Value::as_object).cloned().unwrap_or_default(),
            since: time("since"),
            updated: time("updated"),
        });
    }
    out
}

/// Newest first; corrupt lines are skipped.
pub fn read_history(state: &Path, limit: usize) -> Vec<HistoryEntry> {
    let text = std::fs::read_to_string(state.join(HISTORY_FILE)).unwrap_or_default();
    text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(limit).collect()
}

/// Repos the agent currently works on via discovery: registry entries whose `last_seen`
/// is within the expiry window (older ones are ignored by the agent too).
pub fn read_discovered(state: &Path, expiry_days: i64, now: NaiveDateTime) -> Vec<String> {
    let Some(map) = std::fs::read_to_string(state.join("discovered-repos.json")).ok().and_then(|t| serde_json::from_str::<Map<String, Value>>(&t).ok()) else {
        return Vec::new();
    };
    let cutoff = now - Duration::days(expiry_days);
    map.into_iter()
        .filter(|(_, v)| v.get("last_seen").and_then(Value::as_str).and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()).is_some_and(|t| t >= cutoff))
        .map(|(k, _)| k)
        .collect()
}

/// Roles paused in control.json (role → reason), ignoring elapsed pauses.
pub fn read_pauses(state: &Path, now: DateTime<Local>) -> BTreeMap<String, String> {
    let doc = read_control(state);
    let Some(paused) = doc.get("paused").and_then(Value::as_object) else { return BTreeMap::new() };
    paused
        .iter()
        .filter_map(|(role, entry)| {
            let until = entry.get("until").and_then(Value::as_str).and_then(parse_time);
            if until.is_some_and(|u| u <= now) {
                return None;
            }
            let reason = entry.get("reason").and_then(Value::as_str).unwrap_or("pausiert").to_string();
            Some((role.clone(), reason))
        })
        .collect()
}

fn read_control(state: &Path) -> Map<String, Value> {
    std::fs::read_to_string(state.join(CONTROL_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Pauses (`Some(reason)`) or resumes (`None`) one role; other keys are kept.
pub fn set_pause(state: &Path, role: &str, reason: Option<&str>) -> Result<(), String> {
    let mut doc = read_control(state);
    let paused = doc.entry("paused").or_insert_with(|| Value::Object(Map::new()));
    let Some(map) = paused.as_object_mut() else { return Err("control.json: 'paused' ist kein Objekt".into()) };
    match reason {
        Some(r) => {
            map.insert(role.to_string(), serde_json::json!({ "until": null, "reason": r }));
        }
        None => {
            map.remove(role);
        }
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    write_atomic(&state.join(CONTROL_FILE), text.as_bytes()).map_err(|e| e.to_string())
}

/// The daemon writes its pid to `state/agent.pid` after taking `agent.lock`. Probing the
/// lock itself would briefly hold it and make a just-starting daemon exit.
pub fn agent_pid(state: &Path) -> Option<u32> {
    std::fs::read_to_string(state.join(PID_FILE)).ok()?.trim().parse().ok()
}

pub fn agent_running(state: &Path) -> bool {
    agent_pid(state).is_some_and(crate::sys::process::is_agent)
}

fn tail_text(path: &Path, bytes: u64) -> (String, u64) {
    let Ok(mut f) = std::fs::File::open(path) else { return (String::new(), 0) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(bytes)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    (String::from_utf8_lossy(&buf).into_owned(), len)
}

/// agent.log lines start with local "YYYY-MM-DD HH:MM:SS".
fn line_time(line: &str) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(line.get(..19)?, "%Y-%m-%d %H:%M:%S").ok()?;
    Local.from_local_datetime(&naive).earliest()
}

/// Scans the end of agent.log (and agent.log.1 right after a rotation) for the failure
/// patterns that need a human: Claude logged out, crashing sessions, panics.
pub fn scan_log(logs: &Path) -> LogFindings {
    let (current, len) = tail_text(&logs.join("agent.log"), LOG_TAIL_BYTES);
    let text = if len < LOG_TAIL_BYTES {
        let (previous, _) = tail_text(&logs.join("agent.log.1"), LOG_TAIL_BYTES - len);
        previous + &current
    } else {
        current
    };
    let mut f = LogFindings { log_bytes: len, ..Default::default() };
    for line in text.lines() {
        let Some(t) = line_time(line) else { continue };
        let lower = line.to_lowercase();
        if lower.contains("not logged in") || lower.contains("authentication_failed") || lower.contains("please run /login") {
            f.not_logged_in = Some(t);
        }
        if line.contains("Claude beendet ohne Ergebnis") || line.contains("Claude hing") || line.contains("Claude konnte nicht gestartet") {
            f.claude_failures.push(t);
        }
        if line.contains("(panic)") {
            f.panics.push(t);
        }
        if line.contains("[ERROR]") {
            f.last_error = Some((t, line.get(20..).unwrap_or("").to_string()));
        }
    }
    f
}

#[cfg(test)]
#[path = "agent_files_tests.rs"]
mod tests;
