use super::*;

#[test]
fn parses_issue_and_pr_payloads() {
    let issue = r#"{"number":1480,"title":"Fix it","body":null,"state":"open","labels":[{"id":1,"name":"Agent-Task"}],"assignees":[{"login":"aignermax"}],"html_url":"u"}"#;
    let i: Vec<Issue> = parse_ndjson(issue).unwrap();
    assert!(i[0].has_label("agent-task"));
    assert_eq!(i[0].assignees[0].login, "aignermax");
    assert!(i[0].pull_request.is_none());
    let pr = r#"{"number":5,"title":"Agent: x","html_url":"u","head":{"ref":"agent/issue-1-2","sha":"abc"},"base":{"ref":"dev"},"labels":[],"updated_at":"2026-10-14T10:00:00Z"}"#;
    let p: Vec<PullRequest> = parse_ndjson(pr).unwrap();
    assert_eq!(p[0].head.name, "agent/issue-1-2");
    assert_eq!(p[0].base.name, "dev");
}

#[test]
fn status_matching_ignores_numbers_in_the_path() {
    assert!(is_status("gh api DELETE repos/o/r/issues/7/labels/x: gh: Label does not exist (HTTP 404)", 404));
    assert!(!is_status("gh api DELETE repos/o/r/issues/1404/labels/x: gh: Forbidden (HTTP 403)", 404));
}

#[test]
fn ndjson_error_excerpt_is_char_safe() {
    let line = format!("{{\"number\": \"{}\"}}", "ä".repeat(100));
    assert!(parse_ndjson::<Issue>(&line).is_err());
}

#[test]
fn encodes_path_segments() {
    assert_eq!(encode("agent-task"), "agent-task");
    assert_eq!(encode("needs human"), "needs%20human");
    assert_eq!(encode("agent/issue-7"), "agent%2Fissue-7");
}

/// Real read-only calls with the machine's gh login.
/// `cargo test -p team-core live_gh -- --ignored --nocapture`
#[test]
#[ignore]
fn live_gh_read_only() {
    let login = current_user().unwrap();
    let repo = Repo::new("aignermax/Lunima");
    let issues = repo.open_issues("agent-task").unwrap();
    let prs = repo.open_prs().unwrap();
    let found = search_repos("org:Akhetonics is:issue is:open").unwrap();
    println!("login={login} default={} issues={} prs={} org-repos={found:?}", repo.default_branch().unwrap(), issues.len(), prs.len());
    assert!(!login.is_empty());
    assert!(repo.branch_exists("main").unwrap());
    assert!(!repo.branch_exists("definitely-not-a-branch-xyz").unwrap());
}
