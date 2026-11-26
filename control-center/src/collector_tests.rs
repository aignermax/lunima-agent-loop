use super::*;

#[test]
fn project_autosave_acknowledges_persistence_and_preserves_newer_file_fields() {
    let temp = tempfile::tempdir().unwrap(); let mut worker = worker(temp.path());
    let mut cfg = TeamConfig::default();
    cfg.repos.push(team_core::config::RepoConfig { name: "o/r".into(), ..Default::default() });
    cfg.save(&worker.paths.config()).unwrap();
    let mut edited = cfg.repos.clone(); edited[0].routing = team_core::config::Routing::Codex;
    let changes = crate::core::project_autosave::ChangeSet { base: cfg.repos.clone(), edited };
    cfg.repos[0].coder = false;
    cfg.runner.model = "preserve-global".into();
    cfg.save(&worker.paths.config()).unwrap();
    let (done, wait) = mpsc::channel();
    worker.execute(Action::AutosaveRepos(changes, done));
    let acknowledged = wait.recv().unwrap().unwrap();
    let saved = TeamConfig::load(&worker.paths.config()).unwrap();
    assert_eq!(saved.repos, acknowledged);
    assert_eq!(saved.repos[0].routing, team_core::config::Routing::Codex);
    assert!(!saved.repos[0].coder);
    assert_eq!(saved.runner.model, "preserve-global");
    assert_eq!(worker.snap().cfg().unwrap().repos, acknowledged);
    assert_eq!(barrier(&mut worker), Ok(()));
}

#[test]
fn normal_quit_flush_waits_for_saves_and_reports_failures_without_updater_mode() {
    let temp = tempfile::tempdir().unwrap(); let mut worker = worker(temp.path());
    let changes = crate::core::project_autosave::ChangeSet { base: Vec::new(), edited: Vec::new() };
    let (done, saved) = mpsc::channel();
    worker.execute(Action::AutosaveRepos(changes.clone(), done));
    assert!(saved.recv().unwrap().is_err());
    let (done, flushed) = mpsc::channel(); worker.execute(Action::Flush(done));
    assert!(flushed.recv().unwrap().is_err());
    TeamConfig::default().save(&worker.paths.config()).unwrap();
    let (done, saved) = mpsc::channel(); worker.execute(Action::AutosaveRepos(changes, done));
    assert!(saved.recv().unwrap().is_ok());
    let (done, flushed) = mpsc::channel(); worker.execute(Action::Flush(done));
    assert_eq!(flushed.recv().unwrap(), Ok(()));
    assert!(!worker.shared.update_preparing.load(std::sync::atomic::Ordering::SeqCst));
}

fn worker(root: &std::path::Path) -> Worker {
    let paths = Paths::new(root);
    let shared = Arc::new(Shared {
        update_preparing:std::sync::atomic::AtomicBool::new(false),
        snapshot: Mutex::new(Snapshot::empty(paths.clone())),
        notice: Mutex::new(None),
        busy: Mutex::new(None),
    });
    let (slow, _) = mpsc::channel();
    Worker { shared, paths, cache: PassCache::default(), on_change: Arc::new(|| {}), slow, persistence_errors: BTreeMap::new() }
}

#[test]
fn update_barrier_blocks_queued_tray_restart_stop_and_manual_cycles() {
    let temp=tempfile::tempdir().unwrap();let mut w=worker(temp.path());
    assert_eq!(barrier(&mut w),Ok(()));
    for action in [Action::Agent(Verb::Start),Action::Agent(Verb::Stop),Action::Agent(Verb::Restart),Action::RunOnce("coder"),Action::SetAgentAutostart(false),Action::SetAgentAutostart(true)] {
        assert!(w.perform(&action).unwrap_err().contains("Update wird vorbereitet"));
    }
}

#[test]
fn expired_barrier_cannot_lock_the_team_after_requester_stopped_waiting() {
    let temp=tempfile::tempdir().unwrap();let mut w=worker(temp.path());
    let(done,wait)=mpsc::channel();
    w.execute(Action::Barrier(done,Arc::new(std::sync::atomic::AtomicBool::new(false))));
    assert!(wait.recv().unwrap().is_err());
    assert!(!w.shared.update_preparing.load(std::sync::atomic::Ordering::SeqCst));
}

fn barrier(worker: &mut Worker) -> Result<(), String> {
    let (done, wait) = mpsc::channel();
    worker.execute(Action::Barrier(done,Arc::new(std::sync::atomic::AtomicBool::new(true))));
    wait.recv_timeout(Duration::from_secs(1)).unwrap()
}

#[test]
fn barrier_reports_failed_save_without_replacing_its_error_notice() {
    let temp = tempfile::tempdir().unwrap();
    let mut worker = worker(temp.path());
    worker.execute(Action::SetSetting("roles.coder".into(), "false".into()));
    let failure = worker.shared.notice.lock().unwrap().clone().unwrap();
    assert!(!failure.ok);

    assert!(barrier(&mut worker).is_err());
    let notice = worker.shared.notice.lock().unwrap().clone().unwrap();
    assert!(!notice.ok);
    assert!(notice.text.contains(&failure.text));
    assert!(barrier(&mut worker).is_err(), "barriers must not consume failures");
}

#[test]
fn successful_other_action_does_not_clear_an_outstanding_save_failure() {
    let temp = tempfile::tempdir().unwrap();
    let mut worker = worker(temp.path());
    worker.execute(Action::SetSetting("roles.coder".into(), "false".into()));
    worker.execute(Action::RefreshAll);
    assert!(barrier(&mut worker).is_err());
    assert!(!worker.shared.notice.lock().unwrap().as_ref().unwrap().ok);
}

#[test]
fn every_failed_setting_requires_a_successful_retry_of_its_own_key() {
    let temp = tempfile::tempdir().unwrap();
    let mut worker = worker(temp.path());
    worker.execute(Action::SetSetting("roles.coder".into(), "false".into()));
    worker.execute(Action::SetSetting("roles.qa".into(), "false".into()));
    TeamConfig::default().save(&worker.paths.config()).unwrap();

    worker.execute(Action::SetSetting("roles.coder".into(), "false".into()));
    assert!(barrier(&mut worker).is_err());
    worker.execute(Action::SetSetting("roles.qa".into(), "false".into()));
    assert_eq!(barrier(&mut worker), Ok(()));
    let saved = TeamConfig::load(&worker.paths.config()).unwrap();
    assert!(!saved.roles.coder && !saved.roles.qa);
}

#[test]
fn prompt_failure_survives_another_prompt_save_and_clears_on_retry() {
    let temp = tempfile::tempdir().unwrap();
    let mut worker = worker(temp.path());
    let blocked = temp.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let prompt = blocked.join("rules.md");
    worker.execute(Action::SavePrompt(prompt.clone(), "new rules".into()));
    worker.execute(Action::SavePrompt(temp.path().join("other.md"), "other rules".into()));
    assert!(barrier(&mut worker).is_err());

    std::fs::remove_file(blocked).unwrap();
    worker.execute(Action::SavePrompt(prompt.clone(), "new rules".into()));
    assert_eq!(barrier(&mut worker), Ok(()));
    assert_eq!(std::fs::read_to_string(prompt).unwrap(), "new rules");
}

#[test]
fn barrier_succeeds_when_no_persistence_failed() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(barrier(&mut worker(temp.path())), Ok(()));
}
