//! Coder end to end: scripted Claude, simulated GitHub, real git against a local origin.

use super::*;
use crate::fake::{self, FakeForge, FakeLlm};
use crate::llm::LlmError;
use std::sync::Arc;
use team_core::claude::Outcome;

const SUMMARY: &str = "done\n=== PR SUMMARY ===\n- added feature\n=== END ===";
const REVIEW_OK: &str = "=== REVIEW RESULT ===\nVERDICT: OK\nSUMMARY: fine\n=== FINDINGS ===\n=== END ===";
const REVIEW_BLOCKING: &str = "=== REVIEW RESULT ===\nVERDICT: BLOCKING\nSUMMARY: broken\n=== FINDINGS ===\n- [MAJOR] fix it\n=== END ===";

struct World {
    _tmp: tempfile::TempDir,
    origin: std::path::PathBuf,
    forge: Arc<FakeForge>,
    llm: Arc<FakeLlm>,
    ctx: Ctx,
}

fn world(forge: FakeForge, llm: FakeLlm) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let origin = fake::origin(tmp.path(), &[]);
    let (forge, llm) = (Arc::new(forge), Arc::new(llm));
    let ctx = fake::ctx(tmp.path(), &origin, forge.clone(), llm.clone());
    World { _tmp: tmp, origin, forge, llm, ctx }
}

fn coder() -> Coder {
    Coder::new(Box::new(|_| Ok(vec![])))
}

fn task(n: u64, body: &str, extra: &[&str]) -> team_core::gh::Issue {
    let mut labels = vec!["agent-task"];
    labels.extend_from_slice(extra);
    fake::issue(n, "Add feature", body, &labels)
}

fn max_turns(file: &'static str) -> impl Fn(&crate::llm::Job) -> Result<Outcome, LlmError> + Send + Sync {
    move |job| {
        std::fs::write(job.cwd.join(file), "wip").unwrap();
        Ok(Outcome { text: "not done".into(), reached_max_turns: true, num_turns: 150, ..Default::default() })
    }
}

fn the_pr(w: &World) -> team_core::gh::PullRequest {
    w.forge.s.lock().unwrap().prs.values().next().cloned().expect("a PR")
}

#[test]
fn implements_an_issue_as_a_reviewed_pr() {
    let w = world(FakeForge::new("o/r").with_issue(task(1, "Please add it", &[])), FakeLlm::new().reply(SUMMARY, Some("feature.txt")).reply(REVIEW_OK, None));
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    assert!(pr.title == "Agent: Add feature" && pr.head.name.starts_with("agent/issue-1-"));
    assert_eq!(pr.base.name, "dev", "agent PRs target the integration branch");
    assert!(pr.body.as_deref().unwrap().starts_with("Automated implementation for #1\n"));
    assert!(pr.body.as_deref().unwrap().contains("- added feature"));
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "feature.txt").is_some());
    let log = w.forge.log();
    assert!(log.contains(&"label+ 1 agent-running".to_string()) && log.contains(&"label- 1 agent-task".to_string()));
    assert!(log.contains(&"assign 1 bot".to_string()) && log.contains(&"close 1".to_string()));
    let issue_comments = w.forge.comment_bodies(1);
    assert!(issue_comments[0].contains("Agent `TESTBOX` is now working"));
    assert!(issue_comments.last().unwrap().starts_with("Implementation complete. PR:"));
    assert!(w.forge.comment_bodies(pr.number).iter().any(|c| c.contains("Automated Review — verdict: **OK**")));
    assert!(!w.forge.labels(1).iter().any(|l| l == "agent-running"));
    let history = std::fs::read_to_string(w.ctx.paths.state().join("issue-history.jsonl")).unwrap();
    assert!(history.contains("\"completed\":true") && history.contains("TESTBOX"));
    assert!(Store::new(&w.ctx.paths.sessions()).open_in("o/r").is_empty());
    assert!(w.llm.prompt(0).contains("=== PR SUMMARY ==="));
}

#[test]
fn nothing_to_change_closes_the_issue() {
    let w = world(FakeForge::new("o/r").with_issue(task(2, "", &[])), FakeLlm::new().reply("already there", None));
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(w.forge.s.lock().unwrap().prs.is_empty());
    assert!(w.forge.comment_bodies(2).last().unwrap().starts_with("[OK] This issue appears to be already resolved."));
    assert!(w.forge.log().contains(&"close 2".to_string()));
}

#[test]
fn continuation_keeps_the_previous_sessions_work() {
    let llm = FakeLlm::new().then(max_turns("part1.txt")).reply(SUMMARY, Some("part2.txt")).reply(REVIEW_OK, None);
    let w = world(FakeForge::new("o/r").with_issue(task(3, "big", &[])), llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "part1.txt").is_some(), "work of session 1 must survive");
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "part2.txt").is_some());
    assert!(w.llm.prompt(1).contains("Session 2"));
    assert!(w.forge.comment_bodies(3).iter().any(|c| c.contains("**Session 1 completed**")));
}

#[test]
fn failures_requeue_then_give_up() {
    let w = world(FakeForge::new("o/r").with_issue(task(4, "x", &[])), FakeLlm::new().then(|_| Err(LlmError::Failed("claude crashed".into()))));
    let mut c = coder();
    assert!(c.cycle(&w.ctx).unwrap());
    let labels = w.forge.labels(4);
    assert!(labels.contains(&"agent-task".to_string()) && !labels.contains(&"agent-running".to_string()));
    assert!(w.forge.comment_bodies(4).last().unwrap().contains("Attempt 1 on `TESTBOX` failed"));
    c.cycle(&w.ctx).unwrap();
    c.cycle(&w.ctx).unwrap();
    let last = w.forge.comment_bodies(4).last().unwrap().clone();
    assert!(last.contains("failed to complete this issue after 3 attempts"), "{last}");
    assert!(w.forge.labels(4).contains(&"needs-human".to_string()));
    assert!(!c.cycle(&w.ctx).unwrap(), "given up: not picked again");
}

#[test]
fn usage_limit_keeps_the_claim_and_resumes_later() {
    let limit = chrono::Local::now() + chrono::Duration::hours(1);
    let llm = FakeLlm::new().then(move |_| Err(LlmError::UsageLimit(limit))).reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None);
    let w = world(FakeForge::new("o/r").with_issue(task(5, "x", &[])), llm);
    let mut c = coder();
    assert!(matches!(c.cycle(&w.ctx), Err(crate::ctx::RoleError::UsageLimit(_))));
    assert!(w.forge.labels(5).contains(&"agent-running".to_string()));
    assert!(c.cycle(&w.ctx).unwrap());
    assert!(w.forge.log().contains(&"close 5".to_string()));
    assert_eq!(w.forge.comment_bodies(5).iter().filter(|b| b.contains("is now working")).count(), 1);
}

#[test]
fn another_machines_claim_is_not_resumed() {
    let w = world(FakeForge::new("o/r").with_issue(fake::issue(6, "t", "x", &["agent-running"])), FakeLlm::new().reply(SUMMARY, Some("f.txt")));
    let store = Store::new(&w.ctx.paths.sessions());
    store.save(&crate::session::SessionState::new("o/r", 6, "agent/issue-6-1"));
    w.forge.add_comment(6, "<!-- team-agent:claim machine=OTHERBOX -->\n🤖 Agent `OTHERBOX` is now working on this issue...");
    assert!(!coder().cycle(&w.ctx).unwrap());
    assert_eq!(w.llm.calls(), 0);
}

#[test]
fn blocking_review_escalates_after_the_last_round() {
    let w = world(FakeForge::new("o/r").with_issue(task(7, "x", &[])), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_BLOCKING, None));
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    assert!(w.forge.comment_bodies(pr.number).iter().any(|c| c.starts_with("Reached max review rounds (1)")));
    assert!(w.forge.labels(7).contains(&"needs-human".to_string()));
    assert!(!w.forge.log().contains(&"close 7".to_string()));
}

#[test]
fn complex_issue_gets_a_retry_round() {
    let llm = FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_BLOCKING, None).reply("fixed", Some("fix.txt")).reply(REVIEW_OK, None);
    let w = world(FakeForge::new("o/r").with_issue(task(8, "x", &["complex"])), llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "fix.txt").is_some());
    assert!(w.llm.prompt(2).contains("fix it"), "retry prompt carries the findings");
    assert!(w.llm.prompt(0).contains("UX Design Pass"));
    assert!(w.forge.log().contains(&"close 8".to_string()));
}

#[test]
fn team_branch_is_created_and_targeted() {
    let w = world(FakeForge::new("o/r").with_issue(task(9, "Team branch: team/optics", &[])), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    assert!(coder().cycle(&w.ctx).unwrap());
    assert_eq!(the_pr(&w).base.name, "team/optics");
    assert!(fake::pushed_file(&w.origin, "team/optics", "README.md").is_some());
    assert!(w.forge.comment_bodies(9).iter().any(|c| c.starts_with("🌱 Team branch `team/optics`")));
}

#[test]
fn qa_failed_pr_is_fixed_before_new_issues() {
    let w = world(FakeForge::new("o/r").with_issue(task(11, "new work", &[])), FakeLlm::new().reply("fixed", Some("qa.txt")));
    let setup = crate::workspace::setup(&w.ctx, "seed", "o/r").unwrap();
    setup.clone.git_net(&["push", "origin", "origin/dev:refs/heads/agent/issue-10-1"]).unwrap();
    let mut pr = fake::pr(20, "Agent: old", "agent/issue-10-1", &["qa-failed"]);
    pr.body = Some("Automated implementation for #10\n".into());
    w.forge.s.lock().unwrap().prs.insert(20, pr);
    w.forge.add_comment(20, "[qa-agent] **FAILED**\n\n- `test`: failed (exit 1)");
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(fake::pushed_file(&w.origin, "agent/issue-10-1", "qa.txt").is_some());
    assert!(!w.forge.labels(20).iter().any(|l| l == "qa-failed" || l == "agent-running"));
    assert!(w.llm.prompt(0).contains("[qa-agent] **FAILED**"));
    assert!(w.forge.labels(11).contains(&"agent-task".to_string()), "new issue untouched this cycle");
}

#[test]
fn qa_fix_escalates_after_too_many_failures() {
    let w = world(FakeForge::new("o/r").with_pr(fake::pr(21, "Agent: x", "agent/issue-1-1", &["qa-failed"])), FakeLlm::new().reply("x", None));
    for _ in 0..3 {
        w.forge.add_comment(21, "[qa-agent] **FAILED**\n\nbroken");
    }
    assert!(coder().cycle(&w.ctx).unwrap());
    assert_eq!(w.llm.calls(), 0);
    let labels = w.forge.labels(21);
    assert!(labels.contains(&"needs-human".to_string()) && !labels.contains(&"qa-failed".to_string()));
    assert!(w.forge.comment_bodies(21).last().unwrap().contains("max QA-fix rounds (2)"));
}

#[test]
fn second_machine_finds_nothing_to_claim() {
    let w = world(FakeForge::new("o/r").with_issue(task(12, "x", &[])), FakeLlm::new().then(|_| Err(LlmError::UsageLimit(chrono::Local::now() + chrono::Duration::hours(1)))));
    let _ = coder().cycle(&w.ctx);
    let mut other = w.ctx.clone();
    other.machine = "OTHERBOX".into();
    assert!(!coder().cycle(&other).unwrap(), "issue is claimed by TESTBOX");
}
