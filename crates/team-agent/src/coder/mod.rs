//! The coder: implements `agent-task` issues as PRs and repairs `qa-failed` agent PRs.
//!
//! One cycle = one repository (round robin, repos with known work first):
//! 1. resume this machine's unfinished issues there (after a restart or usage limit),
//! 2. else repair the oldest `qa-failed` agent PR,
//! 3. else claim the oldest free `agent-task` issue and work on it.
//! The discovery sweep over the org runs here too (throttled).

mod finish;
mod pickup;
mod prompt;
mod qa_fix;
mod review_loop;
mod work;

#[cfg(test)]
mod tests;

use crate::claim::Locks;
use crate::ctx::{Ctx, RoleResult};
use crate::repos::{self, RoundRobin};
use crate::role::Role;
use crate::session::Store;
use crate::workspace::{self, RepoSetup};
use std::time::{Duration, Instant};
use work::IssueRun;

pub type Search = Box<dyn Fn(&str) -> Result<Vec<String>, String> + Send>;

pub struct Coder {
    locks: Locks,
    rr: RoundRobin,
    last_discovery: Option<Instant>,
    search: Search,
}

impl Default for Coder {
    fn default() -> Self {
        Self::new(Box::new(|q: &str| team_core::gh::search_repos(q)))
    }
}

impl Coder {
    pub fn new(search: Search) -> Self {
        Self { locks: Locks::default(), rr: RoundRobin::default(), last_discovery: None, search }
    }

    fn discover(&mut self, ctx: &Ctx) {
        let d = &ctx.cfg.discovery;
        if d.org.is_empty() || self.last_discovery.is_some_and(|t| t.elapsed() < Duration::from_secs(d.interval_secs)) {
            return;
        }
        self.last_discovery = Some(Instant::now());
        match repos::sweep(&ctx.cfg, &ctx.paths.state(), &*self.search) {
            Ok(added) => added.iter().for_each(|r| crate::log::info(&format!("Neues Repo mit Agent-Arbeit entdeckt: {r}"))),
            Err(e) => crate::log::warn(&format!("Discovery fehlgeschlagen (nächster Versuch später): {e}")),
        }
    }

    /// Repos with work according to the last sweep come first; without hints, all.
    fn pick_repo(&mut self, ctx: &Ctx) -> Option<String> {
        let state = ctx.paths.state();
        let all = repos::effective(&ctx.cfg, &state, true);
        let hints = repos::load_hints(&state, chrono::Duration::seconds(3 * ctx.cfg.discovery.interval_secs.max(1) as i64));
        let store = Store::new(&ctx.paths.sessions());
        let pool: Vec<String> = all.iter().filter(|r| hints.contains(&r.to_lowercase()) || !store.open_in(r).is_empty()).cloned().collect();
        self.rr.next(if pool.is_empty() { &all } else { &pool })
    }

    fn work_in(&mut self, ctx: &Ctx, setup: &RepoSetup) -> RoleResult<bool> {
        let store = Store::new(&ctx.paths.sessions());
        let run = IssueRun { ctx, setup, store: &store };
        if let Some((issue, state)) = resumable(ctx, setup, &store) {
            crate::log::info(&format!("Setze {}#{} fort", setup.repo, issue.number));
            run.run(issue, state)?;
            return Ok(true);
        }
        if let Err(e) = team_core::provider::choose(&ctx.cfg, &setup.repo, &[]) {
            crate::log::error(&format!("{}: {e} — Repo übersprungen", setup.repo));
            return Ok(false);
        }
        if qa_fix::run(ctx, setup, &mut self.locks)? {
            return Ok(true);
        }
        let has_state = |n: u64| store.load(&setup.repo, n).is_some();
        let Some(issue) = pickup::claim_next(ctx, setup.forge.as_ref(), &mut self.locks, &has_state)? else { return Ok(false) };
        let state = store.load(&setup.repo, issue.number).map(|mut s| {
            if s.handed_to_human {
                s.restart_after_human();
            }
            s
        });
        run.run(issue, state)?;
        Ok(true)
    }
}

/// An unfinished session of this machine whose issue is still claimed (open + running).
/// Sessions of issues that were closed or taken over are dropped.
fn resumable(ctx: &Ctx, setup: &RepoSetup, store: &Store) -> Option<(team_core::gh::Issue, Option<crate::session::SessionState>)> {
    for state in store.open_in(&setup.repo) {
        let Ok(issue) = setup.forge.issue(state.issue_number) else { continue };
        if issue.state != "open" {
            store.delete(&setup.repo, issue.number);
            continue;
        }
        // still ours: running, and no other machine claimed it after a re-queue
        if issue.has_label(&ctx.cfg.labels.running) && pickup::claimed_by(setup.forge.as_ref(), issue.number).is_none_or(|m| m == ctx.machine) {
            return Some((issue, Some(state)));
        }
    }
    // own claims whose session file is missing (crash right after claiming)
    let has_state = |n: u64| store.load(&setup.repo, n).is_some();
    pickup::running_claims(ctx, setup.forge.as_ref(), &has_state).into_iter().next().map(|i| (i, None))
}

impl Role for Coder {
    fn name(&self) -> &'static str {
        "coder"
    }

    /// PRs this machine marked `agent-running` before a crash are released.
    fn recover(&mut self, ctx: &Ctx) {
        crate::qa::claims::recover_file(ctx, crate::qa::claims::CODER_FILE, &ctx.cfg.labels.running);
    }

    fn cycle(&mut self, ctx: &Ctx) -> RoleResult<bool> {
        self.discover(ctx);
        let Some(repo) = self.pick_repo(ctx) else {
            crate::log::warn("Keine Repos konfiguriert oder entdeckt");
            return Ok(false);
        };
        let setup = match workspace::setup(ctx, "coder", &repo) {
            Ok(s) => s,
            Err(e) if ctx.cfg.repo(&repo).is_none() => {
                crate::log::warn(&format!("{repo} nicht nutzbar ({e}) — aus der Discovery entfernt"));
                repos::forget(&ctx.paths.state(), &repo);
                return Ok(false);
            }
            Err(e) => return Err(format!("{repo}: {e}").into()),
        };
        self.work_in(ctx, &setup)
    }
}
