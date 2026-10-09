use super::*;

fn tool_use(id: &str, cmd: &str) -> String {
    serde_json::json!({"type": "assistant", "message": {"content": [
        {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}
    ]}})
    .to_string()
}

fn tool_result(id: &str, text: &str, is_error: bool) -> String {
    serde_json::json!({"type": "user", "message": {"content": [
        {"type": "tool_result", "tool_use_id": id, "content": text, "is_error": is_error}
    ]}})
    .to_string()
}

fn log(lines: &[String]) -> String {
    lines.join("\n")
}

#[test]
fn issue_creation_gets_title_and_url() {
    let pass = parse_pass(&log(&[
        r#"{"type":"system","subtype":"init","model":"claude-opus-5-5"}"#.to_string(),
        tool_use("t1", r#"gh issue create --repo aignermax/Lunima --title "Fix MMI overlap" --body-file x.md"#),
        tool_result("t1", "https://github.com/aignermax/Lunima/issues/1480\n", false),
    ]));
    assert_eq!(pass.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(pass.actions.len(), 1);
    let a = &pass.actions[0];
    assert_eq!(a.kind, ActionKind::IssueCreated);
    assert_eq!(a.text, "Fix MMI overlap");
    assert_eq!(a.url.as_deref(), Some("https://github.com/aignermax/Lunima/issues/1480"));
}

#[test]
fn merges_labels_and_closes_are_recognised() {
    let pass = parse_pass(&log(&[
        tool_use("a", "gh pr merge 1466 --repo aignermax/Lunima --squash"),
        tool_use("b", r#"gh api repos/aignermax/Lunima/issues/1470/labels -X POST -f "labels[]=agent-task""#),
        tool_use("c", "gh issue close 1468 --repo aignermax/Lunima --comment done"),
        tool_use("d", "gh pr edit 1471 --base dev"),
        tool_use("e", "git status"),
    ]));
    let kinds: Vec<_> = pass.actions.iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [ActionKind::PrMerged, ActionKind::LabelAdded, ActionKind::IssueClosed, ActionKind::PrRetargeted]);
    assert_eq!(pass.actions[0].text, "#1466");
    assert_eq!(pass.actions[1].text, "#1470 → agent-task");
}

#[test]
fn failed_tool_call_is_marked() {
    let pass = parse_pass(&log(&[
        tool_use("x", "gh issue edit 5 --add-label agent-task && gh pr merge 7"),
        tool_result("x", "GraphQL: missing read:org", true),
    ]));
    assert!(pass.actions[0].failed);
    assert_eq!(pass.tool_errors, 1);
}

#[test]
fn auth_failure_result_is_an_error_pass() {
    let pass = parse_pass(&log(&[
        "[stderr] something".to_string(),
        r#"{"type":"result","is_error":true,"result":"Failed to authenticate: OAuth session expired","duration_ms":237,"num_turns":1}"#.to_string(),
    ]));
    assert!(pass.finished);
    assert!(pass.is_error);
    assert!(pass.summary.unwrap().contains("OAuth session expired"));
}

#[test]
fn unfinished_log_is_not_finished() {
    let pass = parse_pass(&tool_use("t", "gh issue create --title 'A'"));
    assert!(!pass.finished);
    assert_eq!(pass.actions[0].text, "A");
}
