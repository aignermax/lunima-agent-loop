//! The autonomous issue agent (Python, runs in WSL) as seen from its folder on the Windows
//! drive: role heartbeats, per-role pause, issue history, .env and recent log problems.
//! File contract: autonomous-issue-agent/src/control.py.

use super::state::parse_time;
use chrono::{DateTime, Local};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const CONTROL_FILE: &str = "control.json";
const SETTINGS_FILE: &str = "control-center.json";
const LOG_TAIL_BYTES: u64 = 768 * 1024;

/// What a role last reported in `.sessions/status-<role>.json`.
#[derive(Debug, Clone, Default)]
pub struct Heartbeat {
    pub state: String,
    pub detail: Map<String, Value>,
    pub since: Option<DateTime<Local>>,
    pub updated: Option<DateTime<Local>>,
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
}

/// Problems found at the end of agent.log (all roles log there).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogFindings {
    /// Last "this CLI does not support the model" error.
    pub model_unsupported: Option<DateTime<Local>>,
    /// Last "Not logged in" / authentication error.
    pub not_logged_in: Option<DateTime<Local>>,
    /// Claude failures in the scanned tail, with the time of the newest.
    pub claude_failures: Vec<DateTime<Local>>,
    /// Highest "PR #n has k QA failures" seen: (pr, k, when).
    pub qa_loop: Option<(u64, u64, DateTime<Local>)>,
    pub log_bytes: u64,
}

/// The issue agent's folder: explicit setting in `<loop root>/control-center.json`, else
/// the usual places next to the loop or in the user profile.
pub fn resolve_dir(loop_root: &Path) -> Option<PathBuf> {
    let configured = std::fs::read_to_string(loop_root.join(SETTINGS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("issueAgentDir").and_then(Value::as_str).map(PathBuf::from));
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let candidates = [configured, loop_root.parent().map(|p| p.join("autonomous-issue-agent")), home.map(|h| h.join("autonomous-issue-agent"))];
    candidates.into_iter().flatten().find(|d| d.join("main.py").is_file() && d.join("src").is_dir())
}

/// Remembers an explicit issue-agent folder for this loop root.
pub fn save_dir_setting(loop_root: &Path, dir: &Path) -> Result<(), String> {
    let path = loop_root.join(SETTINGS_FILE);
    let mut obj = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Map<String, Value>>(&t).ok()).unwrap_or_default();
    obj.insert("issueAgentDir".into(), Value::String(dir.display().to_string()));
    std::fs::write(&path, serde_json::to_string_pretty(&obj).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())
}

pub fn sessions(dir: &Path) -> PathBuf {
    dir.join(".sessions")
}

pub fn read_heartbeats(dir: &Path) -> BTreeMap<String, Heartbeat> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(sessions(dir)) else { return out };
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
pub fn read_history(dir: &Path, limit: usize) -> Vec<HistoryEntry> {
    let text = std::fs::read_to_string(sessions(dir).join("issue-history.jsonl")).unwrap_or_default();
    text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(limit).collect()
}

/// Roles paused in control.json (role → reason), ignoring elapsed pauses.
pub fn read_pauses(dir: &Path, now: DateTime<Local>) -> BTreeMap<String, String> {
    let doc = read_control(dir);
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

fn read_control(dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(sessions(dir).join(CONTROL_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Pauses (`Some(reason)`) or resumes (`None`) one role; other keys are kept.
pub fn set_pause(dir: &Path, role: &str, reason: Option<&str>) -> Result<(), String> {
    let mut doc = read_control(dir);
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
    let path = sessions(dir).join(CONTROL_FILE);
    std::fs::create_dir_all(sessions(dir)).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn tail_text(path: &Path, bytes: u64) -> (String, u64) {
    let Ok(mut f) = std::fs::File::open(path) else { return (String::new(), 0) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(bytes)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    (String::from_utf8_lossy(&buf).into_owned(), len)
}

fn line_time(line: &str) -> Option<DateTime<Local>> {
    line.get(..19).and_then(parse_time)
}

/// Scans the end of agent.log for the failure patterns that silently stalled the team before.
pub fn scan_log(dir: &Path) -> LogFindings {
    let (text, len) = tail_text(&dir.join("agent.log"), LOG_TAIL_BYTES);
    let mut f = LogFindings { log_bytes: len, ..Default::default() };
    let qa_re = regex::Regex::new(r"PR #(\d+) has (\d+) QA failures").expect("static regex");
    for line in text.lines() {
        let Some(t) = line_time(line) else { continue };
        if line.contains("does not support this model") {
            f.model_unsupported = Some(t);
        }
        if line.contains("Not logged in") || line.contains("authentication_failed") {
            f.not_logged_in = Some(t);
        }
        if line.contains("Claude Code failed") || line.contains("Claude execution failed") {
            f.claude_failures.push(t);
        }
        if let Some(c) = qa_re.captures(line) {
            let (pr, n) = (c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(0));
            if f.qa_loop.is_none_or(|(_, k, _)| n >= k) {
                f.qa_loop = Some((pr, n, t));
            }
        }
    }
    f
}

#[cfg(test)]
#[path = "issue_agent_tests.rs"]
mod tests;
