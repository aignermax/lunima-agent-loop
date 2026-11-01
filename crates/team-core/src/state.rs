//! State files shared with the Control Center — same formats the Python agent used, so the
//! UI keeps working unchanged:
//!
//! * `control.json` — `{"paused": {"<role>": {"until": iso|null, "reason": str}}}` (UI writes)
//! * `status-<role>.json` — heartbeat: `{role, pid, state, detail, since, updated}` (agent writes)
//! * `issue-history.jsonl` — one record per coder run

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const CONTROL_FILE: &str = "control.json";
pub const HISTORY_FILE: &str = "issue-history.jsonl";

/// RFC 3339, or naive local `YYYY-MM-DD[T ]HH:MM:SS[.fff]` (the Control Center writes both).
/// Ambiguous local times (DST fall-back) take the earlier instant, times in the spring-forward
/// gap the hour after.
pub fn parse_time(raw: &str) -> Option<DateTime<Local>> {
    let raw = raw.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Some(t.with_timezone(&Local));
    }
    let trimmed = raw.split('.').next().unwrap_or(raw);
    let naive = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"].iter().find_map(|f| NaiveDateTime::parse_from_str(trimmed, f).ok())?;
    Local.from_local_datetime(&naive).earliest().or_else(|| Local.from_local_datetime(&(naive + chrono::Duration::hours(1))).earliest())
}

/// The pause reason when `role` is paused right now. Never fails: a malformed file means
/// "not paused" (the UI may be writing it this very moment); an unreadable `until` means
/// paused without end.
pub fn pause_reason(state_dir: &Path, role: &str, now: DateTime<Local>) -> Option<String> {
    let text = std::fs::read_to_string(state_dir.join(CONTROL_FILE)).ok()?;
    let doc: Value = serde_json::from_str(&text).ok()?;
    let entry = doc.get("paused")?.get(role)?.as_object()?;
    if let Some(until) = entry.get("until").and_then(Value::as_str).filter(|u| !u.is_empty()) {
        if let Some(end) = parse_time(until) {
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

pub const PO_STATE_FILE: &str = "po-state.json";

/// The product owner's progress (`state/po-state.json`): the role writes it, the Control
/// Center shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoState {
    /// "YYYY-MM-DD" the counter belongs to
    pub day: String,
    pub runs_today: u32,
    /// RFC 3339
    pub last_start: Option<String>,
    pub last_end: Option<String>,
    /// "ok" | "error" | "timeout" | "usage-limit" | "skipped"
    pub last_result: String,
    pub last_note: String,
    pub next_due: Option<String>,
}

impl PoState {
    /// Missing or unreadable file → `None`.
    pub fn load(state_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(state_dir.join(PO_STATE_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        write_atomic(&state_dir.join(PO_STATE_FILE), serde_json::to_string_pretty(self).unwrap_or_default().as_bytes())
    }
}

pub const UX_STATE_FILE: &str = "ux-state.json";

/// `state/ux-state.json` — the last nightly UX review (the Control Center and the Product
/// Owner read it).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UxState {
    /// window day ("YYYY-MM-DD") of the last attempt
    pub day: String,
    pub last_attempt: Option<String>,
    pub last_start: Option<String>,
    pub last_end: Option<String>,
    /// "running" | "ok" | "error" | "timeout" | "skipped" | "skipped-locked"
    pub last_result: String,
    pub last_note: String,
    /// folder with report.md / findings.json of the last finished run
    pub report_dir: Option<String>,
    pub filed_issues: Vec<u64>,
    /// ids of the persona journeys of the last run
    pub goals: Vec<String>,
    /// attempts in the current window (errors are retried once)
    pub attempts: u32,
    /// commit the app was last built from (unchanged → no rebuild)
    pub built_sha: String,
    /// the last successful run — a failing later run never hides it
    pub last_ok_end: Option<String>,
    pub last_ok_report: Option<String>,
}

impl UxState {
    pub fn load(state_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(state_dir.join(UX_STATE_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        write_atomic(&state_dir.join(UX_STATE_FILE), serde_json::to_string_pretty(self).unwrap_or_default().as_bytes())
    }
}


/// `state/discovery/` — the product-discovery role's files (the Control Center reads them).
pub const DISCOVERY_DIR: &str = "discovery";
pub const DISCOVERY_STATE_FILE: &str = "state.json";
pub const DISCOVERY_LEDGER_FILE: &str = "ledger.jsonl";
pub const DISCOVERY_REPORT_FILE: &str = "report.md";

/// One item the Control Center lists (opportunity, approval, Decision Record …).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryItem {
    /// "O-…", "DR-0003", issue number as text …
    pub id: String,
    pub title: String,
    /// open | parked | proposed | accepted | rejected | pending | approved | declined …
    pub status: String,
    pub url: String,
}

/// `state/discovery/state.json`: progress of the current week and what the last run left
/// for the maintainer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryState {
    /// ISO week of the run, e.g. "2027-w03"
    pub week: String,
    /// stage running right now ("" = none): scout | strategist | critic | apply
    pub stage: String,
    pub stages_done: Vec<String>,
    /// runs of this week that ended in an error
    pub attempts: u32,
    pub last_attempt: Option<String>,
    pub last_start: Option<String>,
    pub last_end: Option<String>,
    /// running | ok | error | budget | paused | skipped | usage-limit
    pub last_result: String,
    pub last_note: String,
    pub next_due: Option<String>,
    pub pr_url: String,
    pub report_url: String,
    pub week_cost_usd: f64,
    pub week_tokens: u64,
    pub evidence_added: usize,
    pub duplicates_dropped: usize,
    pub issues_created: usize,
    pub opportunities: Vec<DiscoveryItem>,
    pub approvals: Vec<DiscoveryItem>,
    pub decisions: Vec<DiscoveryItem>,
}

impl DiscoveryState {
    pub fn dir(state_dir: &Path) -> std::path::PathBuf {
        state_dir.join(DISCOVERY_DIR)
    }

    pub fn load(state_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::dir(state_dir).join(DISCOVERY_STATE_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        write_atomic(&Self::dir(state_dir).join(DISCOVERY_STATE_FILE), serde_json::to_string_pretty(self).unwrap_or_default().as_bytes())
    }
}

/// One Claude stage run in `state/discovery/ledger.jsonl`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerRecord {
    pub week: String,
    /// scout | strategist | critic (a repair run is "<stage>-repair")
    pub stage: String,
    pub machine: String,
    pub model: String,
    /// RFC 3339
    pub started: String,
    pub duration_sec: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub turns: u32,
    /// the CLI's own figure
    pub cost_usd: f64,
    /// "subscription-notional": no money was charged; "api": billed per token
    pub billing: String,
    /// ok | error | timeout | usage-limit | invalid
    pub result: String,
}

pub fn read_ledger(state_dir: &Path) -> Vec<LedgerRecord> {
    std::fs::read_to_string(DiscoveryState::dir(state_dir).join(DISCOVERY_LEDGER_FILE))
        .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

pub fn append_ledger(state_dir: &Path, record: &LedgerRecord) -> std::io::Result<()> {
    let dir = DiscoveryState::dir(state_dir);
    std::fs::create_dir_all(&dir)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(DISCOVERY_LEDGER_FILE))?;
    writeln!(f, "{}", serde_json::to_string(record).unwrap_or_default())
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
