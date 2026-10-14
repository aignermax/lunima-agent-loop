use super::*;
use crate::fake::{self, pr, FakeForge, FakeLlm};
use crate::llm::LlmError;
use std::path::Path;
use std::sync::Arc;
use team_core::git::run_git;

struct Env {
    _tmp: tempfile::TempDir,
    forge: Arc<FakeForge>,
    llm: Arc<FakeLlm>,
    ctx: Ctx,
}

/// Pushes branch `name` (from main) with `files` to the fake origin.
fn branch(tmp: &Path, name: &str, files: &[(&str, &str)]) {
    let seed = tmp.join("seed");
    let g = |a: &[&str]| run_git(Some(&seed), a, false).unwrap();
    g(&["checkout", "-q", "-B", name, "origin/main"]);
    for (f, text) in files {
        std::fs::write(seed.join(f), text).unwrap();
    }
    g(&["add", "-A"]);
    g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", name]);
    g(&["push", "-q", "-f", "origin", name]);
}

fn toml(enabled: &str, build: &str, test: &str, review: bool) -> String {
    format!("agents_enabled = [{enabled}]\nbuild_cmd = \"{build}\"\ntest_cmd = \"{test}\"\ncommand_timeout_sec = 120\nqa_review_enabled = {review}\n")
}

fn env(prs: &[(u64, &str, &[&str], Option<String>)], llm: FakeLlm) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let origin = fake::origin(tmp.path(), &[]);
    let mut forge = FakeForge::new("o/r");
    for (n, br, labels, agent_toml) in prs {
        let files: Vec<(&str, &str)> = agent_toml.as_deref().map(|t| vec![(".agent.toml", t)]).unwrap_or_default();
        branch(tmp.path(), br, &files);
        forge = forge.with_pr(pr(*n, &format!("Agent: change {n}"), br, labels));
    }
    let (forge, llm) = (Arc::new(forge), Arc::new(llm));
    let ctx = fake::ctx(tmp.path(), &origin, forge.clone(), llm.clone());
    Env { _tmp: tmp, forge, llm, ctx }
}

fn review_text(verdict: &str) -> String {
    format!("=== REVIEW RESULT ===\nVERDICT: {verdict}\nSUMMARY: the summary line\n=== FINDINGS ===\n- [MAJOR] a finding\n=== END ===")
}

fn one(t: String) -> Vec<(u64, &'static str, &'static [&'static str], Option<String>)> {
    vec![(1, "agent/a", &[], Some(t))]
}

#[test]
fn build_and_test_ok_passes() {
    let e = env(&one(toml("\"qa\"", "exit 0", "exit 0", false)), FakeLlm::new());
    assert!(Qa::default().cycle(&e.ctx).unwrap());
    let log = e.forge.log();
    assert!(log.contains(&"label+ 1 qa-running".to_string()) && log.contains(&"label+ 1 qa-passed".to_string()));
    let c = e.forge.comment_bodies(1);
    assert_eq!(c.len(), 1);
    assert!(c[0].starts_with("[qa-agent] **PASSED**") && c[0].contains("`build`: ok") && c[0].contains("`test`: ok"));
    assert!(c[0].contains("`ui_test`: skipped"));
    assert_eq!(e.forge.labels(1), vec!["qa-passed"]);
    assert!(claims::load(&e.ctx.paths.state()).is_empty());
}

#[test]
fn failed_build_skips_tests() {
    let e = env(&one(toml("\"qa\"", "exit 1", "exit 0", false)), FakeLlm::new());
    Qa::default().cycle(&e.ctx).unwrap();
    let c = &e.forge.comment_bodies(1)[0];
    assert!(c.starts_with("[qa-agent] **FAILED**") && c.contains("`test`: skipped (build failed)"));
    assert_eq!(e.forge.labels(1), vec!["qa-failed"]);
}

#[test]
fn failed_test_reports_exit_code() {
    let e = env(&one(toml("\"qa\"", "exit 0", "exit 2", false)), FakeLlm::new());
    Qa::default().cycle(&e.ctx).unwrap();
    assert!(e.forge.comment_bodies(1)[0].contains("`test`: failed (exit 2)"));
    assert_eq!(e.forge.labels(1), vec!["qa-failed"]);
}

#[test]
fn qa_disabled_releases_without_verdict_and_next_pr_is_checked() {
    let prs = vec![
        (1, "agent/a", &[][..], Some(toml("\"coder\"", "exit 0", "exit 0", false))),
        (2, "agent/b", &[][..], None),
        (3, "agent/c", &[][..], Some(toml("\"qa\"", "exit 0", "exit 0", false))),
    ];
    let e = env(&prs, FakeLlm::new());
    let mut qa = Qa::default();
    assert!(qa.cycle(&e.ctx).unwrap());
    for n in [1, 2] {
        assert!(e.forge.comment_bodies(n).is_empty());
        assert!(e.forge.labels(n).is_empty());
        assert!(e.forge.log().contains(&format!("label- {n} qa-running")));
    }
    assert_eq!(e.forge.labels(3), vec!["qa-passed"]);
    // the disabled PRs are not claimed again while their head is unchanged
    let before = e.forge.log().len();
    assert!(!qa.cycle(&e.ctx).unwrap());
    assert!(!e.forge.log()[before..].iter().any(|l| l.starts_with("label+")));
}

#[test]
fn review_disabled_never_calls_claude() {
    let e = env(&one(toml("\"qa\"", "exit 0", "", false)), FakeLlm::new().reply(&review_text("OK"), None));
    Qa::default().cycle(&e.ctx).unwrap();
    assert_eq!(e.llm.calls(), 0);
}

#[test]
fn review_ok_passes_and_is_reported() {
    let e = env(&one(toml("\"qa\"", "exit 0", "", true)), FakeLlm::new().reply(&review_text("OK"), None));
    Qa::default().cycle(&e.ctx).unwrap();
    assert_eq!(e.llm.calls(), 1);
    let job = e.llm.jobs.lock().unwrap()[0].clone();
    assert!(job.prompt.contains("main"), "base branch of the PR goes into the prompt");
    assert_eq!(job.model, e.ctx.cfg.models.reviewer);
    let c = &e.forge.comment_bodies(1)[0];
    assert!(c.contains("Claude review: **OK**"));
    assert_eq!(e.forge.labels(1), vec!["qa-passed"]);
}

#[test]
fn review_blocking_fails_mechanically_green_pr() {
    let e = env(&one(toml("\"qa\"", "exit 0", "", true)), FakeLlm::new().reply(&review_text("BLOCKING"), None));
    Qa::default().cycle(&e.ctx).unwrap();
    let c = &e.forge.comment_bodies(1)[0];
    assert!(c.starts_with("[qa-agent] **FAILED**") && c.contains("BLOCKING") && c.contains("the summary line"));
    assert_eq!(e.forge.labels(1), vec!["qa-failed"]);
}

#[test]
fn mechanical_failure_skips_review() {
    let e = env(&one(toml("\"qa\"", "exit 1", "", true)), FakeLlm::new().reply(&review_text("OK"), None));
    Qa::default().cycle(&e.ctx).unwrap();
    assert_eq!(e.llm.calls(), 0);
}

#[test]
fn review_crash_blocks() {
    let e = env(&one(toml("\"qa\"", "exit 0", "", true)), FakeLlm::new().then(|_| Err(LlmError::Failed("exploded".into()))));
    Qa::default().cycle(&e.ctx).unwrap();
    let c = &e.forge.comment_bodies(1)[0];
    assert!(c.contains("QA review could not run: exploded"));
    assert_eq!(e.forge.labels(1), vec!["qa-failed"]);
}

#[test]
fn usage_limit_releases_the_claim_without_verdict() {
    let until = chrono::Local::now() + chrono::Duration::hours(2);
    let e = env(&one(toml("\"qa\"", "exit 0", "", true)), FakeLlm::new().then(move |_| Err(LlmError::UsageLimit(until))));
    assert_eq!(Qa::default().cycle(&e.ctx), Err(RoleError::UsageLimit(until)));
    assert!(e.forge.labels(1).is_empty());
    assert!(e.forge.comment_bodies(1).is_empty());
    assert!(claims::load(&e.ctx.paths.state()).is_empty());
}

#[test]
fn no_commands_configured_fails_with_reason() {
    let e = env(&one(toml("\"qa\"", "", "", false)), FakeLlm::new());
    Qa::default().cycle(&e.ctx).unwrap();
    let c = &e.forge.comment_bodies(1)[0];
    assert!(c.starts_with("[qa-agent] **FAILED**") && c.contains(NO_COMMANDS));
}

#[test]
fn broken_agent_toml_fails_visibly() {
    let e = env(&one("agents_enabled = [".into()), FakeLlm::new());
    Qa::default().cycle(&e.ctx).unwrap();
    let c = &e.forge.comment_bodies(1)[0];
    assert!(c.starts_with("[qa-agent] **FAILED**") && c.contains("`.agent.toml`: failed"));
}

#[test]
fn checkout_failure_releases_the_label() {
    let e = env(&[], FakeLlm::new());
    let forge = Arc::new(FakeForge::new("o/r").with_pr(pr(5, "Agent: ghost", "agent/missing", &[])));
    let ctx = Ctx { forge: { let f = forge.clone(); Arc::new(move |_| f.clone() as Arc<dyn Forge>) }, ..e.ctx.clone() };
    let mut qa = Qa::default();
    assert_eq!(qa.cycle(&ctx), Ok(false));
    assert!(forge.labels(5).is_empty());
    assert!(claims::load(&ctx.paths.state()).is_empty());
    let locks_before = forge.log().iter().filter(|l| l.starts_with("lock")).count();
    assert_eq!(qa.cycle(&ctx), Ok(false));
    assert_eq!(forge.log().iter().filter(|l| l.starts_with("lock")).count(), locks_before, "skipped until its head changes");
}

#[test]
fn selection_skips_labelled_and_foreign_prs() {
    let labels = Labels::default();
    let prs = vec![
        pr(1, "Fix by hand", "x", &[]),
        pr(2, "Agent: a", "a", &["Needs-Human"]),
        pr(3, "Agent: b", "b", &["qa-passed"]),
        pr(4, "Agent: c", "c", &["QA-Running"]),
        pr(5, "Agent: d", "d", &["agent-pr"]),
        pr(6, "Agent: e", "e", &[]),
    ];
    let got: Vec<u64> = eligible(&prs, &labels).iter().map(|p| p.number).collect();
    assert_eq!(got, vec![5, 6]);
}

#[test]
fn locked_pr_is_left_to_the_other_machine() {
    let e = env(&one(toml("\"qa\"", "exit 0", "", false)), FakeLlm::new());
    e.forge.try_lock("qa-pr-1", "OTHERBOX").unwrap();
    assert!(!Qa::default().cycle(&e.ctx).unwrap());
    assert!(e.forge.labels(1).is_empty() && e.forge.comment_bodies(1).is_empty());
}

#[test]
fn recover_clears_only_own_claims() {
    let e = env(&[], FakeLlm::new());
    let forge = Arc::new(FakeForge::new("o/r").with_pr(pr(7, "Agent: a", "a", &["qa-running"])).with_pr(pr(8, "Agent: b", "b", &["qa-running"])));
    let ctx = Ctx { forge: { let f = forge.clone(); Arc::new(move |_| f.clone() as Arc<dyn Forge>) }, ..e.ctx.clone() };
    claims::add(&ctx.paths.state(), "o/r", 7);
    Qa::default().recover(&ctx);
    assert!(forge.labels(7).is_empty());
    assert_eq!(forge.labels(8), vec!["qa-running"], "another machine's claim stays");
    assert!(claims::load(&ctx.paths.state()).is_empty());
}
