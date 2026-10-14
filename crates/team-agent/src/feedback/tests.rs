//! PR-feedback role against a fake GitHub and a real local git origin.

use super::thread::*;
use super::*;
use crate::ctx::RoleError;
use crate::fake::{self, FakeForge, FakeLlm};
use crate::llm::LlmError;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use team_core::claude::Outcome;
use team_core::config::Routing;
use team_core::gh::{Label, RepoName};

const ON: &str = "agents_enabled = [\"coder\", \"pr-feedback\"]\n";
const TRIGGER: &str = "@agent please rename the button";
const REPORT: &str = "Done.\n=== FEEDBACK REPORT ===\n- renamed the button\n=== END ===";

struct Env {
    t: tempfile::TempDir,
    origin: PathBuf,
    forge: Arc<FakeForge>,
}

fn env(toml: &str, head_repo: Option<&str>) -> Env {
    let t = tempfile::tempdir().unwrap();
    let files: Vec<(&str, &str)> = if toml.is_empty() { vec![] } else { vec![(".agent.toml", toml)] };
    let origin = fake::origin(t.path(), &files);
    team_core::git::run_git(None, &["--git-dir", &origin.display().to_string(), "branch", "agent/issue-5", "main"], false).unwrap();
    let mut p = fake::pr(7, "Agent: thing", "agent/issue-5", &[]);
    p.body = Some("Automated implementation for #5\n".into());
    p.head.repo = head_repo.map(|r| RepoName { full_name: r.into() });
    let forge = Arc::new(FakeForge::new("o/r").with_pr(p).with_issue(fake::issue(5, "thing", "", &[])));
    Env { t, origin, forge }
}

impl Env {
    fn ctx(&self, sub: &str, llm: &Arc<FakeLlm>) -> Ctx {
        let dir = self.t.path().join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        fake::ctx(&dir, &self.origin, self.forge.clone(), llm.clone())
    }
    fn comments(&self) -> Vec<String> {
        self.forge.comment_bodies(7)
    }
    fn count(&self, needle: &str) -> usize {
        self.comments().iter().filter(|c| c.contains(needle)).count()
    }
}

fn ok(text: &str) -> Outcome {
    Outcome { text: text.into(), subtype: "success".into(), num_turns: 2, ..Default::default() }
}

#[test]
fn handles_a_request_end_to_end() {
    let e = env(ON, Some("o/r"));
    let trigger = e.forge.add_comment(7, TRIGGER);
    let llm = Arc::new(FakeLlm::new().reply(REPORT, Some("button.txt")));
    let ctx = e.ctx("a", &llm);
    let mut role = PrFeedback::new();
    assert_eq!(role.cycle(&ctx), Ok(true));
    let c = e.comments();
    assert_eq!(c.len(), 3, "trigger, claim, reply: {c:#?}");
    assert!(c[1].contains(&format!("<!-- pr-feedback:claim {trigger} machine=TESTBOX -->")));
    assert!(c[2].starts_with(REPLY_MARKER) && c[2].contains("### What changed\n- renamed the button"));
    assert!(c[2].contains(&format!("#issuecomment-{trigger}")) && c[2].contains(&format!("<!-- pr-feedback:done {trigger} -->")));
    assert!(c[2].contains("_No UI screenshots were produced for this round._"));
    assert!(!c[2].contains("no new commits"));
    assert!(fake::pushed_file(&e.origin, "agent/issue-5", "button.txt").is_some());
    let p = llm.prompt(0);
    for needle in ["PR #7", "agent/issue-5", TRIGGER, "artifacts/ui-screenshots/issue-5/", "FEEDBACK REPORT"] {
        assert!(p.contains(needle), "prompt lacks {needle}");
    }
    assert_eq!(role.cycle(&ctx), Ok(false), "nothing left to do");
    assert_eq!(llm.calls(), 1);
}

#[test]
fn no_change_is_reported() {
    let e = env(ON, Some("o/r"));
    e.forge.add_comment(7, TRIGGER);
    let llm = Arc::new(FakeLlm::new().reply(REPORT, None));
    PrFeedback::new().cycle(&e.ctx("a", &llm)).unwrap();
    assert_eq!(e.count("no new commits were pushed"), 1);
}

#[test]
fn forks_are_declined_without_claude() {
    for head in [Some("fork/r"), None] {
        let e = env(ON, head);
        let id = e.forge.add_comment(7, TRIGGER);
        let llm = Arc::new(FakeLlm::new().reply("x", None));
        assert_eq!(PrFeedback::new().cycle(&e.ctx("a", &llm)), Ok(true));
        assert_eq!(e.count("lives in a fork"), 1);
        assert_eq!(e.count(&format!("<!-- pr-feedback:done {id} -->")), 1);
        assert!(e.count("(deleted fork)") == usize::from(head.is_none()));
        assert_eq!(llm.calls(), 0);
    }
}

#[test]
fn disabled_or_broken_policy_is_declined() {
    for (toml, broken) in [("agents_enabled = [\"coder\"]\n", false), ("", false), ("agents_enabled = [", true)] {
        let e = env(toml, Some("o/r"));
        e.forge.add_comment(7, TRIGGER);
        let llm = Arc::new(FakeLlm::new().reply("x", None));
        assert_eq!(PrFeedback::new().cycle(&e.ctx("a", &llm)), Ok(true));
        assert_eq!(e.count("pr-feedback role is not enabled"), 1);
        assert_eq!(e.count("could not be read"), usize::from(broken));
        assert_eq!(llm.calls(), 0);
    }
}

#[test]
fn second_machine_does_not_take_a_claimed_request() {
    let e = env(ON, Some("o/r"));
    e.forge.add_comment(7, TRIGGER);
    let llm_b = Arc::new(FakeLlm::new().reply("x", None));
    let mut ctx_b = e.ctx("b", &llm_b);
    ctx_b.machine = "OTHER".into();
    let b = Arc::new(Mutex::new(PrFeedback::new()));
    let (b2, ctx_b2) = (b.clone(), ctx_b.clone());
    let llm_a = Arc::new(FakeLlm::new().then(move |_| {
        // while A works, B looks at the same PR
        assert_eq!(b2.lock().unwrap().cycle(&ctx_b2), Ok(false));
        Ok(ok(REPORT))
    }));
    assert_eq!(PrFeedback::new().cycle(&e.ctx("a", &llm_a)), Ok(true));
    assert_eq!(e.count("<!-- pr-feedback:claim"), 1);
    assert_eq!(e.count("<!-- pr-feedback:done"), 1);
    assert_eq!(b.lock().unwrap().cycle(&ctx_b), Ok(false));
    assert_eq!(llm_b.calls(), 0);
}

#[test]
fn failure_is_retried_then_succeeds() {
    let e = env(ON, Some("o/r"));
    e.forge.add_comment(7, TRIGGER);
    let llm = Arc::new(FakeLlm::new().then(|_| Err(LlmError::Failed("boom".into()))).reply(REPORT, Some("f.txt")));
    let ctx = e.ctx("a", &llm);
    let mut role = PrFeedback::new();
    assert_eq!(role.cycle(&ctx), Ok(true));
    assert_eq!(e.count("Attempt 1"), 1);
    assert_eq!(e.count("<!-- pr-feedback:done"), 0);
    assert_eq!(role.cycle(&ctx), Ok(true));
    assert_eq!(e.count("### What changed"), 1);
    assert_eq!(llm.calls(), 2);
}

#[test]
fn abandoned_claim_is_retried_and_second_failure_gives_up() {
    let e = env(ON, Some("o/r"));
    let id = e.forge.add_comment(7, TRIGGER);
    let ghost = e.forge.add_comment(7, &claim_comment(id, "GHOST"));
    e.forge.set_comment_time(7, ghost, &(Utc::now() - chrono::Duration::hours(4)).to_rfc3339());
    let llm = Arc::new(FakeLlm::new().then(|_| Err(LlmError::Failed("boom".into()))));
    let ctx = e.ctx("a", &llm);
    let mut role = PrFeedback::new();
    assert_eq!(role.cycle(&ctx), Ok(true));
    assert_eq!(llm.calls(), 1);
    assert_eq!(e.count("couldn't complete this request after 2 attempts (last error: `boom`)"), 1);
    assert_eq!(e.count(&format!("<!-- pr-feedback:done {id} -->")), 1);
    assert_eq!(role.cycle(&ctx), Ok(false));
}

#[test]
fn usage_limit_leaves_the_request_open() {
    let e = env(ON, Some("o/r"));
    e.forge.add_comment(7, TRIGGER);
    let until = chrono::Local::now() + chrono::Duration::hours(1);
    let llm = Arc::new(FakeLlm::new().then(move |_| Err(LlmError::UsageLimit(until))));
    assert_eq!(PrFeedback::new().cycle(&e.ctx("a", &llm)), Err(RoleError::UsageLimit(until)));
    assert_eq!(e.count("<!-- pr-feedback:done"), 0);
    assert_eq!(e.count("<!-- pr-feedback:failed"), 0);
    assert_eq!(e.count("<!-- pr-feedback:claim"), 1);
}

#[test]
fn cap_notice_is_posted_once() {
    let e = env(ON, Some("o/r"));
    let first = e.forge.add_comment(7, "@agent first");
    e.forge.add_comment(7, &format!("{REPLY_MARKER}\nDone\n<!-- pr-feedback:done {first} -->"));
    e.forge.add_comment(7, "@agent second");
    let llm = Arc::new(FakeLlm::new().reply("x", None));
    let mut ctx = e.ctx("a", &llm);
    ctx.cfg.budgets.pr_feedback_max_rounds = 1;
    let mut role = PrFeedback::new();
    assert_eq!(role.cycle(&ctx), Ok(false));
    assert_eq!(role.cycle(&ctx), Ok(false));
    assert_eq!(e.count("<!-- pr-feedback:cap -->"), 1);
    assert_eq!(llm.calls(), 0);
}

#[test]
fn provider_policy_uses_pr_and_issue_labels() {
    let e = env(ON, Some("o/r"));
    e.forge.add_comment(7, TRIGGER);
    let llm = Arc::new(FakeLlm::new().reply(REPORT, None));
    let mut ctx = e.ctx("a", &llm);
    ctx.cfg.models.coder = "coder-x".into();
    ctx.cfg.models.premium = "premium-x".into();
    e.forge.s.lock().unwrap().issues.get_mut(&5).unwrap().labels.push(Label { name: "ClaudeAPI".into() });
    PrFeedback::new().cycle(&ctx).unwrap();
    assert_eq!(llm.jobs.lock().unwrap()[0].model, "premium-x");
}

#[test]
fn kimi_only_repo_without_key_is_skipped() {
    let e = env(ON, Some("o/r"));
    e.forge.add_comment(7, TRIGGER);
    let llm = Arc::new(FakeLlm::new().reply(REPORT, None));
    let mut ctx = e.ctx("a", &llm);
    ctx.cfg.repos[0].routing = Routing::KimiOnly;
    ctx.cfg.providers.openrouter.key_env = "LUNIMA_TEST_NO_SUCH_KEY".into();
    ctx.cfg.providers.openrouter.key_file = String::new();
    assert_eq!(PrFeedback::new().cycle(&ctx), Ok(false));
    assert_eq!(e.comments().len(), 1, "only the trigger");
    assert_eq!(llm.calls(), 0);
}
