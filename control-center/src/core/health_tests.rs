use super::*;
use crate::core::activity::Pass;
use crate::core::config::ConfigDoc;
use crate::core::logs::{LogFile, LogKind, PassEntry};
use crate::core::state::{LoopState, RunRecord};
use crate::sys::git::CloneStatus;
use crate::sys::schedtask::TaskInfo;
use std::path::PathBuf;

fn healthy() -> Snapshot {
    let mut s = Snapshot::empty(PathBuf::from("root"), Some(PathBuf::from("cli.exe")));
    let map = serde_json::json!({"enabled": true, "clonePath": "C:/clone"}).as_object().unwrap().clone();
    s.config = Ok(ConfigDoc { path: PathBuf::from("agent-loop.json"), map });
    s.task = Some(Ok(TaskInfo { exists: true, state: "Ready".into(), next_run: Some(Local::now() + Duration::minutes(30)), ..Default::default() }));
    s.clone = Some(Ok(CloneStatus { exists: true, branch_line: "dev...origin/dev".into(), dirty: vec![] }));
    s.gh_auth = Some(Ok(()));
    s.passes = vec![pass_entry(Duration::hours(2), Pass { finished: true, ..Default::default() })];
    s
}

fn pass_entry(age: Duration, pass: Pass) -> PassEntry {
    let file = LogFile { path: "x".into(), name: "x".into(), kind: LogKind::Owner, started: Some(Local::now() - age), modified: None, size: 0 };
    PassEntry { file, pass }
}

fn level_of(h: &Health, title: &str) -> Option<Level> {
    h.checks.iter().find(|c| c.title == title).map(|c| c.level)
}

#[test]
fn healthy_loop_is_green() {
    let h = evaluate(&healthy(), Local::now());
    assert_eq!(h.overall, Level::Ok, "{:?}", h.checks);
    assert_eq!(h.headline, "Alles in Ordnung");
    assert!(h.subline.starts_with("Nächster Lauf in"));
}

#[test]
fn indefinite_pause_is_flagged_with_resume_fix() {
    let mut s = healthy();
    s.state = Ok(LoopState { paused_until: Some("9999-12-31T23:59:59.9999999".into()), pause_reason: Some("switch dev-ki -> dev".into()), ..Default::default() });
    let h = evaluate(&s, Local::now());
    assert_eq!(h.headline, "Pausiert");
    let c = h.checks.iter().find(|c| c.title == "Pausiert").unwrap();
    assert_eq!(c.fix.as_ref().unwrap().1, Fix::Resume);
    assert!(c.detail.contains("switch dev-ki -> dev"));
}

#[test]
fn expired_claude_login_is_an_error_with_hint() {
    let mut s = healthy();
    let pass = Pass { finished: true, is_error: true, summary: Some("Failed to authenticate: OAuth session expired".into()), ..Default::default() };
    s.passes = vec![pass_entry(Duration::hours(1), pass)];
    let h = evaluate(&s, Local::now());
    assert_eq!(h.overall, Level::Error);
    let c = h.checks.iter().find(|c| c.title == "Letzter PO-Lauf fehlgeschlagen").unwrap();
    assert!(c.detail.contains("/login"));
}

#[test]
fn dirty_clone_and_worker_streak_are_errors() {
    let mut s = healthy();
    s.clone = Some(Ok(CloneStatus { exists: true, branch_line: "dev".into(), dirty: vec!["a.cs".into(), "b.cs".into()] }));
    let failed = RunRecord { kind: "task".into(), exit_code: 1, note: Some("branch setup failed".into()), ..Default::default() };
    s.state = Ok(LoopState { last_runs: vec![failed.clone(), failed.clone(), failed], ..Default::default() });
    let h = evaluate(&s, Local::now());
    assert_eq!(level_of(&h, "Clone blockiert"), Some(Level::Error));
    let w = h.checks.iter().find(|c| c.title == "Worker scheitern").unwrap();
    assert!(w.detail.starts_with("3 Läufe in Folge"));
    // once the clone is clean again the streak is history, not an emergency
    s.clone = Some(Ok(CloneStatus { exists: true, branch_line: "dev".into(), dirty: vec![] }));
    let h2 = evaluate(&s, Local::now());
    assert_eq!(level_of(&h2, "Worker scheiterten"), Some(Level::Warn));
    assert!(w.detail.contains("uncommittete"));
    assert_eq!(h.headline, "Braucht Aufmerksamkeit");
}

#[test]
fn disabled_task_and_config_offer_fixes() {
    let mut s = healthy();
    s.task = Some(Ok(TaskInfo { exists: true, state: "Disabled".into(), ..Default::default() }));
    if let Ok(c) = s.config.as_mut() {
        c.map.insert("enabled".into(), false.into());
    }
    let h = evaluate(&s, Local::now());
    let fixes: Vec<_> = h.checks.iter().filter_map(|c| c.fix.as_ref().map(|f| f.1.clone())).collect();
    assert!(fixes.contains(&Fix::EnableTask));
    assert!(fixes.contains(&Fix::EnableConfig));
}

#[test]
fn unfinished_pass_while_running_is_info_not_error() {
    let mut s = healthy();
    s.loop_running = true;
    s.passes = vec![pass_entry(Duration::minutes(3), Pass::default())];
    let h = evaluate(&s, Local::now());
    assert_eq!(level_of(&h, "Product Owner arbeitet"), Some(Level::Info));
    assert_eq!(h.headline, "Arbeitet gerade");
}

#[test]
fn unchecked_probes_stay_pending_not_red() {
    let mut s = healthy();
    s.task = None;
    s.clone = None;
    s.gh_auth = None;
    let h = evaluate(&s, Local::now());
    assert!(h.overall <= Level::Info);
}
