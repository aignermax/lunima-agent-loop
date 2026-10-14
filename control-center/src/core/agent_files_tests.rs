use super::*;

#[test]
fn reads_heartbeats_written_by_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let status = r#"{"role": "coder", "pid": 1, "state": "working", "detail": {"repo": "o/r", "issue": 7}, "since": "2026-10-09T12:02:15+02:00", "updated": "2026-10-09T12:02:37+02:00"}"#;
    std::fs::write(dir.path().join("status-coder.json"), status).unwrap();
    std::fs::write(dir.path().join("status-qa.json"), r#"{"state":"paused","detail":{"reason":"Claude-Nutzungslimit bis 15:00"}}"#).unwrap();
    std::fs::write(dir.path().join("status-broken.json"), "{").unwrap();
    let hb = read_heartbeats(dir.path());
    let c = &hb["coder"];
    assert_eq!(c.state, "working");
    assert_eq!(c.detail["issue"], 7);
    assert!(c.updated.unwrap() > c.since.unwrap());
    assert_eq!(hb["qa"].reason(), Some("Claude-Nutzungslimit bis 15:00"));
    assert!(!hb.contains_key("broken"));
}

#[test]
fn pause_round_trip_keeps_other_roles_and_keys() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(CONTROL_FILE), r#"{"paused":{"qa":{"until":null,"reason":"x"}},"other":1}"#).unwrap();
    set_pause(dir.path(), "coder", Some("Control Center")).unwrap();
    let now = Local::now();
    let p = read_pauses(dir.path(), now);
    assert_eq!(p.get("coder").map(String::as_str), Some("Control Center"));
    assert_eq!(p.get("qa").map(String::as_str), Some("x"));
    set_pause(dir.path(), "coder", None).unwrap();
    assert!(!read_pauses(dir.path(), now).contains_key("coder"));
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join(CONTROL_FILE)).unwrap()).unwrap();
    assert_eq!(raw["other"], 1);
    // the daemon must understand what we write
    set_pause(dir.path(), "qa", Some("Urlaub")).unwrap();
    assert_eq!(team_core::state::pause_reason(dir.path(), "qa", now).as_deref(), Some("Urlaub"));
}

#[test]
fn elapsed_pauses_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(CONTROL_FILE), r#"{"paused":{"qa":{"until":"2020-01-01T00:00:00","reason":"x"}}}"#).unwrap();
    assert!(read_pauses(dir.path(), Local::now()).is_empty());
}

#[test]
fn history_newest_first_with_machine() {
    let dir = tempfile::tempdir().unwrap();
    let lines = [
        r#"{"number":1,"title":"a","repository":"o/r","completed":true,"pr_url":"u","total_tokens":1,"total_cost_usd":0.5,"session_count":1,"timestamp":"2026-10-14 08:00:00","duration_sec":60}"#,
        "garbage",
        r#"{"number":2,"title":"b","repository":"o/r","completed":false,"pr_url":null,"total_tokens":1,"total_cost_usd":0.1,"session_count":1,"timestamp":"2026-10-14 09:00:00","duration_sec":null,"machine":"RISCWIN64"}"#,
    ];
    std::fs::write(dir.path().join(HISTORY_FILE), lines.join("\n")).unwrap();
    let h = read_history(dir.path(), 10);
    assert_eq!(h.iter().map(|e| e.number).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(h[0].machine.as_deref(), Some("RISCWIN64"));
}

#[test]
fn pid_file_tells_whether_the_daemon_runs() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!agent_running(dir.path()));
    std::fs::write(dir.path().join(PID_FILE), std::process::id().to_string()).unwrap();
    assert_eq!(agent_pid(dir.path()), Some(std::process::id()));
    assert!(!agent_running(dir.path()), "a live pid of another program is not the agent");
    std::fs::write(dir.path().join(PID_FILE), "4294967").unwrap();
    assert!(!agent_running(dir.path()));
}

#[test]
fn log_scan_finds_logins_failures_panics_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let log = [
        "2026-10-14 09:00:00 [INFO] lunima-team-agent startet",
        "2026-10-14 09:01:00 [ERROR] [coder] o/r#1: Claude beendet ohne Ergebnis (Exit Some(1)): Not logged in · Please run /login",
        "2026-10-14 09:02:00 [ERROR] [qa] Interner Fehler (panic): boom",
        "no timestamp line",
    ];
    std::fs::write(dir.path().join("agent.log.1"), "2026-10-14 08:00:00 [ERROR] [qa] Claude hing (1800 s ohne Ausgabe) und wurde beendet\n").unwrap();
    std::fs::write(dir.path().join("agent.log"), log.join("\n")).unwrap();
    let f = scan_log(dir.path());
    assert!(f.not_logged_in.is_some());
    assert_eq!(f.claude_failures.len(), 2, "includes the rotated file");
    assert_eq!(f.panics.len(), 1);
    assert!(f.last_error.as_ref().unwrap().1.contains("panic"));
}

#[test]
fn discovered_repos_expire() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("discovered-repos.json"), r#"{"o/new":{"last_seen":"2026-10-13 10:00:00"},"o/old":{"last_seen":"2026-09-01 10:00:00"},"o/bad":{}}"#).unwrap();
    let now = NaiveDateTime::parse_from_str("2026-10-14 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
    assert_eq!(read_discovered(dir.path(), 8, now), vec!["o/new"]);
}
