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
    s.config = Ok(ConfigDoc { path: PathBuf::from("agent-loop.json"), map, had_comments: false });
    s.task = Some(Ok(TaskInfo { exists: true, state: "Ready".into(), next_run: Some(Local::now() + Duration::minutes(30)), ..Default::default() }));
    s.clone = Some(Ok(CloneStatus { exists: true, branch_line: "dev...origin/dev".into(), dirty: vec![] }));
    s.gh_auth = Some(Ok(()));
    s.passes = vec![pass_entry(Duration::hours(2), Pass { finished: true, ..Default::default() })];
    s
}

fn pass_entry(age: Duration, pass: Pass) -> PassEntry {
    let file = LogFile { path: "x".into(), name: "x".into(), kind: LogKind::Owner, started: Some(Local::now() - age), modified: None, size: 0 };
    PassEntry { file, pass, record_exit: None }
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
    s.logs = vec![s.passes[0].file.clone()];
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


#[test]
fn run_record_decides_when_the_log_has_no_result_line() {
    // e.g. ownerRunner = kimi: no Claude `result` event, but the loop recorded the exit code
    let mut s = healthy();
    let mut ok = pass_entry(Duration::hours(1), Pass::default());
    ok.record_exit = Some(0);
    s.passes = vec![ok];
    assert_eq!(level_of(&evaluate(&s, Local::now()), "Product Owner"), Some(Level::Ok));
    s.passes[0].record_exit = Some(3);
    assert_eq!(level_of(&evaluate(&s, Local::now()), "Letzter PO-Lauf fehlgeschlagen"), Some(Level::Error));
}

#[test]
fn worker_running_after_po_pass_does_not_make_po_look_busy() {
    let mut s = healthy();
    s.loop_running = true;
    s.passes = vec![pass_entry(Duration::minutes(40), Pass::default())];
    let worker_log = LogFile { path: "w".into(), name: "w".into(), kind: LogKind::Task(7), started: Some(Local::now()), modified: None, size: 0 };
    s.logs = vec![worker_log, s.passes[0].file.clone()];
    assert_eq!(level_of(&evaluate(&s, Local::now()), "PO-Lauf abgebrochen"), Some(Level::Warn));
}

#[test]
fn attach_records_matches_each_pass_to_its_own_window() {
    use crate::core::logs::attach_records;
    let rec = |ago_min: i64, code: i32| RunRecord {
        kind: "owner".into(),
        exit_code: code,
        timestamp: (Local::now() - Duration::minutes(ago_min)).format("%Y-%m-%d %H:%M:%S").to_string(),
        ..Default::default()
    };
    let mut passes = vec![pass_entry(Duration::minutes(30), Pass::default()), pass_entry(Duration::minutes(120), Pass::default())];
    attach_records(&mut passes, &[rec(100, 1), rec(10, 0)]);
    assert_eq!(passes[0].record_exit, Some(0));
    assert_eq!(passes[1].record_exit, Some(1));
}
