//! Collector side of the team: refreshing issue-agent data and executing team actions.

use crate::core::config::{ConfigDoc, FIELDS};
use crate::core::envfile::EnvFile;
use crate::core::issue_agent;
use crate::core::root::CONFIG_FILE;
use crate::core::team::{role_label, TeamSnapshot};
use crate::sys::wsl;
use chrono::Local;
use std::path::Path;

const HISTORY_SHOWN: usize = 40;

/// Cheap file reads, every few seconds. Keeps the slow probes of `prev`.
pub fn refresh_fast(loop_root: &Path, prev: &TeamSnapshot) -> TeamSnapshot {
    let Some(dir) = issue_agent::resolve_dir(loop_root) else { return TeamSnapshot::default() };
    TeamSnapshot {
        heartbeats: issue_agent::read_heartbeats(&dir),
        pauses: issue_agent::read_pauses(&dir, Local::now()),
        history: issue_agent::read_history(&dir, HISTORY_SHOWN),
        env: EnvFile::load(&dir.join(".env")).ok(),
        units: prev.units.clone(),
        claude_version: prev.claude_version.clone(),
        log: prev.log.clone(),
        dir: Some(dir),
    }
}

/// wsl.exe calls and the log scan, every 90 s.
pub fn refresh_slow(team: &mut TeamSnapshot) {
    let Some(dir) = team.dir.clone() else { return };
    team.units = Some(wsl::unit_states());
    team.claude_version = Some(wsl::claude_version());
    team.log = Some(issue_agent::scan_log(&dir));
}

fn dir(loop_root: &Path) -> Result<std::path::PathBuf, String> {
    issue_agent::resolve_dir(loop_root).ok_or_else(|| "Issue-Agent-Ordner nicht gefunden".to_string())
}

/// start / stop / restart an issue-agent role's systemd unit.
pub fn unit(role: &str, verb: &str) -> Result<String, String> {
    let unit = wsl::unit_of(role).ok_or_else(|| format!("unbekannte Rolle {role}"))?;
    wsl::unit_action(verb, unit)?;
    let done = match verb {
        "start" => "gestartet",
        "stop" => "gestoppt",
        _ => "neu gestartet",
    };
    Ok(format!("{} {done}", role_label(role)))
}

pub fn set_paused(loop_root: &Path, role: &str, paused: bool) -> Result<String, String> {
    issue_agent::set_pause(&dir(loop_root)?, role, paused.then_some("Control Center"))?;
    Ok(format!("{} {}", role_label(role), if paused { "pausiert (ab dem nächsten Zyklus)" } else { "läuft wieder" }))
}

/// Toggles a boolean in agent-loop.json (workersEnabled, customerEnabled, …).
pub fn set_loop_flag(loop_root: &Path, key: &str, value: bool) -> Result<String, String> {
    let mut doc = ConfigDoc::load(&loop_root.join(CONFIG_FILE))?;
    let spec = FIELDS.iter().find(|f| f.key == key).ok_or_else(|| format!("unbekannte Einstellung {key}"))?;
    doc.set_from_text(spec, &value.to_string())?;
    doc.validate()?;
    doc.save()?;
    Ok(format!("{}: {}", spec.label, if value { "an" } else { "aus" }))
}

pub fn set_agent_dir(loop_root: &Path, path: &Path) -> Result<String, String> {
    if !path.join("main.py").is_file() {
        return Err(format!("{} enthält keine main.py des Issue-Agents", path.display()));
    }
    issue_agent::save_dir_setting(loop_root, path)?;
    Ok("Issue-Agent verbunden".into())
}

/// Saves the issue agent's .env and restarts its daemons so they load it.
pub fn save_env(env: &EnvFile, restart: bool) -> Result<String, String> {
    env.save()?;
    if !restart {
        return Ok(".env gespeichert".into());
    }
    for (_, unit) in wsl::UNITS {
        wsl::unit_action("restart", unit)?;
    }
    Ok(".env gespeichert — Coder, QA und PR-Feedback neu gestartet".into())
}
