//! Product Owner end to end: scripted Claude, simulated GitHub, real git against a local origin.

use super::*;
use crate::fake::{self, FakeForge, FakeLlm};
use crate::forge::Forge;
use crate::llm::LlmError;
use std::sync::Arc;

struct World {
    tmp: tempfile::TempDir,
    origin: std::path::PathBuf,
    forge: Arc<FakeForge>,
    llm: Arc<FakeLlm>,
}

fn world(forge: FakeForge, llm: FakeLlm) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let origin = fake::origin(tmp.path(), &[]);
    World { tmp, origin, forge: Arc::new(forge), llm: Arc::new(llm) }
}

impl World {
    /// The PO on machine `name` (own data folder, same GitHub).
    fn ctx(&self, name: &str) -> Ctx {
        let dir = self.tmp.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = fake::ctx(&dir, &self.origin, self.forge.clone(), self.llm.clone());
        ctx.machine = name.into();
        ctx.cfg.product_owner.repo = "o/r".into();
        ctx
    }
}

fn with_agent_pr() -> FakeForge {
    FakeForge::new("o/r").with_pr(fake::pr(1, "Agent: thing", "agent/issue-9-1", &["agent-pr"]))
}

fn state(ctx: &Ctx) -> PoState {
    PoState::load(&ctx.paths.state())
}

#[test]
fn one_pass_per_slot_with_the_backlog_in_the_prompt() {
    let w = world(with_agent_pr(), FakeLlm::new().reply("Merged PR #1.", None));
    let ctx = w.ctx("A");
    let mut po = ProductOwner::default();
    assert_eq!(po.cycle(&ctx), Ok(true));
    let job = w.llm.jobs.lock().unwrap()[0].clone();
    for needle in ["Product-Owner pass — o/r", "- PR #1: Agent: thing (branch agent/issue-9-1)", "(docs/ROADMAP.md not found in clone)", "`dev`", "Customer automation is disabled"] {
        assert!(job.prompt.contains(needle), "prompt lacks {needle}");
    }
    assert!(!job.prompt.contains("{REPO}"), "all placeholders filled");
    assert_eq!(job.model, ctx.cfg.models.product_owner);
    assert_eq!(job.max_duration, Some(Duration::from_secs(60 * 60)));
    assert_eq!(job.env.set.get("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS").map(String::as_str), Some("1"));
    assert!(job.env.remove.iter().any(|v| v == "ANTHROPIC_API_KEY"), "runs on the subscription");
    assert_eq!(job.label, "po_o_r");
    let st = state(&ctx);
    assert_eq!((st.last_result.as_str(), st.runs_today), ("ok", 1));
    assert!(st.last_end.is_some() && st.next_due.is_some());
    assert_eq!(po.cycle(&ctx), Ok(false), "not again in the same slot");
    assert_eq!(w.llm.calls(), 1);
}

#[test]
fn only_one_machine_runs_a_slot() {
    let w = world(with_agent_pr(), FakeLlm::new().reply("done", None));
    let (a, b) = (w.ctx("A"), w.ctx("B"));
    assert_eq!(ProductOwner::default().cycle(&a), Ok(true));
    assert_eq!(ProductOwner::default().cycle(&b), Ok(false));
    assert_eq!(w.llm.calls(), 1);
    let st = state(&b);
    assert_eq!(st.last_result, "skipped");
    assert!(st.last_note.contains("anderen Rechner"));
}

#[test]
fn healthy_backlog_skips_without_claude_or_lock() {
    let mut forge = FakeForge::new("o/r");
    for n in 1..=5 {
        forge = forge.with_issue(fake::issue(n, "task", "", &["agent-task"]));
    }
    let w = world(forge, FakeLlm::new().reply("x", None));
    let ctx = w.ctx("A");
    assert_eq!(ProductOwner::default().cycle(&ctx), Ok(false));
    assert_eq!(w.llm.calls(), 0);
    assert!(!w.forge.log().iter().any(|l| l.starts_with("lock")));
    assert_eq!(state(&ctx).last_result, "skipped");
}

#[test]
fn usage_limit_gives_the_slot_back() {
    let until = Local::now() + chrono::Duration::hours(1);
    let w = world(with_agent_pr(), FakeLlm::new().then(move |_| Err(LlmError::UsageLimit(until))).reply("done", None));
    let ctx = w.ctx("A");
    let mut po = ProductOwner::default();
    assert_eq!(po.cycle(&ctx), Err(RoleError::UsageLimit(until)));
    let st = state(&ctx);
    assert_eq!((st.last_result.as_str(), st.runs_today), ("usage-limit", 0));
    assert!(w.forge.s.lock().unwrap().locks.is_empty(), "slot released");
    assert_eq!(po.cycle(&ctx), Ok(true), "runs once the limit is over");
}

#[test]
fn timeout_and_errors_are_recorded() {
    let msg = format!("{} (3600 s) — Claude wurde beendet", team_core::claude::TIMED_OUT_TEXT);
    let w = world(with_agent_pr(), FakeLlm::new().then(move |_| Err(LlmError::Failed(msg.clone()))));
    let ctx = w.ctx("A");
    assert_eq!(ProductOwner::default().cycle(&ctx), Ok(true));
    assert_eq!(state(&ctx).last_result, "timeout");
}

#[test]
fn missing_integration_branch_is_created_from_base() {
    let w = world(with_agent_pr(), FakeLlm::new().reply("done", None));
    let mut ctx = w.ctx("A");
    ctx.cfg.product_owner.integration_branch = "integration".into();
    assert_eq!(ProductOwner::default().cycle(&ctx), Ok(true));
    assert!(fake::pushed_file(&w.origin, "integration", "README.md").is_some());
    assert!(w.llm.prompt(0).contains("`integration`"));
}

#[test]
fn customer_review_evidence_goes_into_the_prompt() {
    let w = world(FakeForge::new("o/r"), FakeLlm::new().reply("done", None));
    let mut ctx = w.ctx("A");
    ctx.cfg.product_owner.customer_command = if cfg!(windows) { "echo PASSED> \"%LUNIMA_CUSTOMER_FEEDBACK%\"".into() } else { "echo PASSED > \"$LUNIMA_CUSTOMER_FEEDBACK\"".into() };
    assert_eq!(ProductOwner::default().cycle(&ctx), Ok(true), "a customer review alone makes a pass worth it");
    let feedback = pass::feedback_path(&ctx);
    assert!(std::fs::read_to_string(&feedback).unwrap().contains("PASSED"));
    let p = w.llm.prompt(0);
    assert!(p.contains("Customer automation is ENABLED") && p.contains(&feedback.display().to_string()));
}

#[test]
fn no_po_repo_means_no_work() {
    let w = world(with_agent_pr(), FakeLlm::new().reply("x", None));
    let mut ctx = w.ctx("A");
    ctx.cfg.product_owner.repo.clear();
    assert_eq!(ProductOwner::default().cycle(&ctx), Ok(false));
    assert_eq!(w.llm.calls(), 0);
}

#[test]
fn a_running_pass_elsewhere_blocks_the_next_slot_until_stale() {
    let w = world(with_agent_pr(), FakeLlm::new().reply("done", None));
    let a = w.ctx("A");
    w.forge.try_lock(RUN_LOCK, "B").unwrap(); // B is still in a pass
    let mut po = ProductOwner::default();
    assert_eq!(po.cycle(&a), Ok(false));
    assert_eq!(w.llm.calls(), 0);
    assert!(!w.forge.s.lock().unwrap().locks.keys().any(|k| k.starts_with("po-slot")), "slot not taken");
    w.forge.age_lock(RUN_LOCK, Duration::from_secs(80 * 60)); // 60 min limit + 10 min grace passed
    let mut po = ProductOwner::default();
    assert_eq!(po.cycle(&a), Ok(true));
    assert!(!w.forge.s.lock().unwrap().locks.contains_key(RUN_LOCK), "run lock released after the pass");
}
