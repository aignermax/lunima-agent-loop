//! When a Product-Owner pass is due, and which machine runs it.
//!
//! Time is cut into slots of `interval_minutes` (counted from the Unix epoch, so every
//! machine computes the same slots). A pass claims its slot by creating the lock ref
//! `refs/team-locks/po-slot-<interval>-<slot>` — atomic on GitHub, so exactly one machine
//! runs per slot. The ref stays as the record of the pass; older slot refs are deleted.

use chrono::{DateTime, Local, TimeZone, Utc};

/// Slot number of `t` for `interval_minutes`.
pub fn slot(t: DateTime<Local>, interval_minutes: u64) -> i64 {
    t.timestamp().div_euclid(60 * interval_minutes.max(1) as i64)
}

/// Start of the slot after `t`'s.
pub fn next_slot_start(t: DateTime<Local>, interval_minutes: u64) -> DateTime<Local> {
    let secs = (slot(t, interval_minutes) + 1) * 60 * interval_minutes.max(1) as i64;
    Utc.timestamp_opt(secs, 0).single().map(|u| u.with_timezone(&Local)).unwrap_or(t)
}

/// Lock key that records "the pass of this slot ran".
pub fn slot_key(interval_minutes: u64, slot: i64) -> String {
    format!("po-slot-{interval_minutes}-{slot}")
}

/// Is a pass due? Not when this machine already ran in the current slot, nor beyond the
/// daily cap (0 = none).
pub fn due(last_start: Option<DateTime<Local>>, runs_today: u32, max_per_day: u32, now: DateTime<Local>, interval_minutes: u64) -> bool {
    if max_per_day > 0 && runs_today >= max_per_day {
        return false;
    }
    last_start.is_none_or(|t| slot(t, interval_minutes) < slot(now, interval_minutes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Utc.with_ymd_and_hms(2026, 10, 14, h, m, 0).unwrap().with_timezone(&Local)
    }

    #[test]
    fn slots_are_the_same_everywhere() {
        assert_eq!(slot(at(8, 0), 240), slot(at(11, 59), 240));
        assert_ne!(slot(at(11, 59), 240), slot(at(12, 0), 240));
        assert_eq!(next_slot_start(at(9, 30), 240), at(12, 0));
        assert_eq!(slot_key(240, 7), "po-slot-240-7");
    }

    #[test]
    fn due_once_per_slot_and_within_the_daily_cap() {
        assert!(due(None, 0, 6, at(9, 0), 240));
        assert!(!due(Some(at(8, 30)), 1, 6, at(9, 0), 240), "same slot");
        assert!(due(Some(at(8, 30)), 1, 6, at(12, 1), 240));
        assert!(!due(Some(at(8, 30)), 6, 6, at(16, 0), 240), "daily cap");
        assert!(due(Some(at(8, 30)), 9, 0, at(16, 0), 240), "0 = no cap");
        assert!(due(Some(at(8, 30) - Duration::days(1)), 0, 6, at(8, 31), 240));
    }
}
