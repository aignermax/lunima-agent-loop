//! UI atomically writes `state/update-drain.json`: `{token, requested_at}` (Unix
//! seconds), renewing the same token every 10 seconds. Requests aged 0..=120 seconds
//! close daemon admission. Existing jobs finish; manual commands only check admission.
//! After joining all workers, the supervisor revalidates the lease and atomically
//! writes `update-drained.json`: `{token, pid}`, then exits successfully. Cancellation
//! before finalization rebuilds workers in the same process. UI must match token AND
//! pid and wait for process exit before installing, holding `state/agent.lock`.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct State {
    token: Option<String>,
    active: usize,
}

pub(crate) struct Drain {
    state_dir: PathBuf,
    state: Mutex<State>,
}

pub(crate) struct Work(Arc<Drain>);

impl Drop for Work {
    fn drop(&mut self) {
        self.0.state.lock().unwrap_or_else(|p| p.into_inner()).active -= 1;
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn parse_request(raw: &str, now: u64) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Request {
        token: String,
        requested_at: u64,
    }
    let request: Request = serde_json::from_str(raw).ok()?;
    let age = now.checked_sub(request.requested_at)?;
    (age <= 120 && !request.token.trim().is_empty()).then_some(request.token)
}

pub(crate) fn pending(state_dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(state_dir.join("update-drain.json")).ok().and_then(|raw| parse_request(&raw, unix_seconds()))
}

impl Drain {
    pub(crate) fn new(state_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { state_dir, state: Mutex::new(State::default()) })
    }

    fn refresh(&self, state: &mut State) {
        if state.token.is_none() {
            state.token = pending(&self.state_dir);
        }
    }

    pub(crate) fn try_start(self: &Arc<Self>) -> Option<Work> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.refresh(&mut state);
        if state.token.is_some() {
            return None;
        }
        state.active += 1;
        Some(Work(self.clone()))
    }

    pub(crate) fn draining(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.refresh(&mut state);
        state.token.is_some()
    }

    /// Returns true on drain, including during login retries and role backoff.
    pub(crate) fn wait(&self, duration: Duration) -> bool {
        let start = Instant::now();
        loop {
            if self.draining() {
                return true;
            }
            let remaining = duration.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(200)));
        }
    }

    /// Only the supervisor calls this, after joining every worker thread.
    pub(crate) fn acknowledge(&self) -> io::Result<bool> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let token = state.token.as_ref().filter(|_| state.active == 0).ok_or_else(|| io::Error::other("daemon is not drained"))?;
        // Cancellation, lease expiry, and a replacement token require a fresh worker
        // generation. Old workers stay closed until they have all been joined.
        if pending(&self.state_dir).as_ref() != Some(token) {
            return Ok(false);
        }
        let ack = serde_json::json!({"token": token, "pid": std::process::id()});
        team_core::state::write_atomic(&self.state_dir.join("update-drained.json"), ack.to_string().as_bytes())?;
        if pending(&self.state_dir).as_ref() != Some(token) {
            // Cancellation must resume workers even if the UI already removed the ack.
            let _ = std::fs::remove_file(self.state_dir.join("update-drained.json"));
            return Ok(false);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(dir: &std::path::Path, token: &str, requested_at: u64) {
        std::fs::write(dir.join("update-drain.json"), json!({"token": token, "requested_at": requested_at}).to_string()).unwrap();
    }

    #[test]
    fn malformed_stale_and_future_requests_do_not_block_work() {
        let dir = tempfile::tempdir().unwrap();
        let now = unix_seconds();
        let drain = Drain::new(dir.path().to_owned());
        for raw in [
            "{".to_string(),
            r#"{"token":"t"}"#.to_string(),
            json!({"token": 7, "requested_at": now}).to_string(),
            json!({"token": " ", "requested_at": now}).to_string(),
            json!({"token": "t", "requested_at": -1}).to_string(),
            json!({"token": "t", "requested_at": now - 121}).to_string(),
            json!({"token": "t", "requested_at": now + 60}).to_string(),
        ] {
            std::fs::write(dir.path().join("update-drain.json"), raw).unwrap();
            assert!(drain.try_start().is_some());
            assert!(!drain.draining());
        }
        assert!(drain.acknowledge().is_err());
        assert!(!dir.path().join("update-drained.json").exists());
    }

    #[test]
    fn request_boundary_is_120_seconds() {
        assert_eq!(parse_request(r#"{"token":"boundary","requested_at":880}"#, 1000), Some("boundary".into()));
        assert_eq!(parse_request(r#"{"token":"expired","requested_at":879}"#, 1000), None);
    }

    #[test]
    fn drain_blocks_new_work_and_ack_waits_for_every_inflight_job() {
        let dir = tempfile::tempdir().unwrap();
        let drain = Drain::new(dir.path().to_owned());
        let role = drain.try_start().unwrap();
        let chat = drain.try_start().unwrap();
        request(dir.path(), "update-42", unix_seconds());
        assert!(drain.try_start().is_none());
        assert!(drain.acknowledge().is_err());
        drop(role);
        assert!(drain.acknowledge().is_err());
        assert!(!dir.path().join("update-drained.json").exists());
        drop(chat);
        drain.acknowledge().unwrap();
        let ack: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.path().join("update-drained.json")).unwrap()).unwrap();
        assert_eq!(ack, json!({"token": "update-42", "pid": std::process::id()}));
        assert!(drain.try_start().is_none(), "acknowledgement must never reopen admission");
    }

    #[test]
    fn removed_request_keeps_old_workers_closed_but_cancels_ack() {
        let dir = tempfile::tempdir().unwrap();
        let drain = Drain::new(dir.path().to_owned());
        request(dir.path(), "original", unix_seconds());
        assert!(drain.draining());
        std::fs::remove_file(dir.path().join("update-drain.json")).unwrap();
        assert!(drain.try_start().is_none());
        assert!(!drain.acknowledge().unwrap());
        assert!(!dir.path().join("update-drained.json").exists());
        let restarted = Drain::new(dir.path().to_owned());
        assert!(restarted.try_start().is_some());
    }

    #[test]
    fn replaced_or_expired_request_cannot_ack_old_token() {
        for replacement in ["different", "original"] {
            let dir = tempfile::tempdir().unwrap();
            let drain = Drain::new(dir.path().to_owned());
            request(dir.path(), "original", unix_seconds());
            assert!(drain.draining());
            request(dir.path(), replacement, if replacement == "original" { unix_seconds() - 121 } else { unix_seconds() });
            assert!(!drain.acknowledge().unwrap());
            assert!(!dir.path().join("update-drained.json").exists());
        }
    }

    #[test]
    fn refreshed_request_is_acknowledged_after_long_work() {
        let dir = tempfile::tempdir().unwrap();
        let drain = Drain::new(dir.path().to_owned());
        let work = drain.try_start().unwrap();
        request(dir.path(), "long-job", unix_seconds());
        assert!(drain.draining());
        request(dir.path(), "long-job", unix_seconds() - 121);
        assert!(drain.acknowledge().is_err());
        request(dir.path(), "long-job", unix_seconds());
        drop(work);
        assert!(drain.acknowledge().unwrap());
    }

    #[test]
    fn long_wait_wakes_promptly_for_update() {
        let dir = tempfile::tempdir().unwrap();
        let drain = Drain::new(dir.path().to_owned());
        let waiting = drain.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || tx.send(waiting.wait(std::time::Duration::from_secs(600))).unwrap());
        request(dir.path(), "wake", unix_seconds());
        assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn concurrent_admission_cannot_race_acknowledgement() {
        let dir = tempfile::tempdir().unwrap();
        let drain = Drain::new(dir.path().to_owned());
        let current = drain.try_start().unwrap();
        request(dir.path(), "race", unix_seconds());
        std::thread::scope(|scope| {
            for _ in 0..12 {
                let drain = &drain;
                scope.spawn(move || assert!(drain.try_start().is_none()));
            }
        });
        assert!(drain.acknowledge().is_err());
        drop(current);
        drain.acknowledge().unwrap();
    }
}
