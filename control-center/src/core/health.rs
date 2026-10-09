//! Health checks: turns a snapshot into a traffic light plus concrete, fixable findings.
//! Each check exists because its failure once stopped the loop unnoticed.

use super::logs::{is_newest_run_log, Outcome};
use super::snapshot::{relative, Snapshot};
use super::state::{parse_time, Pause};
use chrono::{DateTime, Duration, Local};

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
    Resume,
    EnableConfig,
    EnableTask,
    RescueWip,
    RunOwner,
    /// issue-agent role (coder/qa/pr-feedback)
    StartUnit(String),
    RestartUnit(String),
    ResumeRole(String),
    DisableLoopWorkers,
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

fn check(level: Level, title: &str, detail: impl Into<String>) -> Check {
    Check { level, title: title.to_string(), detail: detail.into(), fix: None }
}

fn with_fix(mut c: Check, label: &'static str, fix: Fix) -> Check {
    c.fix = Some((label, fix));
    c
}

pub fn evaluate(s: &Snapshot, now: DateTime<Local>) -> Health {
    let checks: Vec<Check> = [
        config_check(s),
        pause_check(s, now),
        task_check(s, now),
        owner_check(s, now),
        worker_check(s),
        clone_check(s),
        gh_check(s),
    ]
    .into_iter()
    .flatten()
    .chain(super::team::checks(s, now))
    .collect();
    let overall = checks.iter().map(|c| c.level).max().unwrap_or(Level::Pending);
    let (headline, subline) = headline(s, &checks, overall, now);
    Health { overall, headline, subline, checks }
}

fn config_check(s: &Snapshot) -> Option<Check> {
    if s.cli.is_none() {
        return Some(check(Level::Error, "CLI nicht gefunden", "lunima-agent-loop(.exe) liegt weder in publish/ noch neben dieser App noch im PATH."));
    }
    match &s.config {
        Err(e) => Some(check(Level::Error, "Konfiguration fehlerhaft", e.clone())),
        Ok(c) if !c.bool("enabled") => Some(with_fix(
            check(Level::Warn, "Loop deaktiviert", "\"enabled\": false in agent-loop.json — der Zeitplan feuert, tut aber nichts."),
            "Aktivieren",
            Fix::EnableConfig,
        )),
        Ok(_) => Some(check(Level::Ok, "Konfiguration", "agent-loop.json geladen")),
    }
}

fn pause_check(s: &Snapshot, now: DateTime<Local>) -> Option<Check> {
    let state = s.state.as_ref().ok()?;
    let reason = state.pause_reason.clone().filter(|r| !r.is_empty()).map(|r| format!(" — Grund: {r}")).unwrap_or_default();
    let detail = match state.pause(now) {
        Pause::None => return None,
        Pause::Indefinitely => format!("auf unbestimmte Zeit{reason}. Es laufen weder PO noch Worker."),
        Pause::Until(t) => format!("bis {}{reason}", t.format("%d.%m. %H:%M")),
    };
    Some(with_fix(check(Level::Warn, "Pausiert", detail), "Fortsetzen", Fix::Resume))
}

fn task_check(s: &Snapshot, now: DateTime<Local>) -> Option<Check> {
    Some(match s.task.as_ref()? {
        Err(e) => check(Level::Warn, "Zeitplan", format!("Abfrage fehlgeschlagen: {e}")),
        Ok(t) if !t.exists => check(Level::Error, "Kein Zeitplan", "Scheduled Task 'LunimaAgentLoop' fehlt — scripts\\Register-AgentLoop.ps1 ausführen."),
        Ok(t) if t.state == "Disabled" => with_fix(check(Level::Error, "Zeitplan deaktiviert", "Der stündliche Task ist in Windows abgeschaltet."), "Einschalten", Fix::EnableTask),
        Ok(t) => {
            let next = t.next_run.map(|n| format!("nächster Lauf {}", relative(n, now))).unwrap_or_else(|| "kein nächster Lauf geplant".into());
            let last = t.last_run.map(|l| format!(" · zuletzt {}", relative(l, now))).unwrap_or_default();
            // 0 = success; 0x41301 = currently running; 0x41303 = has not run yet
            let failed = t.last_result.is_some_and(|r| ![0, 0x41301, 0x41303].contains(&r));
            let result = if failed { format!(" (Ergebnis {})", t.last_result.unwrap_or_default()) } else { String::new() };
            check(if failed { Level::Warn } else { Level::Ok }, "Zeitplan", format!("{} · {next}{last}{result}", t.state))
        }
    })
}

fn owner_check(s: &Snapshot, now: DateTime<Local>) -> Option<Check> {
    let Some(latest) = s.passes.first() else {
        return Some(check(Level::Info, "Product Owner", "Noch kein PO-Lauf protokolliert."));
    };
    let when = latest.file.started.map(|t| relative(t, now)).unwrap_or_default();
    let actions = latest.pass.actions.len();
    let running = s.loop_running && is_newest_run_log(&s.logs, &latest.file);
    Some(match latest.outcome(running) {
        Outcome::Running => check(Level::Info, "Product Owner arbeitet", format!("Lauf gestartet {when} · {actions} Aktionen bisher")),
        Outcome::Aborted => check(Level::Warn, "PO-Lauf abgebrochen", format!("Lauf {when} endete ohne Abschluss (Timeout oder Absturz).")),
        Outcome::Failed(msg) => {
            let hint = if msg.to_lowercase().contains("authenticat") {
                "\n→ Claude-Anmeldung abgelaufen: in einem Terminal `claude` starten und /login ausführen."
            } else {
                ""
            };
            // no restart offer while another loop process runs (no locking in the loop)
            let c = check(Level::Error, "Letzter PO-Lauf fehlgeschlagen", format!("{when}: {msg}{hint}"));
            if s.loop_running { c } else { with_fix(c, "Erneut starten", Fix::RunOwner) }
        }
        Outcome::Succeeded => {
            let stale = latest.file.started.is_some_and(|t| now - t > Duration::hours(24));
            check(if stale { Level::Warn } else { Level::Ok }, "Product Owner", format!("letzter Lauf {when} · {actions} Aktionen"))
        }
    })
}

fn worker_check(s: &Snapshot) -> Option<Check> {
    // the loop's own workers are off in team mode — their old failures don't matter
    if !s.config.as_ref().is_ok_and(|c| c.bool("workersEnabled")) {
        return None;
    }
    let streak = s.state.as_ref().ok()?.failure_streak("task");
    if streak.len() < 2 {
        return None;
    }
    let note = streak[0].note.clone().unwrap_or_else(|| format!("Exit {}", streak[0].exit_code));
    let since = streak.last().and_then(|r| parse_time(&r.timestamp)).map(|t| format!(" seit {}", t.format("%d.%m. %H:%M"))).unwrap_or_default();
    let clone_clean = matches!(s.clone.as_ref(), Some(Ok(c)) if c.exists && c.dirty.is_empty());
    if note.contains("branch setup") && clone_clean {
        let detail = format!("{} Läufe in Folge{since}: {note}. Der Clone ist jetzt sauber — der nächste Lauf sollte klappen.", streak.len());
        return Some(check(Level::Warn, "Worker scheiterten", detail));
    }
    let hint = if note.contains("branch setup") { " — meist blockieren uncommittete Änderungen im Clone." } else { "" };
    Some(check(Level::Error, "Worker scheitern", format!("{} Läufe in Folge{since}: {note}{hint}", streak.len())))
}

fn clone_check(s: &Snapshot) -> Option<Check> {
    Some(match s.clone.as_ref()? {
        Err(e) => check(Level::Warn, "Clone", e.clone()),
        Ok(c) if !c.exists => check(Level::Error, "Clone fehlt", "clonePath ist kein Git-Repo — `lunima-agent-loop init` ausführen."),
        // a running pass works in the clone: changes there are expected, never "rescue" them
        Ok(c) if !c.dirty.is_empty() && s.loop_running => check(Level::Info, "Clone in Benutzung", format!("Ein Lauf arbeitet gerade darin ({} geänderte Dateien).", c.dirty.len())),
        Ok(c) if !c.dirty.is_empty() => with_fix(
            check(Level::Error, "Clone blockiert", format!("{} uncommittete Dateien verhindern jeden Worker-Lauf (z. B. {}).", c.dirty.len(), c.dirty[0])),
            "Als WIP sichern",
            Fix::RescueWip,
        ),
        Ok(c) => check(Level::Ok, "Clone", format!("sauber · {}", c.branch_line)),
    })
}

fn gh_check(s: &Snapshot) -> Option<Check> {
    Some(match s.gh_auth.as_ref()? {
        Ok(()) => check(Level::Ok, "GitHub CLI", "angemeldet"),
        Err(e) => check(Level::Error, "GitHub CLI", format!("{e} → `gh auth login`")),
    })
}

fn headline(s: &Snapshot, checks: &[Check], overall: Level, now: DateTime<Local>) -> (String, String) {
    let next = s.task.as_ref().and_then(|t| t.as_ref().ok()).and_then(|t| t.next_run).map(|n| format!("Nächster Lauf {}", relative(n, now)));
    let paused = checks.iter().any(|c| c.title == "Pausiert");
    match overall {
        Level::Error => {
            let first = checks.iter().find(|c| c.level == Level::Error).map(|c| c.title.clone()).unwrap_or_default();
            ("Braucht Aufmerksamkeit".into(), first)
        }
        _ if paused => ("Pausiert".into(), "Es wird nichts bearbeitet, bis du fortsetzt.".into()),
        _ if s.loop_running => ("Arbeitet gerade".into(), next.unwrap_or_default()),
        Level::Warn => ("Läuft mit Hinweisen".into(), next.unwrap_or_default()),
        Level::Pending => ("Wird geprüft …".into(), String::new()),
        _ => ("Alles in Ordnung".into(), next.unwrap_or_else(|| "Läuft stündlich".into())),
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
