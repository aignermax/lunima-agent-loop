//! The whole team: the PO loop (Windows) plus the issue agent's roles (WSL). Health checks
//! here cover the issue agent — each one is a failure that once stalled it for days.

use super::envfile::EnvFile;
use super::health::{Check, Fix, Level};
use super::issue_agent::{Heartbeat, HistoryEntry, LogFindings};
use super::snapshot::{relative, Probe, Snapshot};
use crate::sys::wsl::UNITS;
use chrono::{DateTime, Duration, Local};
use std::collections::BTreeMap;
use std::path::PathBuf;

const LOG_WARN_BYTES: u64 = 200 * 1024 * 1024;
const QA_LOOP_ROUNDS: u64 = 5;
/// Longer than the agent's maximum poll backoff (900 s), so a role that is backing off
/// during an API outage is not mistaken for a dead one.
const IDLE_SILENCE_MINUTES: i64 = 20;

/// Everything known about the issue agent.
#[derive(Clone, Default)]
pub struct TeamSnapshot {
    pub dir: Option<PathBuf>,
    pub heartbeats: BTreeMap<String, Heartbeat>,
    pub pauses: BTreeMap<String, String>,
    pub history: Vec<HistoryEntry>,
    /// Repos found by the agent's org discovery.
    pub discovered: Vec<String>,
    /// repo → roles enabled by its .agent.toml (Err: not readable / no file)
    pub agent_tomls: BTreeMap<String, Result<Vec<String>, String>>,
    pub env: Option<EnvFile>,
    pub units: Probe<BTreeMap<String, String>>,
    pub claude_version: Probe<String>,
    /// WSL's time zone (agent.log timestamps are WSL local time).
    pub wsl_offset: Option<chrono::FixedOffset>,
    pub log: Option<LogFindings>,
}

/// German display name of an issue-agent role.
pub fn role_label(role: &str) -> &'static str {
    match role {
        "coder" => "Coder",
        "qa" => "QA-Tester",
        "pr-feedback" => "PR-Feedback",
        _ => "Rolle",
    }
}

fn check(level: Level, title: String, detail: impl Into<String>, fix: Option<(&'static str, Fix)>) -> Check {
    Check { level, title, detail: detail.into(), fix }
}

fn recent(t: Option<DateTime<Local>>, now: DateTime<Local>, hours: i64) -> Option<DateTime<Local>> {
    t.filter(|t| now - *t < Duration::hours(hours))
}

/// Health checks for the issue agent. A PO-only setup (no issue agent) is not a finding —
/// the Team page explains how to connect one.
pub fn checks(s: &Snapshot, now: DateTime<Local>) -> Vec<Check> {
    let t = &s.team;
    if t.dir.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    unit_checks(t, now, &mut out);
    log_checks(t, now, &mut out);
    collision_check(s, &mut out);
    out
}

fn unit_checks(t: &TeamSnapshot, now: DateTime<Local>, out: &mut Vec<Check>) {
    let units = match &t.units {
        Some(Ok(u)) => u,
        Some(Err(e)) => {
            out.push(check(Level::Warn, "WSL nicht erreichbar".into(), format!("Status der Agents unbekannt: {e}"), None));
            return;
        }
        None => return,
    };
    for (role, _) in UNITS {
        let name = role_label(role);
        let state = units.get(*role).map(String::as_str).unwrap_or("unbekannt");
        if state != "active" {
            out.push(check(Level::Error, format!("{name} läuft nicht"), format!("systemd-Unit ist '{state}'."), Some(("Starten", Fix::StartUnit(role.to_string())))));
            continue;
        }
        if let Some(reason) = t.pauses.get(*role) {
            out.push(check(Level::Warn, format!("{name} pausiert"), format!("Grund: {reason}"), Some(("Fortsetzen", Fix::ResumeRole(role.to_string())))));
            continue;
        }
        let Some(hb) = t.heartbeats.get(*role) else { continue };
        // working without a Claude session (git, builds, test suites) can be quiet for a while
        let limit = if hb.state == "working" { Duration::minutes(60) } else { Duration::minutes(IDLE_SILENCE_MINUTES) };
        if let Some(u) = hb.updated.filter(|u| now - *u > limit) {
            out.push(check(Level::Warn, format!("{name} reagiert nicht"), format!("Letztes Lebenszeichen {} (Zustand '{}').", relative(u, now), hb.state), Some(("Neu starten", Fix::RestartUnit(role.to_string())))));
        }
    }
}

fn log_checks(t: &TeamSnapshot, now: DateTime<Local>, out: &mut Vec<Check>) {
    let Some(f) = &t.log else { return };
    let version = t.claude_version.as_ref().and_then(|v| v.as_ref().ok()).map(|v| format!(" (installiert: {v})")).unwrap_or_default();
    if let Some(at) = recent(f.model_unsupported, now, 2) {
        out.push(check(Level::Error, "Claude-CLI in WSL zu alt".into(), format!("{}: das konfigurierte Modell wird nicht unterstützt{version}.\n→ in WSL: npm install -g @anthropic-ai/claude-code@latest", relative(at, now)), None));
    }
    if let Some(at) = recent(f.not_logged_in, now, 2) {
        out.push(check(Level::Error, "Claude in WSL nicht angemeldet".into(), format!("{}: 'Not logged in'. Ohne ANTHROPIC_API_KEY in der .env braucht WSL ein eigenes /login.", relative(at, now)), None));
    }
    let worst = f.qa_loops.iter().filter(|(_, n, at)| *n >= QA_LOOP_ROUNDS && now - *at < Duration::hours(3)).max_by_key(|(_, n, _)| *n);
    if let Some(&(pr, n, at)) = worst {
        out.push(check(Level::Error, "QA-Schleife".into(), format!("PR #{pr} ist {n}× durch die QA gefallen (zuletzt {}). Jede Runde kostet einen Review-Lauf.", relative(at, now)), None));
    }
    let failures = f.claude_failures.iter().filter(|t| now - **t < Duration::hours(2)).count();
    if failures >= 3 {
        out.push(check(Level::Warn, "Claude-Aufrufe scheitern".into(), format!("{failures} fehlgeschlagene Claude-Läufe in den letzten 2 Std. — siehe Protokolle."), None));
    }
    if f.log_bytes > LOG_WARN_BYTES {
        out.push(check(Level::Info, "agent.log sehr groß".into(), format!("{} MB — wird nie rotiert.", f.log_bytes / 1024 / 1024), None));
    }
}

/// The PO loop's own workers and the issue agent's coder on the same repo pick the same issues.
fn collision_check(s: &Snapshot, out: &mut Vec<Check>) {
    let Ok(cfg) = &s.config else { return };
    let Some(env) = &s.team.env else { return };
    let repo = cfg.str("githubRepo");
    let coder_up = matches!(&s.team.units, Some(Ok(u)) if u.get("coder").is_some_and(|st| st == "active"));
    let shared = env.list("AGENT_REPOS").iter().any(|r| r.eq_ignore_ascii_case(&repo));
    if cfg.bool("workersEnabled") && coder_up && shared {
        out.push(check(Level::Warn, "Zwei Coder auf einem Repo".into(), format!("PO-Loop-Worker und Issue-Agent-Coder arbeiten beide an {repo} und konkurrieren um dieselben Issues."), Some(("Loop-Worker abschalten", Fix::DisableLoopWorkers))));
    }
}

#[cfg(test)]
#[path = "team_tests.rs"]
mod tests;
