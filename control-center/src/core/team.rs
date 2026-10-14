//! The four roles of the team agent and what each one is doing, from its heartbeat,
//! the pauses in control.json and team.toml.

use super::agent_files::Heartbeat;
use super::health::Level;
use super::snapshot::{relative, Snapshot};
use chrono::{DateTime, Duration, Local};
use team_core::config::TeamConfig;

/// (role key, German name) in pipeline order.
pub const ROLES: [(&str, &str); 4] = [("product-owner", "Product Owner"), ("coder", "Coder"), ("qa", "QA-Tester"), ("pr-feedback", "PR-Feedback")];

/// Longer than the daemon's maximum backoff (900 s): a role backing off during an outage
/// is not mistaken for a dead one.
pub const IDLE_SILENCE_MINUTES: i64 = 20;
/// Builds and test suites run without heartbeat touches.
pub const WORKING_SILENCE_MINUTES: i64 = 60;

pub fn role_label(role: &str) -> &'static str {
    ROLES.iter().find(|(r, _)| *r == role).map(|(_, l)| *l).unwrap_or("Rolle")
}

pub fn role_enabled(cfg: &TeamConfig, role: &str) -> bool {
    match role {
        "coder" => cfg.roles.coder,
        "qa" => cfg.roles.qa,
        "pr-feedback" => cfg.roles.pr_feedback,
        "product-owner" => cfg.roles.product_owner,
        _ => false,
    }
}

/// A heartbeat that hasn't been refreshed for too long.
pub fn stale_since(hb: &Heartbeat, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let limit = if hb.state == "working" { WORKING_SILENCE_MINUTES } else { IDLE_SILENCE_MINUTES };
    hb.updated.filter(|u| now - *u > Duration::minutes(limit))
}

fn working_text(hb: &Heartbeat, now: DateTime<Local>) -> String {
    let what = match (hb.detail.get("issue"), hb.detail.get("pr")) {
        (Some(i), _) => format!("Issue #{i}"),
        (_, Some(p)) => format!("PR #{p}"),
        _ => "eine Aufgabe".into(),
    };
    let repo = hb.detail.get("repo").and_then(|r| r.as_str()).map(|r| format!(" in {}", r.rsplit('/').next().unwrap_or(r))).unwrap_or_default();
    let title = hb.detail.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(|t| format!(" „{t}“")).unwrap_or_default();
    let since = hb.since.map(|s| format!(" · seit {}", relative(s, now).trim_start_matches("vor "))).unwrap_or_default();
    format!("arbeitet an {what}{repo}{title}{since}")
}

/// Status line of a role: (level, text).
pub fn role_status(s: &Snapshot, role: &str, now: DateTime<Local>) -> (Level, String) {
    if s.cfg().is_some_and(|c| !role_enabled(c, role)) {
        return (Level::Info, "abgeschaltet (Einstellungen → Rollen)".into());
    }
    if !s.agent_running {
        return (Level::Error, "Team-Agent läuft nicht".into());
    }
    if let Some(reason) = s.pauses.get(role) {
        return (Level::Warn, format!("pausiert — {reason}"));
    }
    let Some(hb) = s.heartbeats.get(role) else { return (Level::Pending, "noch kein Lebenszeichen".into()) };
    if let Some(u) = stale_since(hb, now) {
        return (Level::Warn, format!("reagiert nicht · letztes Lebenszeichen {}", relative(u, now)));
    }
    match hb.state.as_str() {
        "working" => (Level::Info, working_text(hb, now)),
        "paused" => (Level::Warn, format!("wartet — {}", hb.reason().unwrap_or("pausiert"))),
        _ => (Level::Ok, format!("wartet auf Arbeit · Lebenszeichen {}", hb.updated.map(|u| relative(u, now)).unwrap_or_default())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use team_core::paths::Paths;

    fn snap() -> Snapshot {
        let mut s = Snapshot::empty(Paths::new("x"));
        s.config = Ok(TeamConfig::default());
        s.agent_running = true;
        s
    }

    fn hb(state: &str, minutes_ago: i64) -> Heartbeat {
        let now = Local::now();
        let mut h = Heartbeat { state: state.into(), updated: Some(now - Duration::minutes(minutes_ago)), since: Some(now - Duration::minutes(90)), ..Default::default() };
        h.detail.insert("issue".into(), 7.into());
        h.detail.insert("repo".into(), "o/Lunima".into());
        h.detail.insert("reason".into(), "Claude-Nutzungslimit bis 15:00".into());
        h
    }

    #[test]
    fn status_follows_config_process_pause_and_heartbeat() {
        let now = Local::now();
        let mut s = snap();
        assert_eq!(role_status(&s, "product-owner", now).0, Level::Info, "PO is off by default");
        assert_eq!(role_status(&s, "coder", now).0, Level::Pending);
        s.heartbeats.insert("coder".into(), hb("working", 1));
        let (level, text) = role_status(&s, "coder", now);
        assert_eq!(level, Level::Info);
        assert!(text.contains("Issue #7 in Lunima") && text.contains("seit"), "{text}");
        s.heartbeats.insert("qa".into(), hb("paused", 0));
        assert!(role_status(&s, "qa", now).1.contains("Nutzungslimit"));
        s.heartbeats.insert("pr-feedback".into(), hb("idle", 30));
        assert_eq!(role_status(&s, "pr-feedback", now).0, Level::Warn);
        s.pauses.insert("coder".into(), "Control Center".into());
        assert_eq!(role_status(&s, "coder", now), (Level::Warn, "pausiert — Control Center".into()));
        s.agent_running = false;
        assert_eq!(role_status(&s, "qa", now).0, Level::Error);
    }

    #[test]
    fn working_roles_may_be_quiet_longer() {
        let now = Local::now();
        assert!(stale_since(&hb("working", 40), now).is_none());
        assert!(stale_since(&hb("idle", 40), now).is_some());
        assert_eq!(role_label("qa"), "QA-Tester");
    }
}
