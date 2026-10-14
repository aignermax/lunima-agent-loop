//! `state/po-state.json` — what the Product Owner did last (the Control Center shows it):
//!
//! ```json
//! {"day": "2026-10-14", "runs_today": 1, "last_start": "…", "last_end": "…",
//!  "last_result": "ok", "last_note": "…", "next_due": "…"}
//! ```
//! `last_result`: `ok` | `error` | `timeout` | `usage-limit` | `skipped`.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use team_core::state::write_atomic;

pub const FILE: &str = "po-state.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoState {
    pub day: String,
    pub runs_today: u32,
    pub last_start: Option<String>,
    pub last_end: Option<String>,
    pub last_result: String,
    pub last_note: String,
    pub next_due: Option<String>,
}

pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

fn stamp(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

impl PoState {
    /// Missing or unreadable file → empty state.
    pub fn load(state_dir: &Path) -> Self {
        std::fs::read_to_string(path(state_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, state_dir: &Path) {
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        if let Err(e) = write_atomic(&path(state_dir), text.as_bytes()) {
            crate::log::warn(&format!("PO-Status nicht gespeichert: {e}"));
        }
    }

    /// Starts a new day's counter when the date changed.
    pub fn roll_day(&mut self, now: DateTime<Local>) {
        let today = now.format("%Y-%m-%d").to_string();
        if self.day != today {
            self.day = today;
            self.runs_today = 0;
        }
    }

    pub fn last_start_time(&self) -> Option<DateTime<Local>> {
        self.last_start.as_deref().and_then(team_core::state::parse_time)
    }

    pub fn started(&mut self, now: DateTime<Local>) {
        self.roll_day(now);
        self.runs_today += 1;
        self.last_start = Some(stamp(now));
        self.last_end = None;
        self.last_result = "running".into();
        self.last_note.clear();
    }

    pub fn finished(&mut self, now: DateTime<Local>, result: &str, note: &str, next_due: Option<DateTime<Local>>) {
        self.last_end = Some(stamp(now));
        self.result(result, note, next_due);
    }

    /// Records an outcome without a pass (skipped, waiting for another machine …).
    pub fn result(&mut self, result: &str, note: &str, next_due: Option<DateTime<Local>>) {
        self.last_result = result.into();
        self.last_note = note.chars().take(500).collect();
        self.next_due = next_due.map(stamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn roundtrip_and_day_rollover() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(PoState::load(t.path()), PoState::default());
        let mon = Local.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap();
        let mut s = PoState::default();
        s.started(mon);
        s.started(mon);
        assert_eq!((s.day.as_str(), s.runs_today), ("2026-10-12", 2));
        s.finished(mon, "ok", "fine", Some(mon + chrono::Duration::hours(4)));
        s.save(t.path());
        let back = PoState::load(t.path());
        assert_eq!(back, s);
        assert_eq!(back.last_start_time(), Some(mon));
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path(t.path())).unwrap()).unwrap();
        for key in ["day", "runs_today", "last_start", "last_end", "last_result", "last_note", "next_due"] {
            assert!(json.get(key).is_some(), "contract field {key}");
        }
        let mut next = back;
        next.roll_day(mon + chrono::Duration::days(1));
        assert_eq!(next.runs_today, 0);
    }
}
