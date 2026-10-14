use super::*;
use chrono::Duration;

#[test]
fn pause_variants_and_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    let now = Local::now();
    assert_eq!(pause_reason(tmp.path(), "coder", now), None);
    let future = (now + Duration::hours(1)).to_rfc3339();
    let past = (now - Duration::hours(1)).to_rfc3339();
    let doc = json!({"paused": {"coder": {"until": null, "reason": "Urlaub"}, "qa": {"until": future}, "pr-feedback": {"until": past}}});
    std::fs::write(tmp.path().join(CONTROL_FILE), doc.to_string()).unwrap();
    assert_eq!(pause_reason(tmp.path(), "coder", now).as_deref(), Some("Urlaub"));
    assert_eq!(pause_reason(tmp.path(), "qa", now).as_deref(), Some("pausiert"));
    assert_eq!(pause_reason(tmp.path(), "pr-feedback", now), None);
    for garbage in ["{nope", "[]", r#"{"paused": ["coder"]}"#, r#"{"paused": {"coder": {"until": 5}}}"#] {
        std::fs::write(tmp.path().join(CONTROL_FILE), garbage).unwrap();
        let _ = pause_reason(tmp.path(), "coder", now); // must not panic
    }
}

#[test]
fn heartbeat_keeps_since_while_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let hb = Heartbeat::new(tmp.path(), "coder");
    hb.report("working", crate::detail!["issue" => 7, "repo" => "o/r"]);
    let read = || serde_json::from_str::<Value>(&std::fs::read_to_string(tmp.path().join("status-coder.json")).unwrap()).unwrap();
    let first = read();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    hb.touch();
    let second = read();
    assert_eq!(first["since"], second["since"]);
    assert_eq!(second["detail"]["issue"], 7);
    assert_eq!(second["role"], "coder");
    hb.idle();
    assert_eq!(read()["state"], "idle");
}

#[test]
fn history_is_appended_as_jsonl_compatible_with_python() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = HistoryRecord {
        number: 7,
        title: "t".into(),
        repository: "o/r".into(),
        completed: true,
        pr_url: Some("u".into()),
        total_tokens: 10,
        total_cost_usd: 0.5,
        session_count: 1,
        timestamp: "2026-10-14 10:00:00".into(),
        duration_sec: Some(60),
        machine: None,
    };
    append_history(tmp.path(), &rec).unwrap();
    append_history(tmp.path(), &rec).unwrap();
    let text = std::fs::read_to_string(tmp.path().join(HISTORY_FILE)).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(!text.contains("machine"), "omitted when unknown, like Python records");
    let back: HistoryRecord = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(back, rec);
}

#[test]
fn parse_time_accepts_control_center_formats() {
    assert!(parse_time("2026-10-14T10:00:00+02:00").is_some());
    assert!(parse_time("2026-10-14T10:00:00.1234567").is_some());
    assert!(parse_time("2026-10-14 10:00:00").is_some());
    assert!(parse_time("9999-12-31T23:59:59.9999999").is_some());
    assert!(parse_time("soon").is_none());
}
