//! Collector side of the team: refreshing issue-agent data and executing team actions.

use crate::core::config::{ConfigDoc, FIELDS};
use crate::core::envfile::{EnvEdits, EnvFile};
use crate::core::issue_agent;
use crate::core::root::CONFIG_FILE;
use crate::core::team::{role_label, TeamSnapshot};
use crate::sys::wsl;
use chrono::Local;
use std::path::Path;

const HISTORY_SHOWN: usize = 40;
const TOML_REFRESH: std::time::Duration = std::time::Duration::from_secs(600);
const DEFAULT_EXPIRY_DAYS: i64 = 8;

/// Cheap file reads, every few seconds. Keeps the slow probes of `prev`.
pub fn refresh_fast(loop_root: &Path, prev: &TeamSnapshot) -> TeamSnapshot {
    let Some(dir) = issue_agent::resolve_dir(loop_root) else { return TeamSnapshot::default() };
    let env = EnvFile::load(&dir.join(".env")).ok();
    let expiry = env.as_ref().and_then(|e| e.get("AGENT_DISCOVERY_EXPIRY_DAYS")).and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_EXPIRY_DAYS);
    TeamSnapshot {
        heartbeats: issue_agent::read_heartbeats(&dir),
        pauses: issue_agent::read_pauses(&dir, Local::now()),
        history: issue_agent::read_history(&dir, HISTORY_SHOWN),
        discovered: crate::core::projects::read_discovered(&dir, expiry, Local::now().naive_local()),
        agent_tomls: prev.agent_tomls.clone(),
        agent_toml_at: prev.agent_toml_at.clone(),
        env,
        units: prev.units.clone(),
        claude_version: prev.claude_version.clone(),
        wsl_offset: prev.wsl_offset,
        log: prev.log.clone(),
        dir: Some(dir),
    }
}

/// wsl.exe calls, the log scan and each project's .agent.toml, every 90 s.
pub fn refresh_slow(team: &mut TeamSnapshot, po_repo: &str) {
    let Some(dir) = team.dir.clone() else { return };
    if let Some(env) = &team.env {
        // one gh call per repo, but at most every TOML_REFRESH — actions trigger slow refreshes
        for p in crate::core::projects::list(env, &team.discovered, po_repo) {
            if team.agent_toml_at.get(&p.repo).is_some_and(|t| t.elapsed() < TOML_REFRESH) {
                continue;
            }
            let roles = crate::sys::github::fetch_raw(&p.repo, ".agent.toml").map(|t| crate::core::projects::parse_agents_enabled(&t));
            team.agent_tomls.insert(p.repo.clone(), roles);
            team.agent_toml_at.insert(p.repo, std::time::Instant::now());
        }
    }
    team.units = Some(wsl::unit_states());
    let probe = wsl::probe();
    team.wsl_offset = probe.as_ref().ok().and_then(|(_, o)| *o).or(team.wsl_offset);
    team.claude_version = Some(probe.map(|(v, _)| v));
    team.log = Some(issue_agent::scan_log(&dir, team.wsl_offset));
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
    let spec = FIELDS.iter().find(|f| f.key == key).ok_or_else(|| format!("unbekannte Einstellung {key}"))?;
    ConfigDoc::set_bool_in_file(&loop_root.join(CONFIG_FILE), key, value)?;
    Ok(format!("{}: {}", spec.label, if value { "an" } else { "aus" }))
}

pub fn set_agent_dir(loop_root: &Path, path: &Path) -> Result<String, String> {
    if !issue_agent::is_agent_dir(path) {
        return Err(format!("{} ist kein autonomous-issue-agent (main.py und src/ fehlen)", path.display()));
    }
    issue_agent::save_dir_setting(loop_root, path)?;
    Ok("Issue-Agent verbunden".into())
}

/// Applies key-level edits to the issue agent's .env *as it is now* and restarts its
/// daemons so they load it.
pub fn save_env(loop_root: &Path, edits: &EnvEdits, restart: bool) -> Result<String, String> {
    let mut env = EnvFile::load(&dir(loop_root)?.join(".env"))?;
    env.apply(edits);
    env.save()?;
    if !restart {
        return Ok(".env gespeichert".into());
    }
    for (_, unit) in wsl::UNITS {
        wsl::unit_action("restart", unit)?;
    }
    Ok(".env gespeichert — Coder, QA und PR-Feedback neu gestartet".into())
}
