//! After coding: push, PR (create or reuse), review rounds, close the issue, history.

use super::pickup::{self, Release};
use super::review_loop;
use crate::ctx::{Ctx, RoleResult};
use crate::forge::{Forge, PR_TITLE_PREFIX};
use crate::pr_text::{self, Stats};
use crate::session::{SessionState, Store};
use crate::qa::claims;
use crate::walkthrough;
use crate::workspace::RepoSetup;
use std::path::Path;
use team_core::gh::{Issue, PullRequest};
use team_core::git::{commit_and_push, run_git};
use team_core::state::{append_history, HistoryRecord};

pub fn finish(ctx: &Ctx, setup: &RepoSetup, store: &Store, issue: &Issue, state: &mut SessionState, wt: &Path, base: &str) -> RoleResult<()> {
    let forge = setup.forge.as_ref();
    let n = issue.number;
    let branch = state.branch_name.clone();
    if !state.in_review {
        commit_and_push(wt, &branch, &format!("Agent: implement #{n} — {}", issue.title))?;
        if commits_ahead(wt, base)? == 0 {
            no_changes(ctx, setup, store, issue, state);
            return Ok(());
        }
    }
    let pr = find_or_create_pr(ctx, setup, issue, state, wt, base)?;
    state.in_review = true;
    state.pr_url = Some(pr.html_url.clone());
    store.save(state);
    // QA and QA fix keep away while the coder still changes the head
    claims::add_to(&ctx.paths.state(), claims::CODER_FILE, &setup.repo, pr.number);
    let _ = forge.add_labels(pr.number, &[&ctx.cfg.labels.running]);
    let escalated = review_loop::run(ctx, setup, issue, state, wt, &pr)?;
    let _ = forge.remove_label(pr.number, &ctx.cfg.labels.running);
    claims::remove_from(&ctx.paths.state(), claims::CODER_FILE, &setup.repo, pr.number);
    if escalated {
        pickup::release(ctx, forge, n, Release::Human);
    } else {
        // the PR is done; a failing comment/close must not send the issue back to coding
        let closed = forge.comment(n, &format!("Implementation complete. PR: {}", pr.html_url)).and_then(|_| forge.close_issue(n));
        if let Err(e) = closed {
            crate::log::warn(&format!("#{n} nicht geschlossen ({e}) — der PR ist trotzdem fertig"));
        }
        pickup::release(ctx, forge, n, Release::Done);
    }
    remove_worktree(setup, wt);
    state.completed = true;
    state.note(&format!("PR created: {}", pr.html_url));
    history(ctx, issue, state, true, Some(pr.html_url.clone()));
    store.delete(&setup.repo, n);
    crate::log::info(&format!("{}#{n} erledigt: {}{}", setup.repo, pr.html_url, if escalated { " (an Menschen übergeben)" } else { "" }));
    Ok(())
}

fn commits_ahead(wt: &Path, base: &str) -> Result<u32, String> {
    let out = run_git(Some(wt), &["rev-list", "--count", &format!("origin/{base}..HEAD")], false)?;
    out.trim().parse().map_err(|_| format!("rev-list: {out}"))
}

/// Nothing to change — the issue was most likely solved already.
fn no_changes(ctx: &Ctx, setup: &RepoSetup, store: &Store, issue: &Issue, state: &mut SessionState) {
    let forge = setup.forge.as_ref();
    let n = issue.number;
    let body = format!(
        "[OK] This issue appears to be already resolved.\n\nThe agent attempted to implement this but found no code changes were necessary, which typically means the fix was already merged in a previous PR.\n\n**Agent Stats:**\n- Sessions: {}\n- Total tokens: {}\n- Cost: ${:.4} USD\n\nClosing as already resolved.",
        state.session_count,
        pr_text::thousands(state.total_tokens),
        state.total_cost_usd
    );
    let _ = forge.comment(n, &body);
    if let Err(e) = forge.close_issue(n) {
        crate::log::warn(&format!("#{n} nicht geschlossen: {e}"));
    }
    pickup::release(ctx, forge, n, Release::Done);
    remove_worktree(setup, &ctx.paths.worktree(&setup.repo, &state.branch_name));
    state.completed = true;
    state.note("No changes needed — closed");
    history(ctx, issue, state, true, None);
    store.delete(&setup.repo, n);
}

fn find_or_create_pr(ctx: &Ctx, setup: &RepoSetup, issue: &Issue, state: &SessionState, wt: &Path, base: &str) -> Result<PullRequest, String> {
    let forge = setup.forge.as_ref();
    let branch = &state.branch_name;
    let wt_md = walkthrough::publish(wt, &setup.repo, branch, issue.number);
    if let Some(pr) = forge.pr_by_branch(branch)? {
        attach(forge, &pr, &wt_md);
        return Ok(pr);
    }
    let stats = Stats { sessions: state.session_count, turns: state.total_turns_used, tokens: state.total_tokens, cost: state.total_cost_usd, tools: vec![] };
    let body = pr_text::pr_body(issue.number, &pr_text::pr_summary(&state.last_output), &pr_text::stats_block(&stats), &wt_md);
    let title = format!("{PR_TITLE_PREFIX} {}", issue.title);
    let pr = match forge.create_pr(&title, &body, branch, base) {
        Ok(pr) => pr,
        Err(e) if e.contains("already exists") => forge.pr_by_branch(branch)?.ok_or(e)?,
        Err(e) if e.to_lowercase().contains("base") && e.to_lowercase().contains("invalid") && base != setup.default_branch => forge.create_pr(&title, &body, branch, &setup.default_branch)?,
        Err(e) => return Err(e),
    };
    if !ctx.cfg.labels.pr.is_empty() {
        if let Err(e) = forge.add_labels(pr.number, &[&ctx.cfg.labels.pr]) {
            crate::log::warn(&format!("PR #{}: Label {} nicht gesetzt: {e}", pr.number, ctx.cfg.labels.pr));
        }
    }
    Ok(pr)
}

/// The branch lives on origin; the local checkout (with its build output) can go.
pub fn remove_worktree(setup: &RepoSetup, wt: &Path) {
    if let Err(e) = setup.clone.remove_worktree(wt) {
        crate::log::warn(&format!("Worktree {} nicht entfernt: {e}", wt.display()));
    }
}

/// Puts a (new) walkthrough into an existing PR's body.
pub fn attach(forge: &dyn Forge, pr: &PullRequest, wt_md: &str) {
    if wt_md.is_empty() {
        return;
    }
    let body = walkthrough::merge_into_body(pr.body.as_deref().unwrap_or(""), wt_md);
    if let Err(e) = forge.edit_pr_body(pr.number, &body) {
        crate::log::warn(&format!("PR #{}: Walkthrough nicht eingefügt: {e}", pr.number));
    }
}

pub fn history(ctx: &Ctx, issue: &Issue, state: &SessionState, completed: bool, pr_url: Option<String>) {
    let record = HistoryRecord {
        number: issue.number,
        title: issue.title.clone(),
        repository: state.repo.clone(),
        completed,
        pr_url,
        total_tokens: state.total_tokens,
        total_cost_usd: state.total_cost_usd,
        session_count: state.session_count,
        timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        duration_sec: state.duration_sec(),
        machine: Some(ctx.machine.clone()),
    };
    if let Err(e) = append_history(&ctx.paths.state(), &record) {
        crate::log::warn(&format!("History nicht geschrieben: {e}"));
    }
}
