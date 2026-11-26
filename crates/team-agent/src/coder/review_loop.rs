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
                let _ = forge.comment(pr.number, &review::review_comment(&v, &reviewer_model(ctx, &setup.repo, issue)));
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

fn reviewer_model(ctx: &Ctx, repo: &str, issue: &Issue) -> String {
    if ctx.cfg.repo(repo).is_some_and(|r| r.profile.is_some()) {
        return team_core::project::resolve(&ctx.cfg, repo, team_core::project::SessionRole::Reviewer, &crate::forge::label_names(&issue.labels))
            .map(|d| d.choice.model).unwrap_or_else(|_| "nicht verfügbar (Projektprofil)".into());
    }
    if issue.has_label(&ctx.cfg.labels.critical) { ctx.cfg.models.reviewer_critical.clone() } else { ctx.cfg.models.reviewer.clone() }
}

/// The reviewer always runs on the subscription; a crash counts as BLOCKING (fail-safe).
fn reviewer(ctx: &Ctx, repo: &str, issue: &Issue, pr: &PullRequest, wt: &Path) -> RoleResult<Verdict> {
    let text = prompt::reviewer(ctx, issue, pr.number, &pr.head.name, &pr.base.name)?;
    let job = Job { policy: crate::llm::PolicyRequest { role: team_core::project::SessionRole::Reviewer, labels: crate::forge::label_names(&issue.labels) }, repo: repo.into(), prompt: text, cwd: wt.to_path_buf(), model: reviewer_model(ctx, repo, issue), max_turns: ctx.cfg.budgets.reviewer_max_turns, env: ChildEnv::subscription(), label: label(repo, "review", pr.number), max_duration: None, tools: None };
    match ctx.llm.run(&job) {
        Ok(out) => Ok(review::parse(&out.text)),
        Err(LlmError::Unavailable(e)) => Err(RoleError::Unavailable(e)),
        Err(LlmError::UsageLimit(backend, t)) => Err(RoleError::UsageLimit(backend, t)),
        Err(LlmError::Failed(e)) => Ok(Verdict::blocking(&format!("Review could not run: {}", e.chars().take(300).collect::<String>()))),
    }
}

#[allow(clippy::too_many_arguments)]
fn retry(ctx: &Ctx, setup: &RepoSetup, issue: &Issue, state: &mut SessionState, wt: &Path, pr: &PullRequest, blocking: &Verdict, round: u32) -> RoleResult<()> {
    let labels = crate::forge::label_names(&issue.labels);
    let choice = team_core::provider::choose(&ctx.cfg, &setup.repo, &labels).map_err(|e| {
        if ctx.cfg.repo(&setup.repo).is_some_and(|r| r.profile.is_some()) { RoleError::Unavailable(e) } else { RoleError::Failed(e) }
    })?;
    let b = &ctx.cfg.budgets;
    let max_turns = if prompt::is_complex(ctx, issue) { b.max_turns_complex } else { b.max_turns_regular };
    let job = Job { policy: crate::llm::PolicyRequest { role: team_core::project::SessionRole::Coder, labels: labels.clone() }, repo: setup.repo.clone(), prompt: prompt::retry(ctx, issue, &state.branch_name, blocking)?, cwd: wt.to_path_buf(), model: choice.model, max_turns, env: choice.env, label: label(&setup.repo, "retry", issue.number), max_duration: None, tools: None };
    crate::log::info(&format!("#{}: Nachbesserung nach Review (Runde {round})", issue.number));
    let out = ctx.llm.run(&job)?;
    state.add_session(out.num_turns, out.usage.total(), out.estimated_cost());
    state.count_tools(&out.bash_commands);
    commit_and_push(wt, &state.branch_name, &format!("Address review feedback (round {})", round + 1))?;
    for label in [&ctx.cfg.labels.qa_passed, &ctx.cfg.labels.qa_failed] {
        let _ = setup.forge.remove_label(pr.number, label);
    }
    let md = walkthrough::publish(wt, &setup.repo, &state.branch_name, issue.number);
    if let Ok(fresh) = setup.forge.pr(pr.number) {
        finish::attach(setup.forge.as_ref(), &fresh, &md);
    }
    Ok(())
}
