//! Handling one trigger comment: decide under the PR lock (decline, give up or claim), then
//! run the worker on the PR branch, push, refresh the walkthrough and reply.

use super::thread::{self, MAX_ATTEMPTS_PER_COMMENT};
use super::{fresh_thread, safe_comment, PrFeedback, ROLE};
use crate::agent_toml::{self, ProjectConfig};
use crate::claim::Claimed;
use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::forge::{label_names, Forge};
use crate::llm::Job;
use crate::workspace::{self, RepoSetup};
use crate::{pr_text, walkthrough};
use chrono::Utc;
use std::collections::HashMap;
use team_core::gh::{Comment, PullRequest};
use team_core::git::{commit_and_push, run_git};
use team_core::provider::{self, Choice};

/// What the locked decision step concluded.
enum Decision {
    /// a final reply was posted (fork, disabled, give-up)
    Finished,
    /// claimed; this is attempt number n
    Work(usize),
}

/// Handles `trigger` on `pr`. `Ok(false)` = repo skipped (provider not usable).
pub(super) fn run(role: &mut PrFeedback, ctx: &Ctx, forge: &dyn Forge, repo: &str, pr: &PullRequest, trigger: &Comment) -> RoleResult<bool> {
    let issue_number = pr_text::linked_issue(pr.body.as_deref().unwrap_or("")).unwrap_or(pr.number);
    let choice = match provider::choose(&ctx.cfg, repo, &labels(forge, pr, issue_number)) {
        Ok(c) => c,
        Err(e) => {
            crate::log::error(&format!("{repo}: {e} — Repo in diesem Durchlauf übersprungen"));
            return Ok(false);
        }
    };
    ctx.heartbeat.report("working", team_core::detail!["repo" => repo, "pr" => pr.number, "title" => pr.title]);
    let setup = workspace::setup(ctx, ROLE, repo)?;
    let policy = load_policy(&setup);
    let id = trigger.id;
    let decision = role.locks.with(forge, &format!("fb-pr-{}", pr.number), &ctx.machine, || {
        let (_, th) = fresh_thread(forge, pr.number)?;
        if th.is_done(id) || th.live_claim(id, Utc::now()) {
            return Ok(Claimed::No);
        }
        if let Some(fork) = fork_name(pr, repo) {
            safe_comment(forge, pr.number, &thread::fork_comment(id, &fork));
            return Ok(Claimed::Yes(Decision::Finished));
        }
        match &policy {
            Err(e) => safe_comment(forge, pr.number, &thread::disabled_comment(id, Some(e))),
            Ok(p) if !p.enabled(ROLE) => safe_comment(forge, pr.number, &thread::disabled_comment(id, None)),
            Ok(_) => {
                let attempts = th.attempts(id);
                if attempts >= MAX_ATTEMPTS_PER_COMMENT {
                    safe_comment(forge, pr.number, &thread::give_up_comment(id, attempts, "the previous attempt never finished (machine stopped or timed out)"));
                } else {
                    forge.comment(pr.number, &thread::claim_comment(id, &ctx.machine))?;
                    return Ok(Claimed::Yes(Decision::Work(attempts + 1)));
                }
            }
        }
        Ok(Claimed::Yes(Decision::Finished))
    })?;
    let attempt = match decision {
        Claimed::No => return Ok(false),
        Claimed::Yes(Decision::Finished) => return Ok(true),
        Claimed::Yes(Decision::Work(n)) => n,
    };
    crate::log::info(&format!("{repo}#{}: Feedback-Kommentar {id}, Versuch {attempt}", pr.number));
    match work(ctx, &setup, forge, pr, trigger, &choice, issue_number) {
        Ok(reply) => safe_comment(forge, pr.number, &reply),
        Err(RoleError::UsageLimit(t)) => return Err(RoleError::UsageLimit(t)),
        Err(RoleError::Failed(e)) => {
            crate::log::error(&format!("{repo}#{}: {e}", pr.number));
            let text = if attempt >= MAX_ATTEMPTS_PER_COMMENT { thread::give_up_comment(id, attempt, &e) } else { thread::failed_comment(id, attempt, &e) };
            safe_comment(forge, pr.number, &text);
        }
    }
    Ok(true)
}

/// Lower-cased labels of the PR and of its linked issue (if it is a different number).
fn labels(forge: &dyn Forge, pr: &PullRequest, issue_number: u64) -> Vec<String> {
    let mut out = label_names(&pr.labels);
    if issue_number != pr.number {
        match forge.issue(issue_number) {
            Ok(i) => out.extend(label_names(&i.labels)),
            Err(e) => crate::log::warn(&format!("Verknüpftes Issue #{issue_number} nicht lesbar: {e}")),
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Head repository when it is not this one (`None` from GitHub = the fork was deleted).
fn fork_name(pr: &PullRequest, repo: &str) -> Option<String> {
    match &pr.head.repo {
        None => Some("(deleted fork)".into()),
        Some(r) if !r.full_name.eq_ignore_ascii_case(repo) => Some(r.full_name.clone()),
        Some(_) => None,
    }
}

/// `.agent.toml` of the default branch (old PR branches predate the opt-in). Missing file →
/// defaults; broken file → the parse error.
fn load_policy(setup: &RepoSetup) -> Result<ProjectConfig, String> {
    let spec = format!("origin/{}:{}", setup.default_branch, agent_toml::FILE);
    match run_git(Some(&setup.clone.dir), &["show", &spec], false) {
        Ok(text) => ProjectConfig::parse(&text),
        Err(_) => Ok(ProjectConfig::default()),
    }
}

fn work(ctx: &Ctx, setup: &RepoSetup, forge: &dyn Forge, pr: &PullRequest, trigger: &Comment, choice: &Choice, issue_number: u64) -> RoleResult<String> {
    let branch = &pr.head.name;
    setup.clone.reset_to_remote(branch)?;
    let before = setup.clone.git(&["rev-parse", "HEAD"])?;
    let mut values: HashMap<&str, String> = HashMap::new();
    values.insert("pr_number", pr.number.to_string());
    values.insert("pr_title", pr.title.clone());
    values.insert("branch", branch.clone());
    values.insert("comment_body", if trigger.body.is_empty() { "(empty comment)".into() } else { trigger.body.clone() });
    values.insert("issue_number", issue_number.to_string());
    ctx.tools.insert(&mut values);
    let job = Job {
        prompt: ctx.render("pr-feedback", &values)?,
        cwd: setup.clone.dir.clone(),
        model: choice.model.clone(),
        max_turns: ctx.cfg.budgets.pr_feedback_max_turns,
        env: choice.env.clone(),
        label: format!("fb_{}_pr-{}", setup.repo.replace('/', "_"), pr.number),
        max_duration: None,
    };
    let out = ctx.llm.run(&job)?;
    if out.reached_max_turns {
        crate::log::warn(&format!("PR #{}: Worker hat das Turn-Limit erreicht", pr.number));
    }
    commit_and_push(&setup.clone.dir, branch, &format!("PR feedback: address comment on #{}", pr.number))?;
    // the worker may have committed (and pushed) by itself — compare with the start
    let pushed = setup.clone.git(&["rev-parse", "HEAD"])? != before;
    if pushed {
        // the QA verdict was for the old head — let QA look again
        for label in [&ctx.cfg.labels.qa_passed, &ctx.cfg.labels.qa_failed] {
            let _ = forge.remove_label(pr.number, label);
        }
    }
    let wt = walkthrough::publish(&setup.clone.dir, &setup.repo, branch, issue_number);
    if !wt.is_empty() {
        let body = forge.pr(pr.number).ok().and_then(|p| p.body).or_else(|| pr.body.clone()).unwrap_or_default();
        if let Err(e) = forge.edit_pr_body(pr.number, &walkthrough::merge_into_body(&body, &wt)) {
            crate::log::warn(&format!("PR #{}: Walkthrough nicht in die Beschreibung übernommen: {e}", pr.number));
        }
    }
    Ok(thread::success_comment(trigger.id, &thread::feedback_report(&out.text), pushed, &wt))
}
