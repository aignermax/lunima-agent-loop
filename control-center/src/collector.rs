//! Background worker: keeps the snapshot fresh and executes user actions off the UI thread.

use crate::core::config::ConfigDoc;
use crate::core::logs::{self, PassCache};
use crate::core::root::CONFIG_FILE;
use crate::core::snapshot::Snapshot;
use crate::core::state::LoopState;
use crate::sys::{autostart, cli, git, github, schedtask};
use chrono::Local;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FAST: Duration = Duration::from_secs(5);
const SLOW: Duration = Duration::from_secs(90);
const PASSES_SHOWN: usize = 40;

#[derive(Debug, Clone)]
pub enum Action {
    Pause,
    Resume,
    RunOwner,
    SetTaskEnabled(bool),
    EnableConfig,
    RescueWip,
    SetAutostart(bool),
    SaveConfig(ConfigDoc),
    SavePrompt(PathBuf, String),
    /// Back to the built-in prompt: remove the override file.
    DeletePrompt(PathBuf),
    RefreshAll,
    /// issue-agent role + systemctl verb (start/stop/restart)
    Unit(String, &'static str),
    SetRolePaused(String, bool),
    /// boolean in agent-loop.json, e.g. workersEnabled
    SetLoopFlag(&'static str, bool),
    SetAgentDir(PathBuf),
    /// issue agent .env; true = restart its daemons afterwards
    SaveEnv(crate::core::envfile::EnvFile, bool),
}

impl Action {
    /// Short German label for the "busy" toast.
    fn label(&self) -> String {
        match self {
            Action::Unit(role, verb) => format!("{} {verb}", crate::core::team::role_label(role)),
            Action::SaveEnv(..) => ".env speichern".into(),
            Action::SaveConfig(_) => "Einstellungen speichern".into(),
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
pub fn start(root: PathBuf, cli_path: Option<PathBuf>, on_change: Arc<dyn Fn() + Send + Sync>) -> Handle {
    let shared = Arc::new(Shared {
        snapshot: Mutex::new(Snapshot::empty(root.clone(), cli_path.clone())),
        notice: Mutex::new(None),
        busy: Mutex::new(None),
    });
    let (tx, rx) = mpsc::channel();
    let worker = Worker { shared: shared.clone(), root, cli: cli_path, cache: PassCache::default(), on_change };
    std::thread::Builder::new().name("collector".into()).spawn(move || worker.run(rx)).expect("spawn collector");
    Handle { shared, tx }
}

struct Worker {
    shared: Arc<Shared>,
    root: PathBuf,
    cli: Option<PathBuf>,
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

    fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.shared.snapshot.lock().expect("snapshot lock"));
        (self.on_change)();
    }

    fn refresh_fast(&mut self) {
        let config = ConfigDoc::load(&self.root.join(CONFIG_FILE));
        let state = LoopState::load(&self.root.join("state").join("state.json"));
        let files = logs::list(&self.root.join("logs"));
        let mut passes = self.cache.passes(&files, PASSES_SHOWN);
        if let Ok(st) = &state {
            logs::attach_records(&mut passes, &st.last_runs);
        }
        let running = cli::loop_running();
        let prev_team = self.shared.snapshot.lock().expect("snapshot lock").team.clone();
        let team = crate::team_ops::refresh_fast(&self.root, &prev_team);
        self.update(|s| {
            s.team = team;
            s.config = config;
            s.state = state;
            s.logs = files;
            s.passes = passes;
            s.loop_running = running;
            s.refreshed = Local::now();
        });
    }

    fn refresh_slow(&mut self) {
        let snap = self.shared.snapshot.lock().expect("snapshot lock").clone();
        let task = schedtask::query();
        let clone = snap.clone_path().map(|p| git::status(&p)).unwrap_or_else(|| Err("clonePath fehlt in der Konfiguration".into()));
        let gh_auth = github::auth_status();
        let repo = snap.config_str("githubRepo");
        let gh_data = if repo.is_empty() { Err("githubRepo fehlt".into()) } else { github::fetch(&repo) };
        let autostart = autostart::is_enabled();
        let mut team = snap.team.clone();
        crate::team_ops::refresh_slow(&mut team);
        self.update(|s| {
            s.team.units = team.units;
            s.team.claude_version = team.claude_version;
            s.team.wsl_offset = team.wsl_offset;
            s.team.log = team.log;
            s.task = Some(task);
            s.clone = Some(clone);
            s.gh_auth = Some(gh_auth);
            s.github = Some(gh_data);
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
        let cli = || self.cli.clone().ok_or_else(|| "lunima-agent-loop nicht gefunden".to_string());
        match action {
            Action::Pause => cli::run_cli(&cli()?, &self.root, &["pause", "Control Center"]).map(|_| "Loop pausiert".into()),
            Action::Resume => cli::run_cli(&cli()?, &self.root, &["resume"]).map(|_| "Loop läuft wieder".into()),
            // the loop has no locking: two concurrent passes would fight over clone and state
            Action::RunOwner if cli::loop_running() => Err("Es läuft bereits ein Loop-Prozess — bitte warten.".into()),
            Action::RunOwner => cli::spawn_cli(&cli()?, &self.root, &["own"]).map(|_| "PO-Lauf gestartet — Fortschritt unter Aktivität".into()),
            Action::SetTaskEnabled(on) => schedtask::set_enabled(*on).map(|_| if *on { "Zeitplan eingeschaltet" } else { "Zeitplan ausgeschaltet" }.into()),
            Action::EnableConfig => self.enable_config(),
            Action::RescueWip if cli::loop_running() => Err("Ein Lauf arbeitet gerade im Clone — nichts angefasst.".into()),
            Action::RescueWip => self.rescue(),
            Action::SetAutostart(on) => autostart::set_enabled(*on, &self.root).map(|_| if *on { "Startet mit Windows" } else { "Autostart aus" }.into()),
            Action::SaveConfig(doc) => doc.validate().and_then(|_| doc.save()).map(|_| "Einstellungen gespeichert".into()),
            Action::SavePrompt(path, text) => save_prompt(path, text),
            Action::DeletePrompt(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok("Eingebaute Regeln aktiv (Override gelöscht)".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Eingebaute Regeln sind bereits aktiv".into()),
                Err(e) => Err(e.to_string()),
            },
            Action::RefreshAll => Ok("Aktualisiert".into()),
            Action::Unit(role, verb) => crate::team_ops::unit(role, verb),
            Action::SetRolePaused(role, paused) => crate::team_ops::set_paused(&self.root, role, *paused),
            Action::SetLoopFlag(key, value) => crate::team_ops::set_loop_flag(&self.root, key, *value),
            Action::SetAgentDir(path) => crate::team_ops::set_agent_dir(&self.root, path),
            Action::SaveEnv(env, restart) => crate::team_ops::save_env(env, *restart),
        }
    }

    fn enable_config(&self) -> Result<String, String> {
        let mut doc = ConfigDoc::load(&self.root.join(CONFIG_FILE))?;
        let spec = crate::core::config::FIELDS.iter().find(|f| f.key == "enabled").ok_or("enabled fehlt")?;
        doc.set_from_text(spec, "true")?;
        doc.save().map(|_| "Loop aktiviert".into())
    }

    fn rescue(&self) -> Result<String, String> {
        let clone = self.shared.snapshot.lock().expect("snapshot lock").clone_path().ok_or("clonePath fehlt")?;
        git::rescue_wip(&clone).map(|b| format!("Änderungen gesichert auf lokalem Branch {b}"))
    }
}

fn save_prompt(path: &std::path::Path, text: &str) -> Result<String, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(format!("{} gespeichert", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}
