//! When the weekly discovery run is due and how machines agree on who runs it.
//!
//! A week is an ISO week (`2027-w03`). The run may start from the configured weekday and
//! hour on; a machine that was off at that time catches up later in the same week. The lock
//! ref `refs/team-locks/discovery-<week>` makes exactly one machine run a week: it is kept as
//! the record of a finished week and released when a run fails, so another machine (or a
//! later attempt) resumes from the progress committed on the week's branch.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Weekday};
use team_core::config::ProductDiscovery;
use team_core::state::DiscoveryState;

/// Pause between attempts after an error (and between lock checks after "skipped").
pub const RETRY: Duration = Duration::minutes(30);

pub fn week_key(t: DateTime<Local>) -> String {
    let w = t.iso_week();
    format!("{}-w{:02}", w.year(), w.week())
}

pub fn lock_key(week: &str) -> String {
    format!("discovery-{week}")
}

fn weekday(name: &str) -> Weekday {
    name.trim().to_lowercase().parse::<Weekday>().unwrap_or(Weekday::Mon)
}

/// Start of this ISO week's window (local time).
pub fn window_start(t: DateTime<Local>, cfg: &ProductDiscovery) -> DateTime<Local> {
    let w = t.iso_week();
    let day = chrono::NaiveDate::from_isoywd_opt(w.year(), w.week(), weekday(&cfg.weekday)).unwrap_or(t.date_naive());
    let at = day.and_time(NaiveTime::from_hms_opt(cfg.hour.min(23), 0, 0).unwrap_or_default());
    Local.from_local_datetime(&at).earliest().unwrap_or(t)
}

/// The next window start after `t` (for the Control Center's "next run").
pub fn next_window(t: DateTime<Local>, cfg: &ProductDiscovery) -> DateTime<Local> {
    let this = window_start(t, cfg);
    if this > t { this } else { window_start(t + Duration::days(7), cfg) }
}

/// A lock this old belongs to a crashed machine: all three stages at their time limit plus
/// the bookkeeping around them.
pub fn stale_after(cfg: &ProductDiscovery) -> Duration {
    Duration::minutes(3 * cfg.timeout_minutes.max(1) as i64 + 30)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Due {
    /// window not open yet / week finished / given up — nothing to do before this time
    No(DateTime<Local>),
    Run,
}

/// Is a run (or the resumption of an interrupted one) due?
pub fn due(st: &DiscoveryState, now: DateTime<Local>, cfg: &ProductDiscovery) -> Due {
    let start = window_start(now, cfg);
    let next = next_window(now, cfg);
    if now < start {
        return Due::No(start);
    }
    let week = week_key(now);
    if st.week != week {
        return Due::Run;
    }
    let last = st.last_attempt.as_deref().and_then(team_core::state::parse_time);
    let waited = last.is_none_or(|t| now - t >= RETRY);
    match st.last_result.as_str() {
        // finished, or stopped by the budget: the week is over for discovery
        "ok" | "budget" => Due::No(next),
        // attempts count every start, so a run that keeps crashing also stops
        _ if st.attempts >= cfg.max_attempts.max(1) => Due::No(next),
        "error" | "skipped" if !waited => Due::No(last.map(|t| t + RETRY).unwrap_or(now)),
        // "running" after a crash or restart resumes right away
        _ => Due::Run,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    fn cfg() -> ProductDiscovery {
        ProductDiscovery { weekday: "mon".into(), hour: 6, max_attempts: 3, ..Default::default() }
    }

    #[test]
    fn iso_weeks_and_windows() {
        assert_eq!(week_key(at(2027, 1, 20, 9)), "2027-w03");
        assert_eq!(week_key(at(2027, 1, 1, 9)), "2026-w53", "ISO year differs from calendar year");
        assert_eq!(window_start(at(2027, 1, 20, 9), &cfg()), at(2027, 1, 18, 6));
        assert_eq!(next_window(at(2027, 1, 18, 5), &cfg()), at(2027, 1, 18, 6));
        assert_eq!(next_window(at(2027, 1, 18, 7), &cfg()), at(2027, 1, 25, 6));
        assert_eq!(lock_key("2027-w03"), "discovery-2027-w03");
    }

    #[test]
    fn due_once_a_week_with_catch_up_and_bounded_retries() {
        let c = cfg();
        let st = |week: &str, result: &str, attempts: u32, last: DateTime<Local>| DiscoveryState { week: week.into(), last_result: result.into(), attempts, last_attempt: Some(last.to_rfc3339()), ..Default::default() };
        assert_eq!(due(&DiscoveryState::default(), at(2027, 1, 18, 5), &c), Due::No(at(2027, 1, 18, 6)), "before the window");
        assert_eq!(due(&DiscoveryState::default(), at(2027, 1, 18, 6), &c), Due::Run);
        assert_eq!(due(&DiscoveryState::default(), at(2027, 1, 21, 15), &c), Due::Run, "machine was off on Monday: catch up");
        let mon = at(2027, 1, 18, 6);
        assert_eq!(due(&st("2027-w03", "ok", 0, mon), at(2027, 1, 19, 6), &c), Due::No(at(2027, 1, 25, 6)));
        assert_eq!(due(&st("2027-w03", "budget", 0, mon), at(2027, 1, 19, 6), &c), Due::No(at(2027, 1, 25, 6)));
        assert_eq!(due(&st("2027-w03", "error", 1, mon), mon + Duration::minutes(10), &c), Due::No(mon + RETRY));
        assert_eq!(due(&st("2027-w03", "error", 1, mon), mon + Duration::minutes(31), &c), Due::Run);
        assert_eq!(due(&st("2027-w03", "error", 3, mon), mon + Duration::hours(5), &c), Due::No(at(2027, 1, 25, 6)), "given up for the week");
        assert_eq!(due(&st("2027-w03", "running", 1, mon), mon + Duration::minutes(1), &c), Due::Run, "resume after a crash");
        assert_eq!(due(&st("2027-w03", "running", 3, mon), mon + Duration::minutes(1), &c), Due::No(at(2027, 1, 25, 6)), "a run that keeps crashing stops too");
        assert_eq!(due(&st("2027-w02", "ok", 0, mon - Duration::days(7)), mon, &c), Due::Run, "new week");
    }
}
