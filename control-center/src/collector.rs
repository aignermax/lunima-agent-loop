//! Background worker: keeps the snapshot fresh and executes user actions off the UI thread.

use crate::core::agent_files;
use crate::core::logs::{self, PassCache};
use crate::core::snapshot::Snapshot;
use crate::core::team::{role_label, ROLES};
use crate::sys::agent_service::{self, Verb};
use crate::sys::{autostart, github, probes};
use chrono::Local;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use team_core::config::TeamConfig;
use team_core::paths::Paths;

const FAST: Duration = Duration::from_secs(5);
const SLOW: Duration = Duration::from_secs(90);
const PASSES_SHOWN: usize = 40;
const HISTORY_SHOWN: usize = 60;
const TOML_REFRESH: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub enum Action {
    RefreshAll,
    /// settings page: everything except the repository list
    SaveConfig(Box<TeamConfig>),
    /// projects page: only the repository list
    SaveRepos(Vec<team_core::config::RepoConfig>),
    /// import legacy settings (.env / agent-loop.json) and write team.toml
    CreateConfig,
    SavePrompt(PathBuf, String),
    /// Back to the built-in prompt: remove the override file.
    DeletePrompt(PathBuf),
    SetRolePaused(String, bool),
    PauseAll(bool),
    Agent(Verb),
    SetAgentAutostart(bool),
    SetAutostart(bool),
    Terminal(&'static str),
}

impl Action {
    /// Short German label for the "busy" toast.
    fn label(&self) -> String {
        match self {
            Action::Agent(v) => format!("Team-Agent {v:?}"),
            Action::SaveConfig(_) => "Einstellungen speichern".into(),
            Action::SaveRepos(_) => "Projekte speichern".into(),
            Action::CreateConfig => "Konfiguration anlegen".into(),
            other => format!("{other:?}").chars().take(40).collect(),
        }
    }
}

/// Result message shown as a toast.
#[derive(Debug, Clone)]
pub struct Notice {
    pub ok: bool,
    pub text: String,
    pub at: Instant,
}

pub struct Shared {
    pub snapshot: Mutex<Snapshot>,
    pub notice: Mutex<Option<Notice>>,
    pub busy: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct Handle {
    pub shared: Arc<Shared>,
    tx: Sender<Action>,
}

impl Handle {
    pub fn send(&self, action: Action) {
        let _ = self.tx.send(action);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot.lock().expect("snapshot lock").clone()
    }
}

/// Starts the worker thread. `on_change` is called after every update (repaint + tray).
pub fn start(paths: Paths, on_change: Arc<dyn Fn() + Send + Sync>) -> Handle {
    let shared = Arc::new(Shared { snapshot: Mutex::new(Snapshot::empty(paths.clone())), notice: Mutex::new(None), busy: Mutex::new(None) });
    let (tx, rx) = mpsc::channel();
    let worker = Worker { shared: shared.clone(), paths, cache: PassCache::default(), on_change };
    std::thread::Builder::new().name("collector".into()).spawn(move || worker.run(rx)).expect("spawn collector");
    Handle { shared, tx }
}

struct Worker {
    shared: Arc<Shared>,
    paths: Paths,
    cache: PassCache,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Action>) {
        let mut last_slow: Option<Instant> = None;
        loop {
            self.refresh_fast();
            if last_slow.is_none_or(|t| t.elapsed() >= SLOW) {
                self.refresh_slow();
                last_slow = Some(Instant::now());
            }
            match rx.recv_timeout(FAST) {
                Ok(action) => {
                    self.execute(action);
                    last_slow = None;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn snap(&self) -> Snapshot {
        self.shared.snapshot.lock().expect("snapshot lock").clone()
    }

    fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.shared.snapshot.lock().expect("snapshot lock"));
        (self.on_change)();
    }

    fn refresh_fast(&mut self) {
        let (state, now) = (self.paths.state(), Local::now());
        let missing = !self.paths.config().is_file();
        let config = if missing { Err("team.toml fehlt".to_string()) } else { TeamConfig::load(&self.paths.config()) };
        let expiry = config.as_ref().map(|c| c.discovery.expiry_days).unwrap_or(8);
        let files = logs::list(&self.paths.logs());
        let passes = self.cache.passes(&files, PASSES_SHOWN);
        let fresh = (
            agent_files::read_heartbeats(&state),
            agent_files::read_pauses(&state, now),
            agent_files::read_history(&state, HISTORY_SHOWN),
            agent_files::read_discovered(&state, expiry, now.naive_local()),
            team_core::state::PoState::load(&state),
            agent_files::agent_running(&state),
        );
        self.update(|s| {
            (s.heartbeats, s.pauses, s.history, s.discovered, s.po, s.agent_running) = fresh;
            s.config = config;
            s.config_missing = missing;
            s.logs = files;
            s.passes = passes;
            s.agent_exe = agent_service::find_exe();
            s.refreshed = now;
        });
    }

    fn refresh_slow(&mut self) {
        let snap = self.snap();
        let service = Some(agent_service::query());
        let setup = crate::core::setup::evaluate(&probes::System, !snap.config_missing);
        let repo = snap.github_repo();
        let gh = if repo.is_empty() { Err("kein Repository konfiguriert".into()) } else { github::fetch(&repo) };
        let (mut tomls, mut toml_at) = (snap.agent_tomls.clone(), snap.agent_toml_at.clone());
        if let Some(cfg) = snap.cfg() {
            // one gh call per repo, at most every TOML_REFRESH — actions trigger slow refreshes
            for p in crate::core::projects::list(cfg, &snap.discovered) {
                if toml_at.get(&p.repo).is_some_and(|t| t.elapsed() < TOML_REFRESH) {
                    continue;
                }
                tomls.insert(p.repo.clone(), github::fetch_raw(&p.repo, ".agent.toml").map(|t| crate::core::projects::parse_agents_enabled(&t)));
                toml_at.insert(p.repo, Instant::now());
            }
        }
        let log = agent_files::scan_log(&self.paths.logs());
        let autostart = autostart::is_enabled();
        self.update(|s| {
            s.service = service;
            s.setup = Some(setup);
            s.github = Some(gh);
            s.agent_tomls = tomls;
            s.agent_toml_at = toml_at;
            s.log = Some(log);
            s.autostart = autostart;
            s.slow_refreshed = Some(Local::now());
        });
    }

    fn execute(&mut self, action: Action) {
        *self.shared.busy.lock().expect("busy lock") = Some(action.label());
        (self.on_change)();
        let result = self.perform(&action);
        *self.shared.busy.lock().expect("busy lock") = None;
        let (ok, text) = match result {
            Ok(t) => (true, t),
            Err(e) => (false, e),
        };
        *self.shared.notice.lock().expect("notice lock") = Some(Notice { ok, text, at: Instant::now() });
        self.refresh_fast();
    }

    fn perform(&mut self, action: &Action) -> Result<String, String> {
        let state = self.paths.state();
        let exe = || agent_service::find_exe().ok_or_else(|| "lunima-team-agent nicht gefunden".to_string());
        match action {
            Action::RefreshAll => Ok("Aktualisiert".into()),
            Action::SaveConfig(cfg) => {
                // repos may have changed on disk meanwhile (Projects page, another tool)
                let current = TeamConfig::load(&self.paths.config())?;
                let cfg = (**cfg).clone().with_repos_of(&current);
                cfg.validate().and_then(|_| cfg.save(&self.paths.config())).map(|_| "Gespeichert — gilt ab dem nächsten Zyklus jeder Rolle".into())
            }
            Action::SaveRepos(repos) => {
                let mut cfg = TeamConfig::load(&self.paths.config())?;
                cfg.repos = repos.clone();
                cfg.validate().and_then(|_| cfg.save(&self.paths.config())).map(|_| "Projekte gespeichert".into())
            }
            Action::CreateConfig => crate::core::setup::create_config(&self.paths, team_core::import::legacy_candidates()),
            Action::SavePrompt(path, text) => save_prompt(path, text),
            Action::DeletePrompt(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok("Eingebaute Regeln aktiv (Override gelöscht)".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Eingebaute Regeln sind bereits aktiv".into()),
                Err(e) => Err(e.to_string()),
            },
            Action::SetRolePaused(role, paused) => {
                agent_files::set_pause(&state, role, paused.then_some("Control Center"))?;
                Ok(format!("{} {}", role_label(role), if *paused { "pausiert (ab dem nächsten Zyklus)" } else { "läuft wieder" }))
            }
            Action::PauseAll(paused) => {
                for (role, _) in ROLES {
                    agent_files::set_pause(&state, role, paused.then_some("Control Center"))?;
                }
                Ok(if *paused { "Alle Rollen pausiert (laufende Arbeit wird noch beendet)" } else { "Alle Rollen laufen wieder" }.into())
            }
            Action::Agent(verb) => {
                let info = self.snap().service.and_then(|s| s.ok());
                agent_service::control(*verb, &exe()?, info.as_ref(), &state)?;
                Ok(match verb {
                    Verb::Start => "Team-Agent gestartet",
                    Verb::Stop => "Team-Agent gestoppt",
                    Verb::Restart => "Team-Agent neu gestartet",
                }
                .into())
            }
            Action::SetAgentAutostart(on) => {
                agent_service::set_autostart(*on, &exe()?)?;
                Ok(if *on { "Team-Agent startet ab jetzt mit der Anmeldung" } else { "Autostart des Team-Agents aus" }.into())
            }
            Action::SetAutostart(on) => autostart::set_enabled(*on).map(|_| if *on { "Control Center startet mit der Anmeldung" } else { "Autostart aus" }.into()),
            Action::Terminal(command) => probes::open_terminal(command).map(|_| format!("Terminal geöffnet: {command}")),
        }
    }
}

fn save_prompt(path: &std::path::Path, text: &str) -> Result<String, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(format!("{} gespeichert", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}
