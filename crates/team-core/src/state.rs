//! State files shared with the Control Center — same formats the Python agent used, so the
//! UI keeps working unchanged:
//!
//! * `control.json` — `{"paused": {"<role>": {"until": iso|null, "reason": str}}}` (UI writes)
//! * `status-<role>.json` — heartbeat: `{role, pid, state, detail, since, updated}` (agent writes)
//! * `issue-history.jsonl` — one record per coder run

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const CONTROL_FILE: &str = "control.json";
pub const HISTORY_FILE: &str = "issue-history.jsonl";

/// The pause reason when `role` is paused right now. Never fails: a malformed file means
/// "not paused" (the UI may be writing it this very moment).
pub fn pause_reason(state_dir: &Path, role: &str, now: DateTime<Local>) -> Option<String> {
    let text = std::fs::read_to_string(state_dir.join(CONTROL_FILE)).ok()?;
    let doc: Value = serde_json::from_str(&text).ok()?;
    let entry = doc.get("paused")?.get(role)?.as_object()?;
    if let Some(until) = entry.get("until").and_then(Value::as_str).filter(|u| !u.is_empty()) {
        let end = DateTime::parse_from_rfc3339(until)
            .map(|t| t.with_timezone(&Local))
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(until, "%Y-%m-%dT%H:%M:%S").map(|n| n.and_local_timezone(Local).unwrap()));
        if let Ok(end) = end {
            if end <= now {
                return None;
            }
        }
    }
    Some(entry.get("reason").and_then(Value::as_str).filter(|r| !r.is_empty()).unwrap_or("pausiert").to_string())
}

/// Heartbeat writer for one role (`status-<role>.json`), written atomically.
pub struct Heartbeat {
    path: PathBuf,
    role: String,
    last: Mutex<Option<(String, String, String)>>, // (state, detail json, since)
}

impl Heartbeat {
    pub fn new(state_dir: &Path, role: &str) -> Self {
        Self { path: state_dir.join(format!("status-{role}.json")), role: role.to_string(), last: Mutex::new(None) }
    }

    /// Reports a state; `since` only changes when state or detail change.
    pub fn report(&self, state: &str, detail: Map<String, Value>) {
        let now = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let detail_json = Value::Object(detail.clone()).to_string();
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let since = match last.as_ref() {
            Some((s, d, since)) if s == state && *d == detail_json => since.clone(),
            _ => now.clone(),
        };
        *last = Some((state.to_string(), detail_json, since.clone()));
        drop(last);
        let payload = json!({"role": self.role, "pid": std::process::id(), "state": state, "detail": detail, "since": since, "updated": now});
        let _ = write_atomic(&self.path, payload.to_string().as_bytes());
    }

    /// Re-writes the current state with a fresh `updated` (long work stays visibly alive).
    pub fn touch(&self) {
        let current = self.last.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some((state, detail, _)) = current {
            let detail = serde_json::from_str::<Map<String, Value>>(&detail).unwrap_or_default();
            self.report(&state, detail);
        }
    }

    pub fn idle(&self) {
        self.report("idle", Map::new());
    }
}

/// Helper for `detail` maps: `detail!["issue" => 7, "repo" => "o/r"]`.
#[macro_export]
macro_rules! detail {
    ($($k:expr => $v:expr),* $(,)?) => {{
        let mut m = serde_json::Map::new();
        $( m.insert($k.to_string(), serde_json::json!($v)); )*
        m
    }};
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// One coder run, as the Control Center's Coder tab expects it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub number: u64,
    pub title: String,
    pub repository: String,
    pub completed: bool,
    pub pr_url: Option<String>,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub session_count: u32,
    /// "YYYY-MM-DD HH:MM:SS" local
    pub timestamp: String,
    pub duration_sec: Option<i64>,
    /// machine that did the run (new; absent in Python-era records)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

pub fn append_history(state_dir: &Path, record: &HistoryRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(state_dir.join(HISTORY_FILE))?;
    writeln!(f, "{}", serde_json::to_string(record).unwrap_or_default())
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
