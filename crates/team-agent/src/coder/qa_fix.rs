//! The coder repairs agent PRs that QA marked `qa-failed`, before taking new issues.
//!
//! Claim: the PR gets `agent-running` (under the cross-machine lock). Escalation to a human
//! after more QA failures than `max_qa_fix_rounds` fix attempts, or after a fix attempt
//! crashed twice (the Python agent retried a crashing fix forever).

use super::finish;
use super::work::label;
use crate::claim::{Claimed, Locks};
use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::directives;
use crate::forge::{label_names, Forge, PR_TITLE_PREFIX, QA_COMMENT_PREFIX, QA_FAILED_PREFIX};
use crate::llm::{Job, LlmError};
use crate::pr_text::linked_issue;
use crate::walkthrough;
use crate::workspace::RepoSetup;
use std::collections::HashMap;
use team_core::detail;
use team_core::gh::{Comment, PullRequest};
use team_core::git::{commit_and_push, run_git};

const ERROR_MARK: &str = "<!-- team-agent:qa-fix-error -->";

fn wanted(ctx: &Ctx, pr: &PullRequest) -> bool {
    let l = &ctx.cfg.labels;
    pr.title.starts_with(PR_TITLE_PREFIX) && pr.has_label(&l.qa_failed) && !pr.has_label(&l.running) && !pr.has_label(&l.needs_human)
}

/// Handles the oldest `qa-failed` agent PR. `Ok(true)` = one was handled (or claimed by
/// someone else right now), so no new issue is started this cycle.
pub fn run(ctx: &Ctx, setup: &RepoSetup, locks: &mut Locks) -> RoleResult<bool> {
    let forge = setup.forge.as_ref();
    let Some(candidate) = forge.open_prs()?.into_iter().find(|p| wanted(ctx, p)) else { return Ok(false) };
    let n = candidate.number;
    let claimed = locks.with(forge, &format!("qafix-pr-{n}"), || {
        let pr = forge.pr(n)?;
        if !wanted(ctx, &pr) {
            return Ok(Claimed::No);
        }
        forge.add_labels(n, &[&ctx.cfg.labels.running])?;
        Ok(Claimed::Yes(pr))
    })?;
    let Claimed::Yes(pr) = claimed else { return Ok(true) };
    ctx.heartbeat.report("working", detail!["repo" => setup.repo, "pr" => n, "title" => pr.title]);
    let comments = forge.comments(n)?;
    let failures = comments.iter().filter(|c| c.body.starts_with(QA_FAILED_PREFIX)).count() as u32;
    let max = ctx.cfg.budgets.max_qa_fix_rounds;
    if failures > max {
        escalate(ctx, forge, n, &format!("Reached max QA-fix rounds ({max}). Escalating to human for review."));
        return Ok(true);
    }
    match fix(ctx, setup, &pr, &comments) {
        Ok(()) => {
            let _ = forge.remove_label(n, &ctx.cfg.labels.qa_failed);
            let _ = forge.remove_label(n, &ctx.cfg.labels.running);
            Ok(true)
        }
        Err(RoleError::UsageLimit(t)) => {
            let _ = forge.remove_label(n, &ctx.cfg.labels.running);
            Err(RoleError::UsageLimit(t))
        }
        Err(RoleError::Failed(e)) => {
            crate::log::error(&format!("QA-Fix PR #{n}: {e}"));
            let short: String = e.chars().take(300).collect();
            if errors_since_last_verdict(&comments) >= 1 {
                escalate(ctx, forge, n, &format!("{ERROR_MARK}\nThe QA fix failed again (`{short}`). Escalating to human for review."));
            } else {
                let _ = forge.comment(n, &format!("{ERROR_MARK}\n⚠️ QA fix attempt on `{}` failed: `{short}` — will retry once.", ctx.machine));
                let _ = forge.remove_label(n, &ctx.cfg.labels.running);
            }
            Ok(true)
        }
    }
}

fn errors_since_last_verdict(comments: &[Comment]) -> usize {
    let start = comments.iter().rposition(|c| c.body.starts_with(QA_COMMENT_PREFIX)).map(|i| i + 1).unwrap_or(0);
    comments[start..].iter().filter(|c| c.body.contains(ERROR_MARK)).count()
}

fn escalate(ctx: &Ctx, forge: &dyn Forge, n: u64, text: &str) {
    let l = &ctx.cfg.labels;
    let _ = forge.add_labels(n, &[&l.needs_human]);
    let _ = forge.comment(n, text);
    let _ = forge.remove_label(n, &l.qa_failed);
    let _ = forge.remove_label(n, &l.running);
}

fn fix(ctx: &Ctx, setup: &RepoSetup, pr: &PullRequest, comments: &[Comment]) -> RoleResult<()> {
    let forge = setup.forge.as_ref();
    let branch = pr.head.name.clone();
    let issue = pr.body.as_deref().and_then(linked_issue).and_then(|i| forge.issue(i).ok());
    let team = issue.as_ref().and_then(|i| directives::team_branch(i.body.as_deref()));
    setup.clone.fetch()?;
    let wt = setup.clone.worktree(&ctx.paths.worktree(&setup.repo, &branch), &branch, team.as_deref().unwrap_or(&setup.working_branch))?;
    let g = |args: &[&str]| run_git(Some(&wt), args, false);
    run_git(Some(&wt), &["fetch", "origin", &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")], true)?;
    g(&["checkout", &branch])?;
    g(&["reset", "--hard", &format!("origin/{branch}")])?;
    g(&["clean", "-fdx"])?;

    let verdict = comments.iter().rev().find(|c| c.body.starts_with(QA_COMMENT_PREFIX)).map(|c| c.body.replace('\r', "")).unwrap_or_default();
    let (summary, details) = verdict.split_once('\n').map(|(a, b)| (a.trim().to_string(), b.trim().to_string())).unwrap_or((verdict.trim().to_string(), String::new()));
    let mut v: HashMap<&str, String> = HashMap::new();
    ctx.tools.insert(&mut v);
    v.insert("issue_number", issue.as_ref().map(|i| i.number.to_string()).unwrap_or_else(|| "?".into()));
    v.insert("issue_title", issue.as_ref().map(|i| i.title.clone()).unwrap_or_else(|| pr.title.clone()));
    v.insert("issue_body", issue.as_ref().and_then(|i| i.body.clone()).filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "No description provided.".into()));
    v.insert("pr_number", pr.number.to_string());
    v.insert("branch", branch.clone());
    v.insert("qa_summary", if summary.is_empty() { "(no summary provided)".into() } else { summary });
    v.insert("qa_details", if details.is_empty() { "(no detail block provided)".into() } else { details });

    let mut labels = label_names(&pr.labels);
    if let Some(i) = &issue {
        labels.extend(label_names(&i.labels));
    }
    let complex = labels.iter().any(|l| l.eq_ignore_ascii_case(&ctx.cfg.labels.complex));
    let choice = team_core::provider::choose(&ctx.cfg, &setup.repo, &labels)?;
    let b = &ctx.cfg.budgets;
    let job = Job { prompt: ctx.render("qa-fix", &v)?, cwd: wt.clone(), model: choice.model, max_turns: if complex { b.max_turns_complex } else { b.max_turns_regular }, env: choice.env, label: label(&setup.repo, "qafix", pr.number), max_duration: None };
    crate::log::info(&format!("QA-Fix für PR #{} ({})", pr.number, choice.why));
    ctx.llm.run(&job).map_err(|e| match e {
        LlmError::UsageLimit(t) => RoleError::UsageLimit(t),
        LlmError::Failed(m) => RoleError::Failed(m),
    })?;
    commit_and_push(&wt, &branch, &format!("QA fix: address verdict on PR #{}", pr.number))?;
    if let Some(i) = &issue {
        let md = walkthrough::publish(&wt, &setup.repo, &branch, i.number);
        if let Ok(fresh) = forge.pr(pr.number) {
            finish::attach(forge, &fresh, &md);
        }
    }
    Ok(())
}
