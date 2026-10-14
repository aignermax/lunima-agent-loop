//! PR-feedback role: a comment containing the marker (default `@agent`) on any open PR asks
//! the agent to implement the request on the PR's branch. One request per cycle; progress
//! lives in the PR thread itself (see `thread`), so several machines can share the work.

mod handle;
pub mod thread;

use crate::claim::{Claimed, Locks};
use crate::ctx::{Ctx, RoleResult};
use crate::forge::Forge;
use crate::repos::{self, RoundRobin};
use crate::role::Role;
use chrono::Utc;
use std::collections::HashMap;
use team_core::gh::{Comment, PullRequest};
use thread::{cap_comment, pending, Thread};

pub const ROLE: &str = "pr-feedback";

/// The PR-feedback team member.
#[derive(Default)]
pub struct PrFeedback {
    rr: RoundRobin,
    locks: Locks,
    /// `owner/repo#n` → `updated_at` of a PR that had nothing left to do
    last_update: HashMap<String, String>,
}

impl PrFeedback {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Role for PrFeedback {
    fn name(&self) -> &'static str {
        ROLE
    }

    fn cycle(&mut self, ctx: &Ctx) -> RoleResult<bool> {
        let list = repos::effective(&ctx.cfg, &ctx.paths.state(), false);
        let Some(repo) = self.rr.next(&list) else {
            crate::log::warn("Keine Repos konfiguriert oder entdeckt");
            return Ok(false);
        };
        let forge = (ctx.forge)(&repo);
        let prs = match forge.open_prs() {
            Ok(p) => p,
            Err(e) if ctx.cfg.repo(&repo).is_none() => {
                crate::log::warn(&format!("{repo} nicht erreichbar ({e}) — aus der Entdeckung entfernt"));
                repos::forget(&ctx.paths.state(), &repo);
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        for pr in prs {
            if let Some(done) = self.visit(ctx, forge.as_ref(), &repo, &pr)? {
                return Ok(done);
            }
        }
        Ok(false)
    }
}

impl PrFeedback {
    /// `Some(result)` ends the cycle (a request was handled, or the repo must be skipped).
    fn visit(&mut self, ctx: &Ctx, forge: &dyn Forge, repo: &str, pr: &PullRequest) -> RoleResult<Option<bool>> {
        let key = format!("{repo}#{}", pr.number);
        if !pr.updated_at.is_empty() && self.last_update.get(&key) == Some(&pr.updated_at) {
            return Ok(None);
        }
        let comments = match forge.comments(pr.number) {
            Ok(c) => c,
            Err(e) => {
                crate::log::warn(&format!("Kommentare von {key} nicht lesbar: {e}"));
                return Ok(None);
            }
        };
        let marker = &ctx.cfg.pr_feedback_marker;
        let thread = Thread::parse(&comments);
        let open = pending(&comments, &thread, marker);
        if open.is_empty() {
            self.last_update.insert(key, pr.updated_at.clone());
            return Ok(None);
        }
        let max_rounds = ctx.cfg.budgets.pr_feedback_max_rounds;
        if thread.rounds() >= max_rounds as usize || thread.cap_notified {
            if !thread.cap_notified {
                self.notify_cap(forge, pr.number, max_rounds, &ctx.machine)?;
            }
            return Ok(None);
        }
        let now = Utc::now();
        let Some(trigger) = open.into_iter().find(|c| !thread.live_claim(c.id, now)).cloned() else {
            return Ok(None); // another machine is on it
        };
        handle::run(self, ctx, forge, repo, pr, &trigger).map(Some)
    }

    fn notify_cap(&mut self, forge: &dyn Forge, number: u64, max_rounds: u32, machine: &str) -> RoleResult {
        self.locks
            .with(forge, &format!("fb-pr-{number}"), machine, || {
                let thread = Thread::parse(&forge.comments(number)?);
                if thread.cap_notified {
                    return Ok(Claimed::No);
                }
                safe_comment(forge, number, &cap_comment(max_rounds));
                Ok(Claimed::Yes(()))
            })
            .map(|_| ())
            .map_err(Into::into)
    }
}

/// Best-effort comment (a failed post only costs the reply, the state is still consistent).
pub(crate) fn safe_comment(forge: &dyn Forge, number: u64, body: &str) {
    if let Err(e) = forge.comment(number, body) {
        crate::log::warn(&format!("Kommentar an #{number} fehlgeschlagen: {e}"));
    }
}

/// Re-reads the thread for a decision inside a lock.
pub(crate) fn fresh_thread(forge: &dyn Forge, number: u64) -> Result<(Vec<Comment>, Thread), String> {
    let comments = forge.comments(number)?;
    let thread = Thread::parse(&comments);
    Ok((comments, thread))
}

#[cfg(test)]
mod tests;
