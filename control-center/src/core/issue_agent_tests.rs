use super::*;

fn agent_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("main.py"), "").unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::create_dir_all(tmp.path().join(".sessions")).unwrap();
    tmp
}

#[test]
fn resolves_sibling_folder_and_explicit_setting() {
    let parent = tempfile::tempdir().unwrap();
    let loop_root = parent.path().join("lunima-agent-loop");
    let agent = parent.path().join("autonomous-issue-agent");
    std::fs::create_dir_all(&loop_root).unwrap();
    std::fs::create_dir_all(agent.join("src")).unwrap();
    std::fs::write(agent.join("main.py"), "").unwrap();
    assert_eq!(resolve_dir(&loop_root), Some(agent.clone()));

    let other = agent_dir();
    save_dir_setting(&loop_root, other.path()).unwrap();
    assert_eq!(resolve_dir(&loop_root).as_deref(), Some(other.path()));
}

#[test]
fn reads_heartbeats_written_by_control_py() {
    let dir = agent_dir();
    let status = r#"{"role": "coder", "pid": 1, "state": "working", "detail": {"repo": "o/r", "issue": 7}, "since": "2026-10-09T12:02:15+02:00", "updated": "2026-10-09T12:02:37+02:00"}"#;
    std::fs::write(dir.path().join(".sessions/status-coder.json"), status).unwrap();
    let hb = read_heartbeats(dir.path());
    let c = &hb["coder"];
    assert_eq!(c.state, "working");
    assert_eq!(c.detail["issue"], 7);
    assert!(c.updated.unwrap() > c.since.unwrap());
}

#[test]
fn pause_round_trip_keeps_other_roles_and_keys() {
    let dir = agent_dir();
    std::fs::write(dir.path().join(".sessions/control.json"), r#"{"note": "keep", "paused": {"qa": {"until": null, "reason": "x"}}}"#).unwrap();
    set_pause(dir.path(), "coder", Some("Control Center")).unwrap();
    let now = Local::now();
    let p = read_pauses(dir.path(), now);
    assert_eq!(p.get("coder").map(String::as_str), Some("Control Center"));
    assert_eq!(p.get("qa").map(String::as_str), Some("x"));
    set_pause(dir.path(), "coder", None).unwrap();
    assert!(!read_pauses(dir.path(), now).contains_key("coder"));
    assert!(std::fs::read_to_string(dir.path().join(".sessions/control.json")).unwrap().contains("keep"));
}

#[test]
fn elapsed_pause_is_ignored() {
    let dir = agent_dir();
    std::fs::write(dir.path().join(".sessions/control.json"), r#"{"paused": {"qa": {"until": "2020-01-01T00:00:00+00:00"}}}"#).unwrap();
    assert!(read_pauses(dir.path(), Local::now()).is_empty());
}

#[test]
fn history_is_newest_first_and_tolerates_garbage() {
    let dir = agent_dir();
    let lines = [
        r#"{"number": 1, "title": "a", "repository": "o/r", "completed": true, "pr_url": null, "timestamp": "2026-10-07 00:32:26"}"#,
        "not json",
        r#"{"number": 2, "title": "b", "repository": "o/r", "completed": false, "pr_url": "u", "total_cost_usd": 0.2, "timestamp": "2026-10-09 10:03:10", "duration_sec": 60}"#,
    ];
    std::fs::write(dir.path().join(".sessions/issue-history.jsonl"), lines.join("\n")).unwrap();
    let h = read_history(dir.path(), 10);
    assert_eq!(h.iter().map(|e| e.number).collect::<Vec<_>>(), [2, 1]);
}

#[test]
fn log_scan_finds_the_known_failure_patterns() {
    let dir = agent_dir();
    let log = "\
2026-10-09 08:38:56,799 [ERROR] Claude Code failed: \n\
2026-10-09 08:39:16,451 [WARNING] PR #1471 has 154 QA failures (>= 2); escalating to human.\n\
2026-10-09 08:40:00,000 [ERROR] API Error: 400 Claude Code 2.1.197 does not support this model\n\
2026-10-09 08:41:00,000 [ERROR] Not logged in · Please run /login\n\
no timestamp line with does not support this model\n";
    std::fs::write(dir.path().join("agent.log"), log).unwrap();
    let f = scan_log(dir.path(), None);
    assert_eq!(f.claude_failures.len(), 1);
    assert_eq!(f.qa_loops.iter().map(|(pr, n, _)| (*pr, *n)).collect::<Vec<_>>(), [(1471, 154)]);
    assert!(f.model_unsupported.is_some());
    assert!(f.not_logged_in.is_some());
    assert!(f.log_bytes > 0);
}

#[test]
fn log_scan_keeps_latest_per_pr_and_applies_wsl_offset() {
    let dir = agent_dir();
    let log = "\
2026-10-09 08:39:16,451 [WARNING] PR #1471 has 154 QA failures (>= 2); escalating to human.\n\
2026-10-09 11:30:00,000 [WARNING] PR #1480 has 6 QA failures (>= 2); escalating to human.\n";
    std::fs::write(dir.path().join("agent.log"), log).unwrap();
    let utc = scan_log(dir.path(), FixedOffset::east_opt(0));
    let loops: Vec<_> = utc.qa_loops.iter().map(|(pr, n, _)| (*pr, *n)).collect();
    assert_eq!(loops, [(1471, 154), (1480, 6)]);
    let t = utc.qa_loops[1].2;
    assert_eq!(t.with_timezone(&chrono::Utc).format("%H:%M").to_string(), "11:30");
}

#[test]
fn agent_dir_needs_main_py_and_src() {
    let dir = agent_dir();
    assert!(is_agent_dir(dir.path()));
    std::fs::remove_dir_all(dir.path().join("src")).unwrap();
    assert!(!is_agent_dir(dir.path()));
}
