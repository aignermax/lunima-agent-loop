//! QA role: verifies agent PRs (`Agent:` title) by running the repo's build/test commands
//! from `.agent.toml` on the PR branch, optionally plus a read-only Claude review, and
//! labels the PR `qa-passed` / `qa-failed` with a `[qa-agent]` verdict comment.
//!
//! One PR per cycle. Claims run under a GitHub-side lock (`claim::Locks`) so two machines
//! never verify the same PR; `qa-running` marks the claim for everyone.

pub mod claims;
pub mod steps;
pub mod verdict;

#[cfg(test)]
mod tests;

use crate::agent_toml::ProjectConfig;
use crate::claim::{Claimed, Locks};
use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::forge::{Forge, PR_TITLE_PREFIX};
use crate::repos::{self, RoundRobin};
use crate::role::Role;
use crate::workspace::{self, RepoSetup};
use std::collections::HashSet;
use steps::StepResult;
use team_core::config::Labels;
use team_core::gh::PullRequest;

pub const NO_COMMANDS: &str = "_No build/test commands configured in `.agent.toml` — nothing was verified._";

/// The QA team member.
#[derive(Default)]
pub struct Qa {
    rr: RoundRobin,
    locks: Locks,
    /// (repo, PR, head sha) of PRs whose `.agent.toml` disables QA — not reselected until
    /// the branch changes (the Python agent re-checked the oldest such PR forever and
    /// starved every newer one)
    disabled: HashSet<(String, u64, String)>,
}

enum Outcome {
    Disabled,
    Verified(bool),
}

/// PRs QA should look at, oldest first.
pub fn eligible(prs: &[PullRequest], labels: &Labels) -> Vec<PullRequest> {
    let blockers = [&labels.qa_passed, &labels.qa_failed, &labels.qa_running, &labels.needs_human];
    prs.iter().filter(|p| p.title.starts_with(PR_TITLE_PREFIX) && !blockers.iter().any(|l| p.has_label(l))).cloned().collect()
}

impl Role for Qa {
    fn name(&self) -> &'static str {
        "qa"
    }

    fn recover(&mut self, ctx: &Ctx) {
        claims::recover(ctx);
    }

    fn cycle(&mut self, ctx: &Ctx) -> RoleResult<bool> {
        let list = repos::effective(&ctx.cfg, &ctx.paths.state(), false);
        let Some(repo) = self.rr.next(&list) else {
            crate::log::warn("Keine Repos konfiguriert");
            return Ok(false);
        };
        let setup = match workspace::setup(ctx, "qa", &repo) {
            Ok(s) => s,
            Err(e) if ctx.cfg.repo(&repo).is_none() => {
                crate::log::warn(&format!("{repo} nicht erreichbar ({e}) — aus der Discovery entfernt"));
                repos::forget(&ctx.paths.state(), &repo);
                return Ok(false);
            }
            Err(e) => return Err(RoleError::Failed(format!("{repo}: {e}"))),
        };
        let prs = setup.forge.open_prs()?;
        for pr in eligible(&prs, &ctx.cfg.labels) {
            if self.disabled.contains(&self.skip_key(&repo, &pr)) {
                continue;
            }
            let Claimed::Yes(pr) = self.claim(ctx, &setup, &pr)? else { continue };
            match self.verify(ctx, &setup, &pr)? {
                Outcome::Disabled => {
                    self.disabled.insert(self.skip_key(&repo, &pr));
                }
                Outcome::Verified(passed) => {
                    crate::log::info(&format!("{repo} PR #{}: {}", pr.number, if passed { "PASSED" } else { "FAILED" }));
                    return Ok(true);
                }
            }
        }
        crate::log::info(&format!("{repo}: keine PRs zur Prüfung"));
        Ok(false)
    }
}

impl Qa {
    fn skip_key(&self, repo: &str, pr: &PullRequest) -> (String, u64, String) {
        (repo.to_lowercase(), pr.number, pr.head.sha.clone())
    }

    /// Re-reads the PR under the lock and marks it `qa-running`.
    fn claim(&mut self, ctx: &Ctx, setup: &RepoSetup, pr: &PullRequest) -> Result<Claimed<PullRequest>, String> {
        let forge: &dyn Forge = &*setup.forge;
        let labels = ctx.cfg.labels.clone();
        let n = pr.number;
        let claimed = self.locks.with(forge, &format!("qa-pr-{n}"), || {
            let fresh = forge.pr(n)?;
            if eligible(std::slice::from_ref(&fresh), &labels).is_empty() {
                return Ok(Claimed::No);
            }
            forge.add_labels(n, &[&labels.qa_running])?;
            Ok(Claimed::Yes(fresh))
        })?;
        if matches!(claimed, Claimed::Yes(_)) {
            claims::add(&ctx.paths.state(), &setup.repo, n);
        }
        Ok(claimed)
    }

    /// Removes `qa-running` (and the local record), optionally adds the verdict label.
    fn release(ctx: &Ctx, setup: &RepoSetup, n: u64, verdict: Option<&str>) {
        let _ = setup.forge.remove_label(n, &ctx.cfg.labels.qa_running);
        if let Some(label) = verdict {
            if let Err(e) = setup.forge.add_labels(n, &[label]) {
                crate::log::warn(&format!("Label {label} an PR #{n} nicht gesetzt: {e}"));
            }
        }
        claims::remove(&ctx.paths.state(), &setup.repo, n);
    }

    fn verify(&mut self, ctx: &Ctx, setup: &RepoSetup, pr: &PullRequest) -> RoleResult<Outcome> {
        let n = pr.number;
        ctx.heartbeat.report("working", team_core::detail!["repo" => setup.repo, "pr" => n, "title" => pr.title]);
        if let Err(e) = setup.clone.reset_to_remote(&pr.head.name) {
            Self::release(ctx, setup, n, None);
            return Err(RoleError::Failed(format!("PR #{n}: checkout failed: {e}")));
        }
        let (steps, note, review) = match ProjectConfig::load(&setup.clone.dir) {
            Err(e) => (vec![StepResult::failure(".agent.toml", &e)], None, None),
            Ok(cfg) if !cfg.enabled("qa") => {
                crate::log::info(&format!("{} PR #{n}: QA in .agent.toml nicht aktiviert — übersprungen", setup.repo));
                Self::release(ctx, setup, n, None);
                return Ok(Outcome::Disabled);
            }
            Ok(cfg) => {
                let steps = steps::run_steps(&cfg, &setup.clone.dir);
                let mechanical = steps::mechanical_passed(&steps);
                let note = (!steps.iter().any(StepResult::ran)).then_some(NO_COMMANDS);
                let review = if mechanical && cfg.qa_review_enabled {
                    match verdict::review(ctx, &setup.repo, pr, &setup.clone.dir) {
                        Ok((v, _model)) => Some(v),
                        Err(e) => {
                            Self::release(ctx, setup, n, None);
                            return Err(e);
                        }
                    }
                } else {
                    None
                };
                (steps, note, review)
            }
        };
        let passed = steps::mechanical_passed(&steps) && !review.as_ref().is_some_and(|v| v.is_blocking());
        let body = verdict::comment(passed, &steps, note, review.as_ref());
        if let Err(e) = setup.forge.comment(n, &body) {
            crate::log::warn(&format!("QA-Kommentar an PR #{n} fehlgeschlagen: {e}"));
        }
        let label = if passed { &ctx.cfg.labels.qa_passed } else { &ctx.cfg.labels.qa_failed };
        Self::release(ctx, setup, n, Some(label));
        Ok(Outcome::Verified(passed))
    }
}
