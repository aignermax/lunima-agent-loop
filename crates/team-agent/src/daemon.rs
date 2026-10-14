//! `lunima-team-agent run`: every role on its own thread, forever.
//!
//! Per cycle a role re-reads team.toml (changes from the Control Center apply without a
//! restart), honours pauses from control.json, and reports its heartbeat. The Claude usage
//! limit belongs to the subscription, so when one role hits it all roles wait. A panicking
//! or failing cycle backs off exponentially; it never takes the process down.

use crate::coder::Coder;
use crate::ctx::{Ctx, RoleError, Tools};
use crate::feedback::PrFeedback;
use crate::forge::Forge;
use crate::llm::ClaudeCli;
use crate::po::ProductOwner;
use crate::qa::Qa;
use crate::role::{backoff_secs, Role};
use chrono::{DateTime, Local};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use team_core::config::TeamConfig;
use team_core::detail;
use team_core::paths::Paths;
use team_core::state::{pause_reason, Heartbeat};

/// Shared "subscription exhausted until" across the role threads.
pub type UsageLimit = Arc<Mutex<Option<DateTime<Local>>>>;

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

pub fn role_enabled(cfg: &TeamConfig, name: &str) -> bool {
    match name {
        "coder" => cfg.roles.coder,
        "qa" => cfg.roles.qa,
        "pr-feedback" => cfg.roles.pr_feedback,
        "product-owner" => cfg.roles.product_owner,
        _ => false,
    }
}

/// The real context for one cycle (GitHub via gh, Claude via the CLI).
pub fn context(cfg: TeamConfig, paths: &Paths, login: &str, heartbeat: Arc<Heartbeat>, role: &str) -> Ctx {
    let inactivity = Duration::from_secs(cfg.budgets.inactivity_minutes.max(1) * 60);
    let llm = ClaudeCli::new(&paths.state(), paths.logs().join(role), inactivity, Some(heartbeat.clone()));
    Ctx {
        machine: cfg.machine.clone(),
        cfg,
        paths: paths.clone(),
        login: login.to_string(),
        llm: Arc::new(llm),
        heartbeat,
        forge: Arc::new(|repo: &str| Arc::new(team_core::gh::Repo::new(repo)) as Arc<dyn Forge>),
        remote: Arc::new(|repo: &str| team_core::git::remote_url(repo)),
        tools: Tools::detect(home()),
    }
}

/// Waits until gh answers (the login may not be ready right after boot).
fn gh_login() -> String {
    let mut wait = 15;
    loop {
        match team_core::gh::current_user() {
            Ok(l) => return l,
            Err(e) => {
                crate::log::error(&format!("GitHub-Login nicht verfügbar ({e}) — `gh auth login` nötig; neuer Versuch in {wait} s"));
                std::thread::sleep(Duration::from_secs(wait));
                wait = (wait * 2).min(600);
            }
        }
    }
}

pub fn all_roles() -> Vec<Box<dyn Role>> {
    vec![Box::new(Coder::default()), Box::new(Qa::default()), Box::new(PrFeedback::default()), Box::new(ProductOwner::default())]
}

/// Runs until the process is stopped.
pub fn run(paths: Paths) -> ! {
    crate::log::init(&paths.logs());
    crate::log::info(&format!("lunima-team-agent {} startet (Daten: {})", env!("CARGO_PKG_VERSION"), paths.root.display()));
    let login = gh_login();
    crate::log::info(&format!("GitHub-Login: {login}"));
    let limit: UsageLimit = Arc::new(Mutex::new(None));
    let threads: Vec<_> = all_roles()
        .into_iter()
        .map(|role| {
            let (paths, login, limit) = (paths.clone(), login.clone(), limit.clone());
            std::thread::Builder::new().name(role.name().into()).spawn(move || role_loop(role, paths, login, limit)).expect("spawn role thread")
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
    unreachable!("role threads never end")
}

fn role_loop(mut role: Box<dyn Role>, paths: Paths, login: String, limit: UsageLimit) {
    let name = role.name();
    crate::log::set_role(name);
    let hb = Arc::new(Heartbeat::new(&paths.state(), name));
    crate::shell::set_heartbeat(hb.clone());
    let mut last_good: Option<TeamConfig> = None;
    let mut recovered = false;
    let mut failures = 0u32;
    loop {
        let cfg = match TeamConfig::load(&paths.config()) {
            Ok(c) => {
                last_good = Some(c.clone());
                c
            }
            Err(e) => match &last_good {
                Some(c) => {
                    crate::log::warn(&format!("team.toml unlesbar ({e}) — letzte gültige Fassung bleibt aktiv"));
                    c.clone()
                }
                None => {
                    hb.report("paused", detail!["reason" => format!("team.toml fehlt oder ist ungültig: {e}")]);
                    sleep(30);
                    continue;
                }
            },
        };
        let poll = cfg.poll_seconds.max(1);
        if let Some(reason) = blocked(&cfg, name, &paths, &limit) {
            hb.report("paused", detail!["reason" => reason]);
            sleep(poll.min(60));
            continue;
        }
        hb.idle();
        let ctx = context(cfg, &paths, &login, hb.clone(), name);
        if !recovered {
            role.recover(&ctx);
            recovered = true;
        }
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| role.cycle(&ctx)));
        hb.idle();
        let wait = match result {
            Ok(Ok(true)) => {
                failures = 0;
                2
            }
            Ok(Ok(false)) => {
                failures = 0;
                poll
            }
            Ok(Err(RoleError::UsageLimit(until))) => {
                crate::log::warn(&format!("Claude-Nutzungslimit — alle Rollen warten bis {}", until.format("%d.%m. %H:%M")));
                *limit.lock().unwrap_or_else(|p| p.into_inner()) = Some(until);
                1
            }
            Ok(Err(RoleError::Failed(e))) => {
                failures += 1;
                crate::log::error(&format!("Zyklus fehlgeschlagen: {e}"));
                backoff_secs(failures, poll)
            }
            Err(panic) => {
                failures += 1;
                let msg = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                crate::log::error(&format!("Interner Fehler (panic): {msg}"));
                backoff_secs(failures, poll)
            }
        };
        sleep(wait);
    }
}

/// Why the role must not work right now, if anything.
fn blocked(cfg: &TeamConfig, name: &str, paths: &Paths, limit: &UsageLimit) -> Option<String> {
    if !role_enabled(cfg, name) {
        return Some("in team.toml abgeschaltet".into());
    }
    if let Some(reason) = pause_reason(&paths.state(), name, Local::now()) {
        return Some(reason);
    }
    let mut until = limit.lock().unwrap_or_else(|p| p.into_inner());
    match *until {
        Some(t) if t > Local::now() => Some(format!("Claude-Nutzungslimit bis {}", t.format("%H:%M"))),
        Some(_) => {
            *until = None;
            None
        }
        None => None,
    }
}

fn sleep(secs: u64) {
    std::thread::sleep(Duration::from_secs(secs));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_blocking_reasons() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure().unwrap();
        let mut cfg = TeamConfig::default();
        let limit: UsageLimit = Arc::new(Mutex::new(None));
        assert_eq!(all_roles().iter().map(|r| r.name()).collect::<Vec<_>>(), vec!["coder", "qa", "pr-feedback", "product-owner"]);
        assert!(blocked(&cfg, "coder", &paths, &limit).is_none());
        cfg.roles.qa = false;
        assert!(blocked(&cfg, "qa", &paths, &limit).unwrap().contains("abgeschaltet"));
        *limit.lock().unwrap() = Some(Local::now() + chrono::Duration::minutes(5));
        assert!(blocked(&cfg, "coder", &paths, &limit).unwrap().contains("Nutzungslimit"));
        *limit.lock().unwrap() = Some(Local::now() - chrono::Duration::minutes(1));
        assert!(blocked(&cfg, "coder", &paths, &limit).is_none());
        assert!(limit.lock().unwrap().is_none(), "elapsed limit is cleared");
        std::fs::write(paths.state().join("control.json"), r#"{"paused":{"coder":{"until":null,"reason":"Urlaub"}}}"#).unwrap();
        assert_eq!(blocked(&cfg, "coder", &paths, &limit).as_deref(), Some("Urlaub"));
    }
}
