//! Review rounds on the coder's PR: test gate (`test_cmd` of .agent.toml), then the LLM
//! reviewer; BLOCKING → the coder retries; still blocking after the last round → human.

use super::finish;
use super::prompt;
use super::work::label;
use crate::agent_toml::ProjectConfig;
use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::llm::{Job, LlmError};
use crate::review::{self, Verdict};
use crate::session::SessionState;
use crate::walkthrough;
use crate::workspace::RepoSetup;
use std::path::Path;
use team_core::gh::{Issue, PullRequest};
use team_core::git::commit_and_push;
use team_core::provider::ChildEnv;

/// `Ok(true)` = escalated to a human.
pub fn run(ctx: &Ctx, setup: &RepoSetup, issue: &Issue, state: &mut SessionState, wt: &Path, pr: &PullRequest) -> RoleResult<bool> {
    let forge = setup.forge.as_ref();
    let complex = prompt::is_complex(ctx, issue);
    let b = &ctx.cfg.budgets;
    let rounds = if complex { b.max_review_rounds_complex } else { b.max_review_rounds_regular }.max(1);
    for round in 1..=rounds {
        let gate = if b.test_gate { ProjectConfig::load(wt).ok().and_then(|p| review::test_gate(wt, &p)) } else { None };
        let blocking = match gate {
            Some(g) if g.is_blocking() => {
                if round < rounds {
                    let _ = forge.comment(pr.number, &format!("## Automated Test Gate — **BLOCKING**\n\n{}", g.summary));
                }
                g
            }
            _ => {
                let v = reviewer(ctx, &setup.repo, issue, pr, wt)?;
                let _ = forge.comment(pr.number, &review::review_comment(&v, &reviewer_model(ctx, issue)));
                if !v.is_blocking() {
                    return Ok(false);
                }
                v
            }
        };
        if round == rounds {
            let _ = forge.comment(pr.number, &format!("Reached max review rounds ({rounds}) with an unresolved BLOCKING result (test gate or reviewer). Last summary: {}", blocking.summary));
            return Ok(true);
        }
        retry(ctx, setup, issue, state, wt, pr, &blocking, round)?;
    }
    Ok(true)
}

fn reviewer_model(ctx: &Ctx, issue: &Issue) -> String {
    if issue.has_label(&ctx.cfg.labels.critical) { ctx.cfg.models.reviewer_critical.clone() } else { ctx.cfg.models.reviewer.clone() }
}

/// The reviewer always runs on the subscription; a crash counts as BLOCKING (fail-safe).
fn reviewer(ctx: &Ctx, repo: &str, issue: &Issue, pr: &PullRequest, wt: &Path) -> RoleResult<Verdict> {
    let text = prompt::reviewer(ctx, issue, pr.number, &pr.head.name, &pr.base.name)?;
    let job = Job { prompt: text, cwd: wt.to_path_buf(), model: reviewer_model(ctx, issue), max_turns: ctx.cfg.budgets.reviewer_max_turns, env: ChildEnv::subscription(), label: label(repo, "review", pr.number) };
    match ctx.llm.run(&job) {
        Ok(out) => Ok(review::parse(&out.text)),
        Err(LlmError::UsageLimit(t)) => Err(RoleError::UsageLimit(t)),
        Err(LlmError::Failed(e)) => Ok(Verdict::blocking(&format!("Review could not run: {}", e.chars().take(300).collect::<String>()))),
    }
}

#[allow(clippy::too_many_arguments)]
fn retry(ctx: &Ctx, setup: &RepoSetup, issue: &Issue, state: &mut SessionState, wt: &Path, pr: &PullRequest, blocking: &Verdict, round: u32) -> RoleResult<()> {
    let labels = crate::forge::label_names(&issue.labels);
    let choice = team_core::provider::choose(&ctx.cfg, &setup.repo, &labels)?;
    let b = &ctx.cfg.budgets;
    let max_turns = if prompt::is_complex(ctx, issue) { b.max_turns_complex } else { b.max_turns_regular };
    let job = Job { prompt: prompt::retry(ctx, issue, &state.branch_name, blocking)?, cwd: wt.to_path_buf(), model: choice.model, max_turns, env: choice.env, label: label(&setup.repo, "retry", issue.number) };
    crate::log::info(&format!("#{}: Nachbesserung nach Review (Runde {round})", issue.number));
    let out = ctx.llm.run(&job)?;
    state.add_session(out.num_turns, out.usage.total(), out.estimated_cost());
    commit_and_push(wt, &state.branch_name, &format!("Address review feedback (round {})", round + 1))?;
    let md = walkthrough::publish(wt, &setup.repo, &state.branch_name, issue.number);
    if let Ok(fresh) = setup.forge.pr(pr.number) {
        finish::attach(setup.forge.as_ref(), &fresh, &md);
    }
    Ok(())
}
