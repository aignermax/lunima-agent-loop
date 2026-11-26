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
use team_core::config::{Backend, TeamConfig};
use team_core::detail;
use team_core::paths::Paths;
use team_core::state::{pause_reason, Heartbeat};

#[path = "update_drain.rs"]
pub(crate) mod update_drain;

/// Shared "subscription exhausted until" across the role threads.
pub type UsageLimit = Arc<Mutex<std::collections::BTreeMap<Backend, DateTime<Local>>>>;

/// Manual `once` commands check this after acquiring the installer's instance lock.
pub fn update_requested(paths: &Paths) -> bool {
    update_drain::pending(&paths.state()).is_some()
}

/// A canceled drain returns control to the supervisor to rebuild its workers.
fn finish_drain(drain: &update_drain::Drain, threads: Vec<std::thread::JoinHandle<()>>) -> Option<i32> {
    for thread in threads {
        let _ = thread.join();
    }
    match drain.acknowledge() {
        Ok(true) => Some(0),
        Ok(false) => None,
        Err(e) => {
            crate::log::error(&format!("Update-Drain konnte nicht bestätigt werden: {e}"));
            Some(1)
        }
    }
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

pub fn role_enabled(cfg: &TeamConfig, name: &str) -> bool {
    match name {
        "coder" => cfg.roles.coder,
        "qa" => cfg.roles.qa,
        "pr-feedback" => cfg.roles.pr_feedback,
        "product-owner" => cfg.roles.product_owner,
        "ux-review" => cfg.roles.ux_review,
        "product-discovery" => cfg.roles.product_discovery,
        _ => false,
    }
}

/// The real context for one cycle (GitHub via gh, Claude via the CLI).
pub fn context(cfg: TeamConfig, paths: &Paths, login: &str, heartbeat: Arc<Heartbeat>, role: &str) -> Ctx {
    context_with_limit(cfg, paths, login, heartbeat, role, None)
}

fn context_with_limit(cfg: TeamConfig, paths: &Paths, login: &str, heartbeat: Arc<Heartbeat>, role: &str, limit: Option<UsageLimit>) -> Ctx {
    let inactivity = Duration::from_secs(cfg.budgets.inactivity_minutes.max(1) * 60);
    let mut llm = ClaudeCli::new(&paths.state(), paths.logs().join(role), inactivity, Some(heartbeat.clone()), &cfg);
    llm.role = role.to_string();
    llm.usage_limit = limit;
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
fn gh_login_with(drain: &update_drain::Drain, mut lookup: impl FnMut() -> Result<String, String>) -> Option<String> {
    let mut wait = 15;
    loop {
        if drain.draining() {
            return None;
        }
        match lookup() {
            Ok(l) => return Some(l),
            Err(e) => {
                crate::log::error(&format!("GitHub-Login nicht verfügbar ({e}) — `gh auth login` nötig; neuer Versuch in {wait} s"));
                if drain.wait(Duration::from_secs(wait)) {
                    return None;
                }
                wait = (wait * 2).min(600);
            }
        }
    }
}

pub fn all_roles() -> Vec<Box<dyn Role>> {
    vec![Box::new(Coder::default()), Box::new(Qa::default()), Box::new(PrFeedback::default()), Box::new(ProductOwner::default()), Box::new(crate::ux::UxReviewer::default()), Box::new(crate::product_discovery::ProductDiscovery::default())]
}

/// Runs until stopped or until an update drain completes. Cancellation rebuilds all
/// workers after joining the old generation; the daemon keeps its instance lock.
pub fn run(paths: Paths) -> i32 {
    crate::log::init(&paths.logs());
    crate::log::info(&format!("lunima-team-agent {} startet (Daten: {})", env!("CARGO_PKG_VERSION"), paths.root.display()));
    let mut login = None;
    let limit: UsageLimit = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    loop {
        let drain = update_drain::Drain::new(paths.state());
        if login.is_none() {
            login = gh_login_with(&drain, team_core::gh::current_user);
            if let Some(login) = &login {
                crate::log::info(&format!("GitHub-Login: {login}"));
            }
        }
        if drain.draining() {
            if let Some(code) = finish_drain(&drain, vec![]) {
                return code;
            }
            continue;
        }
        let Some(login) = login.clone() else { continue };
        let mut threads = vec![crate::chat::spawn_worker(paths.clone(), drain.clone())];
        threads.extend(all_roles().into_iter().map(|role| {
            let (paths, login, limit, drain) = (paths.clone(), login.clone(), limit.clone(), drain.clone());
            std::thread::Builder::new().name(role.name().into()).spawn(move || role_loop(role, paths, login, limit, drain)).expect("spawn role thread")
        }));
        if let Some(code) = finish_drain(&drain, threads) {
            return code;
        }
        crate::log::info("Update-Drain abgebrochen oder abgelaufen — Team startet wieder");
    }
}

fn role_loop(mut role: Box<dyn Role>, paths: Paths, login: String, limit: UsageLimit, drain: Arc<update_drain::Drain>) {
    let name = role.name();
    crate::log::set_role(name);
    let hb = Arc::new(Heartbeat::new(&paths.state(), name));
    crate::shell::set_heartbeat(hb.clone());
    let mut last_good: Option<TeamConfig> = None;
    let mut recovered = false;
    let mut failures = 0u32;
    loop {
        if drain.draining() {
            return;
        }
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
                    drain.wait(Duration::from_secs(30));
                    continue;
                }
            },
        };
        let poll = cfg.poll_seconds.max(1);
        if let Some(reason) = blocked(&cfg, name, &paths, &limit) {
            hb.report("paused", detail!["reason" => reason]);
            drain.wait(Duration::from_secs(poll.min(60)));
            continue;
        }
        // Admission and request observation share one lock with chat and the supervisor.
        let Some(work) = drain.try_start() else { return };
        hb.idle();
        let ctx = context_with_limit(cfg, &paths, &login, hb.clone(), name, Some(limit.clone()));
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
            Ok(Err(RoleError::UsageLimit(backend, until))) => {
                crate::log::warn(&format!("{}-Nutzungslimit — Rollen dieses Backends warten bis {}", backend.label(), until.format("%d.%m. %H:%M")));
                limit.lock().unwrap_or_else(|p| p.into_inner()).insert(backend, until);
                1
            }
            Ok(Err(RoleError::Unavailable(e))) => {
                hb.report("paused", detail!["reason" => format!("Sitzung wartet: {e}")]);
                poll.max(15)
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
        drop(work);
        if drain.wait(Duration::from_secs(wait)) {
            return;
        }
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
    team_core::mem::configure(cfg.resources.min_free_mb, cfg.resources.start_free_mb);
    if let Some(reason) = team_core::mem::start_blocked() {
        return Some(format!("{reason} — wartet, bis wieder Platz ist"));
    }
    let mut limits = limit.lock().unwrap_or_else(|p| p.into_inner());
    limits.retain(|_, until| *until > Local::now());
    let repos = match name {
        "product-owner" => vec![cfg.product_owner.repo.clone()],
        "product-discovery" => vec![if cfg.product_discovery.repo.is_empty() { cfg.product_owner.repo.clone() } else { cfg.product_discovery.repo.clone() }],
        "ux-review" => vec![if cfg.ux_review.repo.is_empty() { cfg.product_owner.repo.clone() } else { cfg.ux_review.repo.clone() }],
        _ => crate::repos::effective(cfg, &paths.state(), name == "coder"),
    };
    let mut backends = Vec::new();
    for repo in &repos {
        if let Some(profile) = cfg.repo(repo).and_then(|r| r.profile.as_ref()) {
            backends.extend(team_core::project::configured_backends(cfg, repo).unwrap_or_else(|_| profile.allowed_backends.clone()));
            continue;
        }
        backends.push(team_core::provider::repo_runner(cfg, repo).backend);
        if cfg.repo(repo).is_some_and(|r| matches!(r.routing, team_core::config::Routing::Kimi | team_core::config::Routing::KimiOnly)) {
            backends.extend([Backend::Openrouter, Backend::Eco]);
        }
    }
    // Mixed roles rotate while any project provider is available. Session admission
    // checks the actual endpoint, including Kimi sessions using the Claude executable.
    (!backends.is_empty() && backends.iter().all(|backend| limits.contains_key(backend)))
        .then(|| format!("{}-Nutzungslimit", backends.iter().map(|b| b.label()).collect::<Vec<_>>().join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HeldRole {
        started: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl Role for HeldRole {
        fn name(&self) -> &'static str { "coder" }
        fn cycle(&mut self, _: &Ctx) -> crate::ctx::RoleResult<bool> {
            self.started.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(false)
        }
    }

    fn request_drain(paths: &Paths) {
        let request = serde_json::json!({"token": "daemon-test", "requested_at": Local::now().timestamp()});
        std::fs::write(paths.state().join("update-drain.json"), request.to_string()).unwrap();
    }

    #[test]
    fn daemon_with_pending_request_acknowledges_and_returns_success_without_login() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        request_drain(&paths);
        assert_eq!(run(paths.clone()), 0);
        let ack: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.state().join("update-drained.json")).unwrap()).unwrap();
        assert_eq!(ack, serde_json::json!({"token": "daemon-test", "pid": std::process::id()}));
    }

    #[test]
    fn canceled_supervisor_drain_restarts_admission_after_all_workers_join() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        let drain = update_drain::Drain::new(paths.state());
        let work = drain.try_start().unwrap();
        request_drain(&paths);
        assert!(drain.draining());
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _work = work;
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let supervisor = std::thread::spawn(move || done_tx.send(finish_drain(&drain, vec![worker])).unwrap());
        assert!(done_rx.recv_timeout(Duration::from_millis(300)).is_err());
        std::fs::remove_file(paths.state().join("update-drain.json")).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap(), None);
        supervisor.join().unwrap();
        assert!(!paths.state().join("update-drained.json").exists());
        assert!(update_drain::Drain::new(paths.state()).try_start().is_some());
    }

    #[test]
    fn supervisor_acknowledges_only_after_worker_thread_has_exited() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        let drain = update_drain::Drain::new(paths.state());
        request_drain(&paths);
        assert!(drain.draining());
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || release_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let supervisor = std::thread::spawn(move || done_tx.send(finish_drain(&drain, vec![worker])).unwrap());
        assert!(done_rx.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(!paths.state().join("update-drained.json").exists());
        release_tx.send(()).unwrap();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap(), Some(0));
        supervisor.join().unwrap();
        assert!(paths.state().join("update-drained.json").exists());
    }

    #[test]
    fn login_retry_wait_drains_without_another_github_call() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        let drain = update_drain::Drain::new(paths.state());
        let (called_tx, called_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = gh_login_with(&drain, || {
                called_tx.send(()).unwrap();
                Err("not logged in".into())
            });
            done_tx.send(result).unwrap();
        });
        called_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        request_drain(&paths);
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap(), None);
        worker.join().unwrap();
        assert!(called_rx.try_recv().is_err());
    }

    #[test]
    fn pending_drain_skips_login_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        request_drain(&paths);
        let drain = update_drain::Drain::new(paths.state());
        assert_eq!(gh_login_with(&drain, || panic!("must not contact GitHub during drain")), None);
    }

    #[test]
    fn draining_role_finishes_current_cycle_and_skips_next_poll() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        let mut cfg = TeamConfig::default();
        cfg.poll_seconds = 600;
        cfg.resources.min_free_mb = 1;
        cfg.resources.start_free_mb = 1;
        cfg.save(&paths.config()).unwrap();
        let drain = update_drain::Drain::new(paths.state());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (p, d) = (paths.clone(), drain.clone());
        let worker = std::thread::spawn(move || {
            role_loop(Box::new(HeldRole { started: started_tx, release: release_rx }), p, "test-login".into(), Arc::new(Mutex::new(std::collections::BTreeMap::new())), d);
            done_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        request_drain(&paths);
        assert!(drain.draining());
        assert!(done_rx.try_recv().is_err(), "current role cycle must finish naturally");
        assert!(drain.acknowledge().is_err());
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(2)).expect("role should skip its 600-second poll wait");
        worker.join().unwrap();
        assert!(started_rx.try_recv().is_err(), "no second role cycle");
        drain.acknowledge().unwrap();
    }

    #[test]
    fn invalid_config_wait_also_drains_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        let drain = update_drain::Drain::new(paths.state());
        request_drain(&paths);
        let (tx, rx) = std::sync::mpsc::channel();
        let (started_tx, _started_rx) = std::sync::mpsc::channel();
        let (_release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            role_loop(Box::new(HeldRole { started: started_tx, release: release_rx }), paths, "test-login".into(), Arc::new(Mutex::new(std::collections::BTreeMap::new())), drain);
            tx.send(()).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn roles_and_blocking_reasons() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure().unwrap();
        let mut cfg = TeamConfig::default();
        cfg.resources.min_free_mb = 1;
        cfg.resources.start_free_mb = 1;
        let limit: UsageLimit = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
        cfg.repos.push(team_core::config::RepoConfig { name: "o/r".into(), ..Default::default() });
        assert_eq!(all_roles().iter().map(|r| r.name()).collect::<Vec<_>>(), vec!["coder", "qa", "pr-feedback", "product-owner", "ux-review", "product-discovery"]);
        assert!(blocked(&cfg, "coder", &paths, &limit).is_none());
        cfg.roles.qa = false;
        assert!(blocked(&cfg, "qa", &paths, &limit).unwrap().contains("abgeschaltet"));
        assert!(blocked(&cfg, "product-discovery", &paths, &limit).unwrap().contains("abgeschaltet"), "discovery is off unless switched on");
        cfg.roles.product_discovery = true;
        std::fs::write(paths.state().join("control.json"), r#"{"paused":{"product-discovery":{"until":null,"reason":"Notstopp"}}}"#).unwrap();
        assert_eq!(blocked(&cfg, "product-discovery", &paths, &limit).as_deref(), Some("Notstopp"), "discovery pauses independently of delivery");
        assert!(blocked(&cfg, "coder", &paths, &limit).is_none());
        std::fs::remove_file(paths.state().join("control.json")).unwrap();
        limit.lock().unwrap().clear();
        limit.lock().unwrap().insert(Backend::Claude, Local::now() + chrono::Duration::minutes(5));
        assert!(blocked(&cfg, "coder", &paths, &limit).unwrap().contains("Nutzungslimit"));
        cfg.runner.backend = Backend::Codex;
        assert!(blocked(&cfg, "coder", &paths, &limit).is_some(), "Chat runner must not override the Claude project's cooldown");
        cfg.repos[0].routing = team_core::config::Routing::Codex;
        assert!(blocked(&cfg, "coder", &paths, &limit).is_none(), "Claude exhaustion must not block Codex projects");
        limit.lock().unwrap().clear();
        limit.lock().unwrap().insert(Backend::Codex, Local::now() + chrono::Duration::minutes(5));
        assert!(blocked(&cfg, "coder", &paths, &limit).unwrap().contains("Codex"));
        limit.lock().unwrap().clear();
        limit.lock().unwrap().insert(Backend::Claude, Local::now() - chrono::Duration::minutes(1));
        assert!(blocked(&cfg, "coder", &paths, &limit).is_none());
        assert!(limit.lock().unwrap().is_empty(), "elapsed limit is cleared");
        std::fs::write(paths.state().join("control.json"), r#"{"paused":{"coder":{"until":null,"reason":"Urlaub"}}}"#).unwrap();
        assert_eq!(blocked(&cfg, "coder", &paths, &limit).as_deref(), Some("Urlaub"));
    }
}
