//! Finding, claiming and releasing issues.
//!
//! A claimed issue has: no `agent-task` label, the `agent-running` label, the gh login as
//! assignee and a claim comment naming the machine. Release either re-queues it (label
//! back, for a retry by any machine) or hands it to a human (`needs-human`).

use crate::claim::{Claimed, Locks};
use crate::ctx::Ctx;
use crate::directives;
use crate::forge::Forge;
use crate::session::SessionState;
use regex::Regex;
use team_core::gh::Issue;
use team_core::git::RepoClone;

/// Free = open, activation label, not running / needs-human, nobody assigned.
pub fn is_free(ctx: &Ctx, i: &Issue) -> bool {
    let l = &ctx.cfg.labels;
    i.state == "open" && i.pull_request.is_none() && i.has_label(&l.task) && !i.has_label(&l.running) && !i.has_label(&l.needs_human) && i.assignees.is_empty()
}

/// Claims the oldest free issue (skipping ones another machine is claiming right now).
pub fn claim_next(ctx: &Ctx, forge: &dyn Forge, locks: &mut Locks, has_state: &dyn Fn(u64) -> bool) -> Result<Option<Issue>, String> {
    for candidate in forge.open_issues(&ctx.cfg.labels.task)? {
        if !is_free(ctx, &candidate) {
            continue;
        }
        let n = candidate.number;
        let claimed = locks.with(forge, &format!("issue-{n}"), || {
            let fresh = forge.issue(n)?;
            if !is_free(ctx, &fresh) {
                return Ok(Claimed::No);
            }
            mark_claimed(ctx, forge, &fresh, !has_state(n))?;
            Ok(Claimed::Yes(forge.issue(n).unwrap_or(fresh)))
        })?;
        if let Claimed::Yes(issue) = claimed {
            return Ok(Some(issue));
        }
    }
    Ok(None)
}

fn mark_claimed(ctx: &Ctx, forge: &dyn Forge, issue: &Issue, announce: bool) -> Result<(), String> {
    let l = &ctx.cfg.labels;
    let n = issue.number;
    forge.add_labels(n, &[&l.running])?;
    forge.remove_label(n, &l.task)?;
    if let Err(e) = forge.add_assignee(n, &ctx.login) {
        crate::log::warn(&format!("#{n}: Zuweisung an {} fehlgeschlagen: {e}", ctx.login));
    }
    let mark = format!("{CLAIM_MARK}{} -->", ctx.machine);
    let text = if announce {
        let note = directives::team_branch(issue.body.as_deref())
            .map(|tb| format!("\n\nWork will branch off **`{tb}`** and the PR will target it. If that branch doesn't exist yet, it is created automatically. (Wrong branch? Edit the issue's team-branch field, remove `{}` if set, and re-add the `{}` label.)", l.needs_human, l.task))
            .unwrap_or_default();
        format!("{mark}\n🤖 Agent `{}` is now working on this issue...{note}", ctx.machine)
    } else {
        format!("{mark}\n🤖 Agent `{}` is continuing work on this issue...", ctx.machine)
    };
    let _ = forge.comment(n, &text);
    Ok(())
}

const CLAIM_MARK: &str = "<!-- team-agent:claim machine=";

/// The machine named in the latest claim comment of an issue.
pub fn claimed_by(forge: &dyn Forge, n: u64) -> Option<String> {
    let comments = forge.comments(n).ok()?;
    let body = comments.iter().rev().map(|c| &c.body).find(|b| b.contains(CLAIM_MARK))?;
    let rest = &body[body.find(CLAIM_MARK)? + CLAIM_MARK.len()..];
    Some(rest[..rest.find(" -->")?].to_string())
}

/// How the claim ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Release {
    /// done (PR exists / closed) — just drop the running label and assignee
    Done,
    /// back into the queue (retry by any machine)
    Requeue,
    /// a human has to look (`needs-human`)
    Human,
}

/// Never fails (each step is best effort, logged).
pub fn release(ctx: &Ctx, forge: &dyn Forge, n: u64, how: Release) {
    let l = &ctx.cfg.labels;
    let warn = |what: &str, r: Result<(), String>| {
        if let Err(e) = r {
            crate::log::warn(&format!("#{n}: {what} fehlgeschlagen: {e}"));
        }
    };
    match how {
        Release::Requeue => warn("Label zurücksetzen", forge.add_labels(n, &[&l.task])),
        Release::Human => warn("needs-human setzen", forge.add_labels(n, &[&l.needs_human])),
        Release::Done => {}
    }
    warn("agent-running entfernen", forge.remove_label(n, &l.running));
    warn("Zuweisung lösen", forge.remove_assignees(n, std::slice::from_ref(&ctx.login)));
}

/// Branch for an issue, the same on every machine: a `branch:` directive, else the branch
/// of an open PR that references the issue, else the newest `agent/issue-N-*` on origin
/// (work another machine pushed), else a new one.
pub fn branch_for(forge: &dyn Forge, clone: &RepoClone, issue: &Issue, state: Option<&SessionState>) -> String {
    if let Some(s) = state.filter(|s| !s.branch_name.is_empty()) {
        return s.branch_name.clone();
    }
    if let Some(b) = directives::work_branch(issue.body.as_deref()) {
        return b;
    }
    let n = issue.number;
    let mention = Regex::new(&format!(r"#{n}\b")).expect("regex");
    if let Ok(prs) = forge.open_prs() {
        if let Some(pr) = prs.iter().find(|p| mention.is_match(&p.title) || mention.is_match(p.body.as_deref().unwrap_or(""))) {
            return pr.head.name.clone();
        }
    }
    let prefix = format!("agent/issue-{n}-");
    let pattern = format!("refs/heads/{prefix}*");
    if let Ok(out) = clone.git_net(&["ls-remote", "--heads", "origin", &pattern]) {
        let newest = out.lines().filter_map(|l| l.split("refs/heads/").nth(1)).filter(|b| b[prefix.len()..].chars().all(|c| c.is_ascii_digit())).max_by_key(|b| b[prefix.len()..].parse::<u64>().unwrap_or(0));
        if let Some(b) = newest {
            return b.to_string();
        }
    }
    format!("{prefix}{}", chrono::Utc::now().timestamp())
}
