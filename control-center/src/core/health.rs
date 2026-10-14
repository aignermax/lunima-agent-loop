//! Health checks: turns a snapshot into a traffic light plus concrete, fixable findings.
//! Each check exists because its failure once stopped the team unnoticed.

use super::agent_files::LogFindings;
use super::snapshot::{relative, Snapshot};
use super::team::{role_label, role_status, stale_since, ROLES};
use chrono::{DateTime, Duration, Local};
use team_core::state::parse_time;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Pending,
    Ok,
    Info,
    Warn,
    Error,
}

/// One-click remedies the UI can offer next to a finding.
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    OpenSetup,
    StartAgent,
    RestartAgent,
    EnableAgentAutostart,
    ResumeRole(String),
}

#[derive(Debug, Clone)]
pub struct Check {
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub fix: Option<(&'static str, Fix)>,
}

#[derive(Debug, Clone)]
pub struct Health {
    pub overall: Level,
    pub headline: String,
    pub subline: String,
    pub checks: Vec<Check>,
}

fn check(level: Level, title: impl Into<String>, detail: impl Into<String>, fix: Option<(&'static str, Fix)>) -> Check {
    Check { level, title: title.into(), detail: detail.into(), fix }
}

pub fn evaluate(s: &Snapshot, now: DateTime<Local>) -> Health {
    let mut checks = Vec::new();
    config_checks(s, &mut checks);
    setup_checks(s, &mut checks);
    agent_checks(s, now, &mut checks);
    if s.agent_running {
        role_checks(s, now, &mut checks);
    }
    po_check(s, now, &mut checks);
    if let Some(f) = &s.log {
        log_checks(f, now, &mut checks);
    }
    let overall = checks.iter().map(|c| c.level).max().unwrap_or(Level::Pending);
    let (headline, subline) = headline(s, &checks, overall, now);
    Health { overall, headline, subline, checks }
}

fn config_checks(s: &Snapshot, out: &mut Vec<Check>) {
    match &s.config {
        Err(_) if s.config_missing => out.push(check(Level::Error, "Keine Konfiguration", format!("{} fehlt — der Einrichtungsassistent legt sie an.", s.paths.config().display()), Some(("Einrichten", Fix::OpenSetup)))),
        Err(e) => out.push(check(Level::Error, "Konfiguration fehlerhaft", e.clone(), None)),
        Ok(c) => out.push(check(Level::Ok, "Konfiguration", format!("team.toml · Rechner „{}“ · {} Repos", c.machine, c.repos.len()), None)),
    }
}

fn setup_checks(s: &Snapshot, out: &mut Vec<Check>) {
    let Some(checks) = &s.setup else { return };
    for c in checks.iter().filter(|c| !c.ok && !c.optional && !c.title.starts_with("Konfiguration")) {
        out.push(check(Level::Error, c.title, c.detail.clone(), Some(("Einrichten", Fix::OpenSetup))));
    }
}

fn agent_checks(s: &Snapshot, now: DateTime<Local>, out: &mut Vec<Check>) {
    if s.agent_exe.is_none() {
        out.push(check(Level::Error, "Team-Agent nicht installiert", "lunima-team-agent liegt weder neben dieser App noch im PATH.", None));
        return;
    }
    if !s.agent_running {
        let fix = s.config.is_ok().then_some(("Starten", Fix::StartAgent));
        out.push(check(Level::Error, "Team-Agent läuft nicht", "Ohne ihn arbeitet keine Rolle.", fix));
    } else {
        let busy = s.heartbeats.values().filter(|h| h.state == "working" && stale_since(h, now).is_none()).count();
        out.push(check(Level::Ok, "Team-Agent", format!("läuft · {busy} Rolle(n) arbeiten gerade"), None));
    }
    match &s.service {
        Some(Ok(i)) if !i.autostart => out.push(check(Level::Info, "Kein Autostart", "Der Team-Agent startet nicht von selbst mit der Anmeldung.", Some(("Einschalten", Fix::EnableAgentAutostart)))),
        Some(Err(e)) => out.push(check(Level::Warn, "Autostart unbekannt", format!("Abfrage fehlgeschlagen: {e}"), None)),
        _ => {}
    }
}

fn role_checks(s: &Snapshot, now: DateTime<Local>, out: &mut Vec<Check>) {
    for (role, _) in ROLES.iter().filter(|(r, _)| s.enabled_roles().contains(r)) {
        let name = role_label(role);
        if let Some(reason) = s.pauses.get(*role) {
            out.push(check(Level::Warn, format!("{name} pausiert"), format!("Grund: {reason}"), Some(("Fortsetzen", Fix::ResumeRole(role.to_string())))));
            continue;
        }
        let (level, text) = role_status(s, role, now);
        if level == Level::Warn {
            let fix = text.starts_with("reagiert nicht").then_some(("Neu starten", Fix::RestartAgent));
            out.push(check(Level::Warn, format!("{name}: {}", if fix.is_some() { "reagiert nicht" } else { "wartet" }), text, fix));
        }
    }
}

fn po_check(s: &Snapshot, now: DateTime<Local>, out: &mut Vec<Check>) {
    if !s.enabled_roles().contains(&"product-owner") {
        return;
    }
    let Some(po) = &s.po else {
        out.push(check(Level::Info, "Product Owner", "Noch kein PO-Lauf protokolliert.", None));
        return;
    };
    let last = po.last_start.as_deref().and_then(parse_time).map(|t| relative(t, now)).unwrap_or_else(|| "—".into());
    let note = if po.last_note.is_empty() { String::new() } else { format!(": {}", po.last_note) };
    match po.last_result.as_str() {
        "error" | "timeout" => out.push(check(Level::Warn, "Letzter PO-Lauf fehlgeschlagen", format!("{last} ({}){note}", po.last_result), None)),
        _ => {
            let overdue = po.next_due.as_deref().and_then(parse_time).filter(|d| s.agent_running && now - *d > Duration::hours(1));
            match overdue {
                Some(d) => out.push(check(Level::Warn, "PO-Lauf überfällig", format!("war fällig {}", relative(d, now)), Some(("Neu starten", Fix::RestartAgent)))),
                None => out.push(check(Level::Ok, "Product Owner", format!("letzter Lauf {last}{note}"), None)),
            }
        }
    }
}

fn log_checks(f: &LogFindings, now: DateTime<Local>, out: &mut Vec<Check>) {
    if let Some(at) = f.not_logged_in.filter(|t| now - *t < Duration::hours(2)) {
        out.push(check(Level::Error, "Claude nicht angemeldet", format!("{}: Claude meldet „not logged in“.", relative(at, now)), Some(("Einrichten", Fix::OpenSetup))));
    }
    let failures = LogFindings::count_since(&f.claude_failures, now, 2);
    if failures >= 3 {
        out.push(check(Level::Warn, "Claude-Läufe scheitern", format!("{failures} abgebrochene oder hängende Claude-Läufe in 2 Std. — siehe Protokolle."), None));
    }
    let panics = LogFindings::count_since(&f.panics, now, 24);
    if panics > 0 {
        out.push(check(Level::Warn, "Interne Fehler im Agent", format!("{panics} abgefangene Abstürze in 24 Std. — siehe agent.log."), None));
    }
}

fn headline(s: &Snapshot, checks: &[Check], overall: Level, now: DateTime<Local>) -> (String, String) {
    let next = s.po.as_ref().and_then(|p| p.next_due.as_deref()).and_then(parse_time).filter(|t| *t > now).map(|t| format!("Nächster PO-Lauf {}", relative(t, now)));
    let working = s.heartbeats.values().any(|h| h.state == "working" && stale_since(h, now).is_none());
    match overall {
        Level::Error => ("Braucht Aufmerksamkeit".into(), checks.iter().find(|c| c.level == Level::Error).map(|c| c.title.clone()).unwrap_or_default()),
        _ if s.all_paused() => ("Pausiert".into(), "Es wird nichts bearbeitet, bis du fortsetzt.".into()),
        _ if working => ("Arbeitet gerade".into(), next.unwrap_or_default()),
        Level::Warn => ("Läuft mit Hinweisen".into(), next.unwrap_or_default()),
        Level::Pending => ("Wird geprüft …".into(), String::new()),
        _ => ("Alles in Ordnung".into(), next.unwrap_or_else(|| "Wartet auf Arbeit".into())),
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
