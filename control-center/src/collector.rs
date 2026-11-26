//! Background workers: one keeps the file-based part of the snapshot fresh and executes
//! user actions off the UI thread; a second one runs the slow probes (GitHub, tool
//! versions, the scheduled task). They are separate so a click never waits seconds for a
//! probe that happens to be running.

use crate::core::agent_files;
use crate::core::logs::{self, PassCache};
use crate::core::snapshot::Snapshot;
use crate::core::team::{role_label, ROLES};
use crate::sys::agent_service::{self, Verb};
use crate::sys::{autostart, github, probes};
use chrono::Local;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use team_core::config::TeamConfig;
use team_core::paths::Paths;

#[cfg(test)]
#[path = "collector_tests.rs"]
mod tests;

const FAST: Duration = Duration::from_secs(5);
const SLOW: Duration = Duration::from_secs(90);
const PASSES_SHOWN: usize = 40;
const HISTORY_SHOWN: usize = 60;
const TOML_REFRESH: Duration = Duration::from_secs(600);
const CHAT_SHOWN: usize = 60;

#[derive(Debug, Clone)]
pub enum Action {
    /// Completion barrier for queued settings before the updater exits the UI.
    Barrier(Sender<Result<(), String>>, Arc<std::sync::atomic::AtomicBool>),
    RefreshAll,
    /// projects page: only the repository list
    AutosaveRepos(crate::core::project_autosave::ChangeSet, Sender<Result<Vec<team_core::config::RepoConfig>, String>>),
    /// Drain preceding saves before a normal quit; does not enter updater mode.
    Flush(Sender<Result<(), String>>),
    /// import legacy settings (.env / agent-loop.json) and write team.toml
    CreateConfig,
    SavePrompt(PathBuf, String),
    /// Back to the built-in prompt: remove the override file.
    DeletePrompt(PathBuf),
    SetRolePaused(String, bool),
    /// one setting of team.toml by dotted path (`roles.coder`), saved at once
    SetSetting(String, String),
    /// Max writes to a role (team page); the team agent's chat worker answers
    SendChat(String, String),
    PauseAll(bool),
    Agent(Verb),
    SetAgentAutostart(bool),
    SetAutostart(bool),
    Terminal(&'static str),
    /// `lunima-team-agent once <role>` in a visible terminal
    RunOnce(&'static str),
    /// asks the running daemon to start the weekly discovery now (trigger file)
    RequestDiscoveryRun,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PersistenceKey {
    Setting(String),
    Repos,
    Config,
    Prompt(PathBuf),
    RolePause(String),
    PauseAll,
    Chat(String),
    AgentAutostart,
    Autostart,
    DiscoveryRun,
}

impl Action {
    /// Identify the saved target, rather than the value being retried.
    fn persistence_key(&self) -> Option<PersistenceKey> {
        Some(match self {
            Self::SetSetting(path, _) => PersistenceKey::Setting(path.clone()),
            Self::AutosaveRepos(..) => PersistenceKey::Repos,
            Self::CreateConfig => PersistenceKey::Config,
            Self::SavePrompt(path, _) | Self::DeletePrompt(path) => PersistenceKey::Prompt(path.clone()),
            Self::SetRolePaused(role, _) => PersistenceKey::RolePause(role.clone()),
            Self::PauseAll(_) => PersistenceKey::PauseAll,
            Self::SendChat(role, _) => PersistenceKey::Chat(role.clone()),
            Self::SetAgentAutostart(_) => PersistenceKey::AgentAutostart,
            Self::SetAutostart(_) => PersistenceKey::Autostart,
            Self::RequestDiscoveryRun => PersistenceKey::DiscoveryRun,
            Self::Barrier(..) | Self::Flush(..) | Self::RefreshAll | Self::Agent(_) | Self::Terminal(_) | Self::RunOnce(_) => return None,
        })
    }

    /// Short German label for the "busy" toast.
    fn label(&self) -> String {
        match self {
            Action::Agent(v) => format!("Team-Agent {v:?}"),
            Action::SetSetting(..) => "Einstellungen speichern".into(),
            Action::SendChat(..) => "Nachricht senden".into(),
            Action::AutosaveRepos(..) => "Projekte speichern".into(),
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
    pub update_preparing: std::sync::atomic::AtomicBool,
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

    /// An action failed after `t`: a switch shown ahead of the file goes back at once.
    pub fn failed_since(&self, t: Instant) -> bool {
        self.shared.notice.lock().expect("notice lock").as_ref().is_some_and(|n| !n.ok && n.at > t)
    }
}

/// Starts the worker thread. `on_change` is called after every update (repaint + tray).
pub fn start(paths: Paths, on_change: Arc<dyn Fn() + Send + Sync>) -> Handle {
    let shared = Arc::new(Shared { update_preparing: std::sync::atomic::AtomicBool::new(false), snapshot: Mutex::new(Snapshot::empty(paths.clone())), notice: Mutex::new(None), busy: Mutex::new(None) });
    let (tx, rx) = mpsc::channel();
    let (slow_tx, slow_rx) = mpsc::channel();
    let slow = SlowWorker { shared: shared.clone(), paths: paths.clone(), on_change: on_change.clone() };
    std::thread::Builder::new().name("collector-slow".into()).spawn(move || slow.run(slow_rx)).expect("spawn slow collector");
    let worker = Worker { shared: shared.clone(), paths, cache: PassCache::default(), on_change, slow: slow_tx, persistence_errors: BTreeMap::new() };
    std::thread::Builder::new().name("collector".into()).spawn(move || worker.run(rx)).expect("spawn collector");
    Handle { shared, tx }
}

struct Worker {
    shared: Arc<Shared>,
    paths: Paths,
    cache: PassCache,
    on_change: Arc<dyn Fn() + Send + Sync>,
    /// asks the slow worker for a fresh round (after an action)
    slow: Sender<()>,
    /// A successful action clears only the failure for its own saved target.
    persistence_errors: BTreeMap<PersistenceKey, String>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Action>) {
        // the slow probes read the configuration from the snapshot: their first round has to
        // wait for the first file refresh, or it judges an empty snapshot for 90 s
        self.refresh_fast();
        let _ = self.slow.send(());
        loop {
            self.refresh_fast();
            match rx.recv_timeout(FAST) {
                Ok(action) => {
                    self.execute(action);
                    let _ = self.slow.send(());
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
        let ux = team_core::state::UxState::load(&state);
        let chats = load_chats(&self.paths);
        let discovery = crate::core::discovery::load(&state, config.as_ref().ok());
        self.update(|s| {
            (s.heartbeats, s.pauses, s.history, s.discovered, s.po, s.agent_running) = fresh;
            s.ux = ux;
            s.chats = chats;
            s.discovery = discovery;
            s.config = config;
            s.config_missing = missing;
            s.logs = files;
            s.passes = passes;
            s.agent_exe = agent_service::find_exe();
            s.refreshed = now;
        });
    }


    /// The scheduled task's state right after a change to it, so the switch and the next
    /// action don't wait for the slow round.
    fn refresh_service(&self) {
        let service = agent_service::query();
        self.update(|s| s.service = Some(service));
    }
}

struct SlowWorker {
    shared: Arc<Shared>,
    paths: Paths,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl SlowWorker {
    /// After the first file refresh, then every `SLOW` and right after each action
    /// (several requests in a row: one round).
    fn run(self, rx: Receiver<()>) {
        if rx.recv().is_err() {
            return;
        }
        loop {
            self.refresh_slow();
            match rx.recv_timeout(SLOW) {
                Ok(()) => while rx.try_recv().is_ok() {},
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

    fn refresh_slow(&self) {
        let snap = self.snap();
        let service = Some(agent_service::query());
        let default = team_core::config::TeamConfig::default();
        let setup = crate::core::setup::evaluate_team(&probes::System, !snap.config_missing, snap.cfg().unwrap_or(&default));
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
}

impl Worker {
    fn execute(&mut self, action: Action) {
        *self.shared.busy.lock().expect("busy lock") = Some(action.label());
        (self.on_change)();
        let result = self.perform(&action);
        if let Some(key) = action.persistence_key() {
            match &result {
                Ok(_) => { self.persistence_errors.remove(&key); }
                Err(error) => { self.persistence_errors.insert(key, error.clone()); }
            }
        }
        // An unrelated successful action must not hide an unsaved change.
        let result = result.and_then(|text| self.persistence_result().map(|()| text));
        *self.shared.busy.lock().expect("busy lock") = None;
        let (ok, text) = match result {
            Ok(t) => (true, t),
            Err(e) => (false, e),
        };
        *self.shared.notice.lock().expect("notice lock") = Some(Notice { ok, text, at: Instant::now() });
        self.refresh_fast();
    }

    fn persistence_result(&self) -> Result<(), String> {
        if self.persistence_errors.is_empty() {
            Ok(())
        } else {
            Err(format!("Nicht gespeichert: {}", self.persistence_errors.iter()
                .map(|(key, error)| format!("{key:?}: {error}"))
                .collect::<Vec<_>>().join("; ")))
        }
    }

    fn perform(&mut self, action: &Action) -> Result<String, String> {
        if self.shared.update_preparing.load(std::sync::atomic::Ordering::SeqCst) && matches!(action,Action::Agent(_)|Action::RunOnce(_)|Action::SetAgentAutostart(_)) {
            return Err("Update wird vorbereitet; Agent-Aufgaben werden erst fertiggestellt".into());
        }
        let state = self.paths.state();
        let exe = || agent_service::find_exe().ok_or_else(|| "lunima-team-agent nicht gefunden".to_string());
        match action {
            Action::Flush(done) => {
                let result = self.persistence_result();
                let _ = done.send(result.clone());
                result.map(|_| "Änderungen gespeichert".into())
            }
            Action::Barrier(done,active) => {
                let result = self.persistence_result().and_then(|()| {
                    if !active.load(std::sync::atomic::Ordering::SeqCst) {return Err("Update-Anforderung wurde abgebrochen".into());}
                    self.shared.update_preparing.store(true,std::sync::atomic::Ordering::SeqCst);
                    if !active.load(std::sync::atomic::Ordering::SeqCst) {
                        self.shared.update_preparing.store(false,std::sync::atomic::Ordering::SeqCst);
                        return Err("Update-Anforderung wurde abgebrochen".into());
                    }
                    Ok(())
                });
                let _ = done.send(result.clone());
                result.map(|()| "Einstellungen gespeichert".into())
            },
            Action::RefreshAll => Ok("Aktualisiert".into()),
            Action::AutosaveRepos(changes, done) => {
                let result = (|| {
                    let mut cfg = TeamConfig::load(&self.paths.config())?;
                    cfg.repos = crate::core::project_autosave::merge_repos(&changes.base, &changes.edited, &cfg.repos)?;
                    cfg.validate()?;
                    cfg.save(&self.paths.config())?;
                    let repos = cfg.repos.clone();
                    // Publish before acknowledging: clearing a local draft must never
                    // briefly show the snapshot from before its save.
                    self.update(|s| { s.config = Ok(cfg); s.config_missing = false; });
                    Ok(repos)
                })();
                let _ = done.send(result.clone());
                result.map(|_| "Projekte automatisch gespeichert".into())
            }
            Action::CreateConfig => crate::core::setup::create_config(&self.paths, team_core::import::legacy_candidates()),
            Action::SavePrompt(path, text) => save_prompt(path, text),
            Action::DeletePrompt(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok("Eingebaute Regeln aktiv (Override gelöscht)".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Eingebaute Regeln sind bereits aktiv".into()),
                Err(e) => Err(e.to_string()),
            },
            Action::SetSetting(path, value) => {
                let mut cfg = TeamConfig::load(&self.paths.config())?.with_setting(path, value)?;
                // a product owner without a repository would idle: take the only project
                if path == "roles.product_owner" && cfg.roles.product_owner && cfg.product_owner.repo.is_empty() {
                    if let [only] = cfg.repos.as_slice() {
                        cfg.product_owner.repo = only.name.clone();
                    }
                }
                cfg.save(&self.paths.config())?;
                Ok(setting_text(path, value))
            }
            Action::SendChat(role, text) => {
                let msg = team_core::chat::Message::new(team_core::chat::MAX, role, team_core::chat::Kind::Chat, text);
                team_core::chat::post(&self.paths, &msg)?;
                Ok(if self.snap().agent_running { format!("An {} gesendet", role_label(role)) } else { format!("An {} gesendet — die Antwort kommt, sobald der Team-Agent läuft", role_label(role)) })
            }
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
                // live, not from the snapshot: the slow round may not have seen the last
                // autostart change yet, and a stale "managed" stops the agent for good
                let info = agent_service::query().ok();
                let result = agent_service::control(*verb, &exe()?, info.as_ref(), &state);
                self.refresh_service();
                result?;
                Ok(match verb {
                    Verb::Start => "Team-Agent gestartet",
                    Verb::Stop => "Team-Agent gestoppt",
                    Verb::Restart => "Team-Agent neu gestartet",
                }
                .into())
            }
            Action::SetAgentAutostart(on) => {
                let result = agent_service::set_autostart(*on, &exe()?);
                self.refresh_service();
                result?;
                Ok(if *on { "Team-Agent startet ab jetzt mit der Anmeldung" } else { "Autostart des Team-Agents aus" }.into())
            }
            Action::SetAutostart(on) => autostart::set_enabled(*on).map(|_| if *on { "Control Center startet mit der Anmeldung" } else { "Autostart aus" }.into()),
            Action::Terminal(command) => probes::open_terminal(command).map(|_| format!("Terminal geöffnet: {command}")),
            Action::RequestDiscoveryRun => {
                let dir = team_core::state::DiscoveryState::dir(&state);
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                std::fs::write(dir.join(team_core::state::DISCOVERY_RUN_NOW_FILE), "").map_err(|e| e.to_string())?;
                Ok("Discovery startet beim nächsten Zyklus des Team-Agents".into())
            }
            Action::RunOnce(role) => probes::open_terminal(&agent_service::once_command(&exe()?, role)).map(|_| format!("{} läuft im Terminal", role_label(role))),
        }
    }
}

fn load_chats(paths: &Paths) -> std::collections::BTreeMap<String, crate::core::snapshot::ChatView> {
    use team_core::chat;
    chat::ROLES
        .iter()
        .map(|(role, _)| {
            let all = chat::read(paths, role);
            let view = crate::core::snapshot::ChatView { unread: chat::unread_for_max(&all, role), busy: chat::is_busy(paths, role), messages: all[all.len().saturating_sub(CHAT_SHOWN)..].to_vec() };
            (role.to_string(), view)
        })
        .collect()
}

fn setting_text(path: &str, value: &str) -> String {
    match path.strip_prefix("roles.") {
        Some(role) => format!("{} {}", role_label(&role.replace('_', "-")), if value == "true" { "eingeschaltet" } else { "ausgeschaltet" }),
        None => "Gespeichert — gilt ab dem nächsten Zyklus".into(),
    }
}

fn save_prompt(path: &std::path::Path, text: &str) -> Result<String, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(format!("{} gespeichert", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}
