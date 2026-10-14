use super::*;
use serde_json::json;

fn lines(events: &[Value]) -> Vec<String> {
    events.iter().map(|e| e.to_string()).collect()
}

fn assistant(id: &str, input: u64, output: u64, cmd: Option<&str>) -> Value {
    let mut content = vec![json!({"type": "text", "text": "…"})];
    if let Some(c) = cmd {
        content.push(json!({"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": c}}));
    }
    json!({"type": "assistant", "message": {"id": id, "content": content, "usage": {"input_tokens": input, "output_tokens": output, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}})
}

#[test]
fn success_with_deduplicated_usage_and_commands() {
    let ev = [
        json!({"type": "system", "subtype": "init", "session_id": "s-1", "model": "claude-opus-5-5"}),
        assistant("m1", 100, 10, Some("git status")),
        assistant("m1", 100, 10, None), // same API message split into two events
        assistant("m2", 50, 5, Some("dotnet test")),
        json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done.\n=== PR SUMMARY ===", "num_turns": 7, "total_cost_usd": 1.25}),
    ];
    let o = parse_events(&lines(&ev), 150).unwrap();
    assert_eq!(o.usage.input, 150);
    assert_eq!(o.usage.output, 15);
    assert_eq!(o.bash_commands, ["git status", "dotnet test"]);
    assert_eq!(o.session_id, "s-1");
    assert_eq!(o.num_turns, 7);
    assert!(!o.reached_max_turns && !o.is_error);
    assert_eq!(o.cost_usd, 1.25);
}

#[test]
fn max_turns_from_subtype_or_count() {
    let r = |sub: &str, turns: u64| lines(&[json!({"type": "result", "subtype": sub, "is_error": sub != "success", "result": "", "num_turns": turns})]);
    assert!(parse_events(&r("error_max_turns", 3), 150).unwrap().reached_max_turns);
    assert!(parse_events(&r("success", 150), 150).unwrap().reached_max_turns);
    assert!(!parse_events(&r("success", 149), 150).unwrap().reached_max_turns);
}

#[test]
fn no_result_event_means_crash() {
    assert!(parse_events(&lines(&[assistant("m", 1, 1, None)]), 10).is_none());
    assert!(parse_events(&["not json".to_string()], 10).is_none());
}

#[test]
fn usage_limit_messages() {
    let now = Local.with_ymd_and_hms(2026, 10, 14, 10, 0, 0).unwrap();
    let epoch = usage_limit_reset("Claude AI usage limit reached|1792000000", now).unwrap();
    assert_eq!(epoch.timestamp(), 1792000000);
    let pm = usage_limit_reset("You've hit your limit · resets 3pm (Europe/Berlin)", now).unwrap();
    assert_eq!(pm.format("%H:%M").to_string(), "15:00");
    let morning = usage_limit_reset("5-hour limit reached ∙ resets 9am", now).unwrap();
    assert_eq!(morning.format("%d %H:%M").to_string(), "15 09:00", "already past today → tomorrow");
    let fallback = usage_limit_reset("usage limit reached", now).unwrap();
    assert_eq!(fallback, now + Duration::hours(1));
    assert!(usage_limit_reset("Task failed: tests red", now).is_none());
}

#[test]
fn error_result_with_limit_sets_retry_time() {
    let ev = [json!({"type": "result", "subtype": "success", "is_error": true, "result": "Claude AI usage limit reached|1792000000", "num_turns": 1})];
    let o = parse_events(&lines(&ev), 10).unwrap();
    assert_eq!(o.usage_limit_until.unwrap().timestamp(), 1792000000);
}
