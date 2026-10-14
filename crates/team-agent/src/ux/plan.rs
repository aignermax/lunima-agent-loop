//! When the nightly UX review runs and which persona journeys it walks.
//!
//! The run window starts at `ux_review.hour` (local) and lasts `UxReview::WINDOW_HOURS`.
//! A window belongs to the day it started on, so a window crossing midnight is still one
//! run. A locked desktop is retried every `LOCKED_RETRY` inside the window.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Timelike};
use serde_json::Value;
use std::path::Path;
use team_core::config::UxReview;
use team_core::state::UxState;

pub const LOCKED_RETRY: Duration = Duration::minutes(30);
/// Placeholder in goals for a sample layout found in the clone.
pub const SAMPLE_GDS: &str = "{SAMPLE_GDS}";
/// Built-in persona journeys (format of `tools/ux-tester/customer_scenarios.json`).
pub const BUILTIN_GOALS: &str = include_str!("../../assets/ux-goals.json");

pub fn in_window(now: DateTime<Local>, hour: u32) -> bool {
    (now.hour() + 24 - hour % 24) % 24 < UxReview::WINDOW_HOURS
}

/// The day a window belongs to (the date it started on).
pub fn window_day(now: DateTime<Local>, hour: u32) -> NaiveDate {
    (now - Duration::hours((hour % 24) as i64)).date_naive()
}

/// Due: inside the window and this window has no finished attempt yet (a locked desktop
/// is retried after `LOCKED_RETRY`; a run interrupted by a crash is not repeated).
pub fn due(st: &UxState, now: DateTime<Local>, hour: u32) -> bool {
    if !in_window(now, hour) {
        return false;
    }
    if st.day != window_day(now, hour).to_string() {
        return true;
    }
    let last = st.last_attempt.as_deref().and_then(team_core::state::parse_time);
    st.last_result == "skipped-locked" && last.is_none_or(|t| now - t >= LOCKED_RETRY)
}

/// `ux-<yyyymmdd>`: the cross-machine record that this day's review ran.
pub fn lock_key(day: NaiveDate) -> String {
    format!("ux-{}", day.format("%Y%m%d"))
}

/// Goals from `goals_file`, else the built-in ones.
pub fn load_goals(goals_file: &str) -> Result<Vec<Value>, String> {
    let text = if goals_file.trim().is_empty() { BUILTIN_GOALS.to_string() } else { std::fs::read_to_string(goals_file).map_err(|e| format!("{goals_file}: {e}"))? };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("Ziele: {e}"))?;
    let list = v.as_array().cloned().ok_or("Ziele: keine Liste")?;
    if list.iter().any(|g| ["id", "persona", "goal", "success"].iter().any(|k| g.get(*k).and_then(Value::as_str).is_none_or(|s| s.trim().is_empty()))) {
        return Err("Ziele: jedes braucht id, persona, goal, success".into());
    }
    Ok(list)
}

/// Fills `{SAMPLE_GDS}`; goals that need a sample are dropped when there is none.
pub fn substitute(goals: Vec<Value>, sample: Option<&Path>) -> Vec<Value> {
    goals
        .into_iter()
        .filter_map(|g| {
            let text = g.to_string();
            if !text.contains(SAMPLE_GDS) {
                return Some(g);
            }
            let path = sample?.display().to_string();
            let escaped = serde_json::to_string(&path).ok()?;
            serde_json::from_str(&text.replace(SAMPLE_GDS, escaped.trim_matches('"'))).ok()
        })
        .collect()
}

/// `n` journeys, rotating by day so every journey comes up within a few nights.
pub fn pick(goals: &[Value], day: NaiveDate, n: usize) -> Vec<Value> {
    if goals.is_empty() {
        return vec![];
    }
    let n = n.clamp(1, goals.len());
    let start = (day.num_days_from_ce() as usize * n) % goals.len();
    (0..n).map(|i| goals[(start + i) % goals.len()].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(d: u32, h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap()
    }

    #[test]
    fn window_and_window_day() {
        assert!(in_window(at(15, 2, 0), 2) && in_window(at(15, 5, 59), 2));
        assert!(!in_window(at(15, 6, 0), 2) && !in_window(at(15, 1, 59), 2));
        assert!(in_window(at(16, 1, 0), 23), "window crossing midnight");
        assert_eq!(window_day(at(16, 1, 0), 23), at(15, 0, 0).date_naive());
        assert_eq!(lock_key(at(15, 3, 0).date_naive()), "ux-20261015");
    }

    #[test]
    fn due_once_per_window_retrying_a_locked_desktop() {
        let mut st = UxState::default();
        assert!(due(&st, at(15, 2, 10), 2));
        assert!(!due(&st, at(15, 12, 0), 2), "outside the window");
        st.day = "2026-10-15".into();
        st.last_attempt = Some(at(15, 2, 10).to_rfc3339());
        st.last_result = "ok".into();
        assert!(!due(&st, at(15, 3, 0), 2), "done today");
        assert!(due(&st, at(16, 2, 0), 2), "next night");
        st.last_result = "skipped-locked".into();
        assert!(!due(&st, at(15, 2, 20), 2));
        assert!(due(&st, at(15, 2, 41), 2), "locked desktop retried after 30 min");
        st.last_result = "running".into();
        assert!(!due(&st, at(15, 4, 0), 2), "a crashed run is not repeated the same night");
    }

    #[test]
    fn builtin_goals_cover_the_requested_journeys() {
        let g = load_goals("").unwrap();
        let ids: Vec<&str> = g.iter().filter_map(|x| x["id"].as_str()).collect();
        for id in ["newcomer-first-launch", "design-circuit-from-scratch", "gds-import", "simulation-trust", "learn-logic"] {
            assert!(ids.contains(&id), "missing {id}");
        }
    }

    #[test]
    fn sample_placeholder_is_filled_or_the_goal_dropped() {
        let g = load_goals("").unwrap();
        let n = g.len();
        let with = substitute(g.clone(), Some(Path::new("C:\\x\\mzi.gds")));
        assert_eq!(with.len(), n);
        let gds = with.iter().find(|x| x["id"] == "gds-import").unwrap();
        assert!(gds["goal"].as_str().unwrap().contains("C:\\x\\mzi.gds"));
        assert!(!with.iter().any(|x| x.to_string().contains(SAMPLE_GDS)));
        assert_eq!(substitute(g, None).len(), n - 1);
    }

    #[test]
    fn rotation_covers_everything_in_a_few_nights() {
        let g = load_goals("").unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for d in 0..4 {
            let day = at(15, 3, 0).date_naive() + Duration::days(d);
            let picked = pick(&g, day, 3);
            assert_eq!(picked.len(), 3);
            seen.extend(picked.iter().map(|x| x["id"].as_str().unwrap().to_string()));
        }
        assert_eq!(seen.len(), g.len());
        assert_eq!(pick(&g[..2], at(15, 3, 0).date_naive(), 3).len(), 2);
    }
}
