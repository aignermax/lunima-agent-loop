//! Coder end to end: scripted Claude, simulated GitHub, real git against a local origin.

use super::*;
use crate::fake::{self, FakeForge, FakeLlm};
use crate::forge::Forge;
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
    world_arc(Arc::new(forge), llm)
}

/// `world` with a forge the scripted sessions can also reach (a human acting meanwhile).
fn world_arc(forge: Arc<FakeForge>, llm: FakeLlm) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let origin = fake::origin(tmp.path(), &[]);
    let llm = Arc::new(llm);
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
fn project_budget_and_policy_reach_coder_and_reviewer_jobs() {
    let mut w = world(FakeForge::new("o/r").with_issue(task(1, "Please add it", &["complex", "critical"])), FakeLlm::new().reply(SUMMARY, Some("feature.txt")).reply(REVIEW_OK, None));
    let mut profile = team_core::project::ProjectProfile::from_legacy(&w.ctx.cfg, "o/r");
    profile.budgets.max_turns_complex = Some(7);
    profile.budgets.reviewer_max_turns = Some(9);
    w.ctx.cfg.repos[0].profile = Some(profile);
    assert!(coder().cycle(&w.ctx).unwrap());
    let jobs = w.llm.jobs.lock().unwrap();
    assert_eq!(jobs[0].max_turns, 7);
    assert_eq!(jobs[0].policy.role, team_core::project::SessionRole::Coder);
    assert!(jobs[0].policy.labels.contains(&"complex".into()));
    assert_eq!(jobs[1].max_turns, 9);
    assert_eq!(jobs[1].policy.role, team_core::project::SessionRole::Reviewer);
    assert!(jobs[1].policy.labels.contains(&"critical".into()));
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
    let llm = FakeLlm::new().then(move |_| Err(LlmError::UsageLimit(team_core::config::Backend::Claude, limit))).reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None);
    let w = world(FakeForge::new("o/r").with_issue(task(5, "x", &[])), llm);
    let mut c = coder();
    assert!(matches!(c.cycle(&w.ctx), Err(crate::ctx::RoleError::UsageLimit(_, _))));
    assert!(w.forge.labels(5).contains(&"agent-running".to_string()));
    assert!(c.cycle(&w.ctx).unwrap());
    assert!(w.forge.log().contains(&"close 5".to_string()));
    assert_eq!(w.forge.comment_bodies(5).iter().filter(|b| b.contains("is now working")).count(), 1);
}

#[test]
fn unavailable_provider_waits_without_spending_an_attempt_or_handing_over() {
    let llm = FakeLlm::new().then(|_| Err(LlmError::Unavailable("CLI fehlt".into())));
    let w = world(FakeForge::new("o/r").with_issue(task(5, "x", &[])), llm);
    let mut c = coder();
    for _ in 0..4 { assert!(matches!(c.cycle(&w.ctx), Err(crate::ctx::RoleError::Unavailable(_)))); }
    let state = Store::new(&w.ctx.paths.sessions()).load("o/r", 5).unwrap();
    assert_eq!(state.attempts, 0);
    assert_eq!(state.session_count, 0);
    assert!(!state.handed_to_human);
    assert!(!w.forge.labels(5).contains(&"needs-human".into()));
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
    let w = world(FakeForge::new("o/r").with_issue(task(12, "x", &[])), FakeLlm::new().then(|_| Err(LlmError::UsageLimit(team_core::config::Backend::Claude, chrono::Local::now() + chrono::Duration::hours(1)))));
    let _ = coder().cycle(&w.ctx);
    let mut other = w.ctx.clone();
    other.machine = "OTHERBOX".into();
    assert!(!coder().cycle(&other).unwrap(), "issue is claimed by TESTBOX");
}

#[test]
fn pr_is_marked_running_during_review_and_worktree_is_removed() {
    let w = world(FakeForge::new("o/r").with_issue(task(30, "x", &[])), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    let log = w.forge.log();
    let on = log.iter().position(|l| *l == format!("label+ {} agent-running", pr.number)).expect("PR marked running");
    let off = log.iter().position(|l| *l == format!("label- {} agent-running", pr.number)).expect("PR released");
    assert!(on < off);
    assert!(!w.ctx.paths.worktree("o/r", &pr.head.name).exists());
}

#[test]
fn own_claim_without_session_is_resumed() {
    let w = world(FakeForge::new("o/r").with_issue(fake::issue(31, "t", "x", &["agent-running"])), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    w.forge.add_comment(31, "<!-- team-agent:claim machine=TESTBOX -->\n🤖 Agent `TESTBOX` is now working on this issue...");
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(w.forge.log().contains(&"close 31".to_string()));
}

#[test]
fn silent_foreign_claim_is_requeued_after_a_day() {
    let w = world(FakeForge::new("o/r").with_issue(fake::issue(32, "t", "x", &["agent-running"])), FakeLlm::new().reply("x", None));
    let id = w.forge.add_comment(32, "<!-- team-agent:claim machine=GONE -->\n🤖 Agent `GONE` is now working on this issue...");
    assert!(!coder().cycle(&w.ctx).unwrap(), "fresh foreign claim is respected");
    assert!(w.forge.labels(32).contains(&"agent-running".to_string()));
    w.forge.set_comment_time(32, id, "2020-01-01T00:00:00Z");
    coder().cycle(&w.ctx).unwrap();
    assert!(w.forge.comment_bodies(32).iter().any(|c| c.contains("silent for over 24 hours")));
    assert!(w.forge.log().contains(&"label+ 32 agent-task".to_string()), "back into the queue");
    assert!(w.forge.comment_bodies(32).iter().any(|c| c.contains("machine=TESTBOX")), "and taken over by this machine");
}

#[test]
fn session_cap_hands_over_and_requeue_starts_fresh() {
    let mut w = world(FakeForge::new("o/r").with_issue(task(33, "x", &[])), FakeLlm::new().then(max_turns("a.txt")));
    w.ctx.cfg.budgets.max_sessions = 2;
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(w.forge.labels(33).contains(&"needs-human".to_string()));
    assert!(w.forge.comment_bodies(33).last().unwrap().contains("Handing this issue to a human"));
    assert!(!w.forge.comment_bodies(33).iter().any(|c| c.contains("Attempt")), "not a failed attempt");
    // a human re-queues it
    w.forge.remove_label(33, "needs-human").unwrap();
    w.forge.add_labels(33, &["agent-task"]).unwrap();
    coder().cycle(&w.ctx).unwrap();
    assert_eq!(w.llm.calls(), 4, "two fresh sessions after the re-queue");
}

#[test]
fn failure_after_the_pr_exists_goes_to_a_human() {
    let llm = FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_BLOCKING, None).then(|_| Err(LlmError::Failed("crash".into())));
    let w = world(FakeForge::new("o/r").with_issue(task(34, "x", &["complex"])), llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    let labels = w.forge.labels(34);
    assert!(labels.contains(&"needs-human".to_string()) && !labels.contains(&"agent-task".to_string()));
    assert!(!w.forge.labels(the_pr(&w).number).contains(&"agent-running".to_string()));
}

#[test]
fn human_pr_mentioning_the_issue_is_not_adopted() {
    let mut human = fake::pr(40, "Refactor X", "feature/human", &[]);
    human.body = Some("see also #35".into());
    let w = world(FakeForge::new("o/r").with_issue(task(35, "x", &[])).with_pr(human), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    assert!(coder().cycle(&w.ctx).unwrap());
    let prs = w.forge.s.lock().unwrap().prs.clone();
    assert!(prs.values().any(|p| p.head.name.starts_with("agent/issue-35-")));
    assert!(fake::pushed_file(&w.origin, "feature/human", "f.txt").is_none());
}

fn assigned(mut i: team_core::gh::Issue, login: &str) -> team_core::gh::Issue {
    i.assignees.push(team_core::gh::User { login: login.into() });
    i
}

#[test]
fn issue_assigned_to_the_agents_own_login_is_taken_a_colleagues_is_not() {
    let forge = FakeForge::new("o/r").with_issue(assigned(task(41, "x", &[]), "colleague")).with_issue(assigned(task(42, "x", &[]), "BOT"));
    let w = world(forge, FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(the_pr(&w).head.name.starts_with("agent/issue-42-"));
    assert!(w.forge.labels(41).iter().any(|l| l == "agent-task") && w.forge.comment_bodies(41).is_empty(), "a colleague's issue stays untouched");
}

/// `branch` gets a commit on top of `base` on origin; with `merge_into`, it is then merged
/// there with a merge commit.
fn on_origin(w: &World, branch: &str, base: &str, file: &str, merge_into: Option<&str>) {
    let seed = w._tmp.path().join("seed");
    let g = |a: &[&str]| team_core::git::run_git(Some(&seed), a, false).unwrap();
    let id = ["-c", "user.name=t", "-c", "user.email=t@t"];
    g(&["fetch", "-q", "origin"]);
    g(&["checkout", "-q", "-B", branch, &format!("origin/{base}")]);
    std::fs::write(seed.join(file), file).unwrap();
    g(&["add", "-A"]);
    g(&[&id[..], &["commit", "-q", "-m", file]].concat());
    g(&["push", "-q", "origin", branch]);
    if let Some(into) = merge_into {
        g(&["checkout", "-q", "-B", into, &format!("origin/{into}")]);
        g(&[&id[..], &["merge", "-q", "--no-ff", "-m", "merge", branch]].concat());
        g(&["push", "-q", "origin", into]);
    }
}

/// A feature merged straight into main: dev is bypassed.
fn advance_main(w: &World) {
    on_origin(w, "feat/later", "main", "later.txt", Some("main"));
}

#[test]
fn abandoned_dev_is_bypassed_outside_the_po_repo() {
    let w = world(FakeForge::new("o/r").with_issue(task(43, "x", &[])), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    advance_main(&w);
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    assert_eq!(pr.base.name, "main");
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "later.txt").is_some(), "work starts from the current main");
}

#[test]
fn po_repo_keeps_its_integration_branch_when_behind() {
    let mut w = world(FakeForge::new("o/r").with_issue(task(44, "x", &[])), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    w.ctx.cfg.product_owner.repo = "o/r".into();
    advance_main(&w);
    assert!(coder().cycle(&w.ctx).unwrap());
    assert_eq!(the_pr(&w).base.name, "dev");
}

#[test]
fn git_flow_dev_stays_the_target_after_a_release_merge() {
    let w = world(FakeForge::new("o/r").with_issue(task(45, "x", &[])), FakeLlm::new().reply(SUMMARY, Some("f.txt")).reply(REVIEW_OK, None));
    on_origin(&w, "dev", "dev", "released.txt", Some("main"));
    assert!(coder().cycle(&w.ctx).unwrap());
    assert_eq!(the_pr(&w).base.name, "dev");
}

/// A session that writes `file`, reports `cost` (60 M cache-read tokens, as on TestConsole)
/// and either finishes or runs out of turns.
fn costing(file: &'static str, cost: f64, done: bool) -> impl Fn(&crate::llm::Job) -> Result<Outcome, LlmError> + Send + Sync {
    move |job| {
        std::fs::write(job.cwd.join(file), "work").unwrap();
        let usage = team_core::claude::Usage { cache_read: 60_000_000, ..Default::default() };
        Ok(Outcome { text: if done { SUMMARY } else { "not done" }.into(), reached_max_turns: !done, num_turns: if done { 40 } else { 150 }, cost_usd: cost, usage, ..Default::default() })
    }
}

#[test]
fn a_finished_session_over_budget_still_becomes_a_pr() {
    // TestConsole#26 (2026-11-25) finished, was then stopped by the token count and its
    // commit never left the worktree
    let w = world(FakeForge::new("o/r").with_issue(task(20, "x", &[])), FakeLlm::new().then(costing("quick.txt", 55.0, true)).reply(REVIEW_OK, None));
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "quick.txt").is_some());
    assert!(w.forge.log().contains(&"close 20".to_string()));
    assert!(!w.forge.labels(20).contains(&"needs-human".to_string()));
}

#[test]
fn cached_context_does_not_exhaust_the_budget() {
    // two sessions of 60 M cache-read tokens each: 120 M tokens, but 24 USD of a 40 USD budget
    let llm = FakeLlm::new().then(costing("part1.txt", 12.0, false)).then(costing("part2.txt", 12.0, true)).reply(REVIEW_OK, None);
    let w = world(FakeForge::new("o/r").with_issue(task(21, "x", &[])), llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    let pr = the_pr(&w);
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "part1.txt").is_some());
    assert!(fake::pushed_file(&w.origin, &pr.head.name, "part2.txt").is_some());
}

#[test]
fn an_unfinished_session_over_budget_pushes_its_work_and_hands_over() {
    let w = world(FakeForge::new("o/r").with_issue(task(22, "x", &[])), FakeLlm::new().then(costing("wip.txt", 45.0, false)));
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(w.forge.s.lock().unwrap().prs.is_empty());
    let last = w.forge.comment_bodies(22).last().unwrap().clone();
    assert!(last.contains("**Budget reached**") && last.contains("$45.00 of its $40.00 budget"), "{last}");
    let branch = Store::new(&w.ctx.paths.sessions()).load("o/r", 22).unwrap().branch_name;
    assert!(last.contains(&format!("Work in progress is on branch `{branch}`")), "{last}");
    assert!(fake::pushed_file(&w.origin, &branch, "wip.txt").is_some(), "the work survives on origin");
    assert!(w.forge.labels(22).contains(&"needs-human".to_string()));
    assert_eq!(w.llm.calls(), 1);
}

#[test]
fn a_stop_posted_late_in_a_session_is_caught_before_the_pr() {
    // the session ends before the next background check: the check after it still sees it
    let forge = Arc::new(FakeForge::new("o/r").with_issue(task(23, "x", &[])));
    let human = forge.clone();
    let llm = FakeLlm::new().then(move |job| {
        std::fs::write(job.cwd.join("half.txt"), "half").unwrap();
        human.add_comment(23, "/agent stop — Nik takes this one over");
        Ok(Outcome { text: SUMMARY.into(), num_turns: 30, ..Default::default() })
    });
    let w = world_arc(forge, llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(w.forge.s.lock().unwrap().prs.is_empty(), "no PR for an issue a human took over");
    let last = w.forge.comment_bodies(23).last().unwrap().clone();
    assert!(last.starts_with("🛑 The agent stopped working on this issue: `someone` asked the agent to stop."), "{last}");
    let branch = Store::new(&w.ctx.paths.sessions()).load("o/r", 23).unwrap().branch_name;
    assert!(fake::pushed_file(&w.origin, &branch, "half.txt").is_some());
    let labels = w.forge.labels(23);
    assert!(!labels.iter().any(|l| l == "agent-running" || l == "needs-human"), "{labels:?}");
    assert!(!coder().cycle(&w.ctx).unwrap(), "a stopped issue is not picked up again");
}

#[test]
fn a_human_pr_fixing_the_issue_keeps_the_agent_from_starting() {
    let mut theirs = fake::pr(60, "Lab fixes batch 5", "fix/lab-batch-5", &[]);
    theirs.body = Some("Fixes #24".into());
    let w = world(FakeForge::new("o/r").with_issue(task(24, "x", &[])).with_pr(theirs), FakeLlm::new().reply(SUMMARY, Some("f.txt")));
    assert!(coder().cycle(&w.ctx).unwrap());
    assert_eq!(w.llm.calls(), 0, "no session for duplicate work");
    let last = w.forge.comment_bodies(24).last().unwrap().clone();
    assert!(last.contains("PR #60 already fixes this issue") && !last.contains("Work in progress"), "{last}");
    assert!(!w.forge.labels(24).iter().any(|l| l == "agent-running" || l == "needs-human"));
    let branches = team_core::git::run_git(None, &["--git-dir", &w.origin.display().to_string(), "branch", "--list", "agent/*"], false).unwrap();
    assert!(branches.trim().is_empty(), "no empty agent branch on origin: {branches}");
}

#[test]
fn a_session_ended_by_the_watch_is_a_stop_not_a_failed_attempt() {
    let stopped = |_: &crate::llm::Job| Err(LlmError::Failed(format!("{}: the issue was assigned to `maddnik`", team_core::claude::STOPPED_TEXT)));
    let w = world(FakeForge::new("o/r").with_issue(task(25, "x", &[])), FakeLlm::new().then(stopped));
    assert!(coder().cycle(&w.ctx).unwrap());
    let comments = w.forge.comment_bodies(25);
    assert!(comments.last().unwrap().starts_with("🛑 The agent stopped working on this issue: the issue was assigned to `maddnik`."));
    assert!(!comments.iter().any(|c| c.contains("failed")), "not counted as an attempt: {comments:?}");
    assert!(!w.forge.labels(25).iter().any(|l| l == "agent-task" || l == "needs-human"));
}

#[test]
fn a_stop_command_during_the_session_ends_it_and_keeps_the_work() {
    let forge = Arc::new(FakeForge::new("o/r").with_issue(task(26, "x", &[])));
    let human = forge.clone();
    // the session runs on, as a real one does, until its stop check fires
    let llm = FakeLlm::new().then_watched(move |job, stop| {
        std::fs::write(job.cwd.join("half.txt"), "half").unwrap();
        human.add_comment(26, "/agent stop");
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            if let Some(reason) = stop() {
                return Err(LlmError::Failed(format!("{}: {reason}", team_core::claude::STOPPED_TEXT)));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(Outcome { text: "still running".into(), num_turns: 30, ..Default::default() })
    });
    let w = world_arc(forge, llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    let last = w.forge.comment_bodies(26).last().unwrap().clone();
    assert!(last.starts_with("🛑 The agent stopped working on this issue: `someone` asked the agent to stop."), "{last}");
    let branch = Store::new(&w.ctx.paths.sessions()).load("o/r", 26).unwrap().branch_name;
    assert!(fake::pushed_file(&w.origin, &branch, "half.txt").is_some());
    assert!(w.forge.s.lock().unwrap().prs.is_empty());
}

#[test]
fn an_issue_closed_during_the_session_ends_it_silently() {
    let forge = Arc::new(FakeForge::new("o/r").with_issue(task(27, "x", &[])));
    let human = forge.clone();
    let llm = FakeLlm::new().then(move |job| {
        std::fs::write(job.cwd.join("late.txt"), "x").unwrap();
        human.s.lock().unwrap().issues.get_mut(&27).unwrap().state = "closed".into();
        Ok(Outcome { text: SUMMARY.into(), num_turns: 30, ..Default::default() })
    });
    let w = world_arc(forge, llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    assert!(w.forge.s.lock().unwrap().prs.is_empty());
    assert!(!w.forge.comment_bodies(27).iter().any(|c| c.contains("🛑")), "nobody needs a note on a closed issue");
    assert!(Store::new(&w.ctx.paths.sessions()).load("o/r", 27).is_none());
    assert!(!w.forge.labels(27).iter().any(|l| l == "agent-running" || l == "needs-human"));
}

#[test]
fn a_takeover_while_paused_by_the_usage_limit_is_seen_on_resume() {
    let limit = chrono::Local::now() + chrono::Duration::hours(1);
    let llm = FakeLlm::new().then(move |_| Err(LlmError::UsageLimit(team_core::config::Backend::Claude, limit))).reply(SUMMARY, Some("f.txt"));
    let w = world(FakeForge::new("o/r").with_issue(task(28, "x", &[])), llm);
    let mut c = coder();
    assert!(matches!(c.cycle(&w.ctx), Err(crate::ctx::RoleError::UsageLimit(_, _))));
    // during the pause Nik takes the issue
    w.forge.s.lock().unwrap().issues.get_mut(&28).unwrap().assignees.push(team_core::gh::User { login: "MaddNik".into() });
    assert!(c.cycle(&w.ctx).unwrap());
    assert_eq!(w.llm.calls(), 1, "no second session");
    assert!(w.forge.comment_bodies(28).last().unwrap().contains("the issue was assigned to `maddnik`"));
}

#[test]
fn a_stop_shortly_before_a_crash_is_a_stop_not_a_requeue() {
    let forge = Arc::new(FakeForge::new("o/r").with_issue(task(29, "x", &[])));
    let human = forge.clone();
    let llm = FakeLlm::new().then(move |_| {
        human.add_comment(29, "/agent stop");
        Err(LlmError::Failed("Claude hing (1800 s ohne Ausgabe) und wurde beendet".into()))
    });
    let w = world_arc(forge, llm);
    assert!(coder().cycle(&w.ctx).unwrap());
    let comments = w.forge.comment_bodies(29);
    assert!(comments.last().unwrap().contains("asked the agent to stop"), "{comments:?}");
    assert!(!comments.iter().any(|c| c.contains("failed")), "no failed attempt: {comments:?}");
    assert!(!w.forge.labels(29).iter().any(|l| l == "agent-task"), "not re-queued");
}
