use super::{
    cache, install,
    model::{self, Pending},
    Status,
};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub enum Command {
    Check,
    Install,
    Cancel,
}
pub struct Worker {
    pub state: Arc<Mutex<Status>>,
    pub tx: mpsc::Sender<Command>,
    home: PathBuf,
    pub eligibility: Arc<Mutex<Option<Result<(), String>>>>,
}

impl Worker {
    #[cfg(test)]
    pub(super) fn fixture(status: Status) -> Self {
        let (tx, _rx) = mpsc::channel();
        Self {
            state: Arc::new(Mutex::new(status)),
            tx,
            home: PathBuf::from("fixture"),
            eligibility: Arc::new(Mutex::new(Some(Ok(())))),
        }
    }
    pub fn start(home: PathBuf, barrier: crate::collector::Handle) -> Self {
        let state = Arc::new(Mutex::new(Status::Checking));
        let (tx, rx) = mpsc::channel();
        let shared = state.clone();
        let eligibility = Arc::new(Mutex::new(None));
        let eligible = eligibility.clone();
        let worker_home = home.clone();
        std::thread::spawn(move || {
            *eligible.lock().unwrap_or_else(|p| p.into_inner()) = Some(install::supported());
            run(worker_home, shared, rx, barrier)
        });
        Self {
            state,
            tx,
            home,
            eligibility,
        }
    }
    pub fn status(&self) -> Status {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn send(&self, c: Command) {
        if matches!(c, Command::Cancel) {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if matches!(*state, Status::Waiting(_)) {
                let _ = self.tx.send(c);
            }
            return;
        }
        let _ = self.tx.send(c);
    }
    pub fn home(&self) -> PathBuf {
        self.home.clone()
    }
}

fn put(state: &Mutex<Status>, s: Status) {
    *state.lock().unwrap_or_else(|p| p.into_inner()) = s;
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn run(
    home: PathBuf,
    state: Arc<Mutex<Status>>,
    rx: mpsc::Receiver<Command>,
    barrier: crate::collector::Handle,
) {
    let mut pending = match cache::load(&home) {
        Ok(p) => p,
        Err(e) => {
            put(
                &state,
                Status::Error(format!("Vorbereitetes Update ungültig: {e}")),
            );
            None
        }
    };
    if let Some(p) = &pending {
        put(&state, Status::Ready(p.version.clone()));
        if !cache::root(&home).join("install-failed").exists() && install::supported().is_ok() {
            if let Err(e) = apply(&home, p, &state, &rx, &barrier) {
                put(&state, Status::Error(e));
            }
        }
    }
    let mut first = pending.is_none();
    loop {
        let command = if first {
            first = false;
            Command::Check
        } else {
            match rx.recv_timeout(Duration::from_secs(3600)) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => Command::Check,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };
        match command {
            Command::Check => {
                put(&state, Status::Checking);
                match check_and_download(&home, &state) {
                    Ok(Some(p)) => {
                        put(&state, Status::Ready(p.version.clone()));
                        pending = Some(p);
                    }
                    Ok(None) => put(
                        &state,
                        match &pending {
                            Some(p) => Status::Ready(p.version.clone()),
                            None => Status::Current,
                        },
                    ),
                    Err(e) => put(&state, Status::Error(format!("Update-Prüfung: {e}"))),
                }
            }
            Command::Install => {
                if let Some(p) = &pending {
                    if let Err(e) = apply(&home, p, &state, &rx, &barrier) {
                        put(&state, Status::Error(e));
                    }
                } else {
                    put(&state,Status::Error("Kein Update vorbereitet. Bitte zuerst erneut prüfen.".into()));
                }
            }
            Command::Cancel => {
                if let Some(p) = &pending {
                    put(&state, Status::Ready(p.version.clone()));
                }
            }
        }
    }
}

fn check_and_download(home: &Path, state: &Mutex<Status>) -> Result<Option<Pending>, String> {
    let Some(p) = cache::check()? else {
        return Ok(None);
    };
    let path = p.path(
        &cache::root(home),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    if model::verify(&path, p.size, &p.sha256).is_err() {
        put(state, Status::Downloading(p.version.clone()));
        cache::download(home, &p)?;
    } else {
        team_core::state::write_atomic(
            &cache::root(home).join("pending.json"),
            &serde_json::to_vec(&p).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(Some(p))
}

fn apply(
    home: &Path,
    p: &Pending,
    state: &Mutex<Status>,
    rx: &mpsc::Receiver<Command>,
    barrier: &crate::collector::Handle,
) -> Result<(), String> {
    struct ReleasePreparation<'a>(&'a std::sync::atomic::AtomicBool,Arc<std::sync::atomic::AtomicBool>);
    impl Drop for ReleasePreparation<'_> {
        fn drop(&mut self) {self.1.store(false,std::sync::atomic::Ordering::SeqCst);self.0.store(false,std::sync::atomic::Ordering::SeqCst);}
    }
    let active=Arc::new(std::sync::atomic::AtomicBool::new(true));
    let _preparation=ReleasePreparation(&barrier.shared.update_preparing,active.clone());
    install::supported()?;
    let package = p.path(
        &cache::root(home),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    model::verify(&package, p.size, &p.sha256)?;
    let (done, wait) = mpsc::channel();
    barrier.send(crate::collector::Action::Barrier(done,active));
    wait.recv_timeout(Duration::from_secs(30))
        .map_err(|_| "Einstellungen werden noch gespeichert; bitte erneut versuchen")??;
    let dir = home.join("state");
    let token = format!("{}-{}", std::process::id(), now());
    let request = dir.join("update-drain.json");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let agent =
        crate::core::agent_files::agent_pid(&dir).filter(|p| crate::sys::process::is_agent(*p));
    let write_request = || {
        team_core::state::write_atomic(
            &request,
            serde_json::json!({"token":token,"requested_at":now()})
                .to_string()
                .as_bytes(),
        )
        .map_err(|e| e.to_string())
    };
    write_request()?;
    put(state, Status::Waiting(p.version.clone()));
    let start = Instant::now();
    let mut refreshed = Instant::now();
    let mut postponed = false;
    let outcome = (|| {
        if let Some(pid) = agent {
            loop {
                if !crate::sys::process::is_agent(pid) {
                    // Acknowledgement is published immediately before exit. Read after
                    // observing exit so the two checks cannot straddle that publication.
                    let drained=acknowledged(&dir,&token,pid);
                    if !drained {
                        return Err(
                            "Agent wurde ohne Update-Bestätigung beendet. Bitte erneut versuchen."
                                .into(),
                        );
                    }
                    break;
                }
                if refreshed.elapsed() > Duration::from_secs(10) {
                    write_request()?;
                    refreshed = Instant::now();
                }
                if start.elapsed() > Duration::from_secs(24 * 3600) {
                    return Err("Agent-Aufgaben laufen noch. Update bleibt vorbereitet.".into());
                }
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(Command::Cancel) => {
                        postponed = true;
                        put(state, Status::Postponing);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("Update verschoben; Control Center wurde beendet.".into())
                    }
                    _ => {}
                }
            }
        }
        model::verify(&package, p.size, &p.sha256)?;
        // Freeze cancellation under the same lock used by the button's sender. Every
        // accepted postpone request is consumed before a helper can be launched.
        {
            let mut view = state.lock().unwrap_or_else(|p| p.into_inner());
            postponed |= rx.try_iter().any(|c| matches!(c, Command::Cancel));
            if !postponed {
                *view = Status::Handoff;
            }
        }
        if postponed {
            return Err("Update verschoben; das Team wird wieder gestartet.".into());
        }
        install::launch(&package, home, &token, agent.is_some())?;
        put(state, Status::Installing);
        // Hidden tray windows may stop painting. Handoff must not depend on another UI frame.
        // The collector barrier above has already persisted pending settings.
        std::process::exit(0)
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(request);
        // A helper launch failure after successful drain must not leave the team stopped.
        if agent.is_some_and(|pid| !crate::sys::process::is_agent(pid)) {
            if let Some(exe) = crate::sys::agent_service::find_exe() {
                let _ = crate::sys::agent_service::control(
                    crate::sys::agent_service::Verb::Start,
                    &exe,
                    None,
                    &dir,
                );
            }
        }
    }
    outcome
}

fn acknowledged(dir:&Path,token:&str,pid:u32)->bool {
    std::fs::read(dir.join("update-drained.json")).ok()
        .and_then(|b|serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v|v["token"].as_str()==Some(token)&&v["pid"].as_u64()==Some(pid as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_matching_token_and_pid_acknowledge_the_drain() {
        let dir=tempfile::tempdir().unwrap();let file=dir.path().join("update-drained.json");
        assert!(!acknowledged(dir.path(),"current",42));
        for value in ["{}",r#"{"token":"old","pid":42}"#,r#"{"token":"current","pid":41}"#] {
            std::fs::write(&file,value).unwrap();assert!(!acknowledged(dir.path(),"current",42));
        }
        std::fs::write(file,r#"{"token":"current","pid":42}"#).unwrap();
        assert!(acknowledged(dir.path(),"current",42));
    }
}
