//! The Product Owner: reviews and merges agent PRs, keeps the integration branch fresh and
//! the backlog healthy — one Claude pass per `interval_minutes`, team-wide (see
//! `schedule` for how two machines agree on who runs a slot). Port of the C# agent loop's
//! owner pass; its workers are gone (the coder role implements the issues).

mod pass;
mod schedule;
mod state;

#[cfg(test)]
mod tests;

use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::role::Role;
use chrono::{DateTime, Local};
use pass::Ended;
use state::PoState;
use std::time::{Duration, Instant};
use team_core::detail;

/// Pre-flight (gh calls) at most this often while nothing is due.
const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct ProductOwner {
    last_check: Option<Instant>,
    /// slot this machine already decided about (ran, skipped as taken)
    settled_slot: Option<i64>,
}

impl ProductOwner {
    fn checked_recently(&self) -> bool {
        self.last_check.is_some_and(|t| t.elapsed() < CHECK_EVERY)
    }

    fn finish(ctx: &Ctx, st: &mut PoState, result: &str, note: &str, next: DateTime<Local>) {
        st.finished(Local::now(), result, note, Some(next));
        st.save(&ctx.paths.state());
        crate::log::info(&format!("PO-Durchlauf beendet: {result} — {}", note.chars().take(160).collect::<String>()));
    }
}

impl Role for ProductOwner {
    fn name(&self) -> &'static str {
        "product-owner"
    }

    fn cycle(&mut self, ctx: &Ctx) -> RoleResult<bool> {
        let po = &ctx.cfg.product_owner;
        if po.repo.is_empty() {
            return Ok(false);
        }
        let now = Local::now();
        let interval = po.interval_minutes.max(1);
        let slot = schedule::slot(now, interval);
        let next = schedule::next_slot_start(now, interval);
        let state_dir = ctx.paths.state();
        let mut st = PoState::load(&state_dir);
        st.roll_day(now);
        if self.settled_slot == Some(slot) || !schedule::due(st.last_start_time(), st.runs_today, po.max_runs_per_day, now, interval) || self.checked_recently() {
            return Ok(false);
        }
        self.last_check = Some(Instant::now());
        let forge = (ctx.forge)(&po.repo);
        let (prs, issues) = pass::backlog(ctx, forge.as_ref())?;
        if !pass::has_work(ctx, &prs, &issues) {
            st.result("skipped", "nichts zu tun: keine offenen Agent-PRs, Backlog gesund", Some(next));
            st.save(&state_dir);
            return Ok(false);
        }
        let key = schedule::slot_key(interval, slot);
        self.settled_slot = Some(slot);
        if !forge.try_lock(&key)? {
            st.result("skipped", "Durchlauf dieses Intervalls lief auf einem anderen Rechner", Some(next));
            st.save(&state_dir);
            return Ok(false);
        }
        for old in [slot - 1, slot - 2] {
            let _ = forge.unlock(&schedule::slot_key(interval, old));
        }
        st.started(now);
        st.next_due = Some(next.to_rfc3339_opts(chrono::SecondsFormat::Secs, false));
        st.save(&state_dir);
        ctx.heartbeat.report("working", detail!["repo" => po.repo, "prs" => prs.len(), "issues" => issues.len()]);
        crate::log::info(&format!("PO-Durchlauf für {} ({} PRs, {} Issues)", po.repo, prs.len(), issues.len()));
        match pass::run(ctx, &prs, &issues) {
            Ok(Ended::Ok(note)) => Self::finish(ctx, &mut st, "ok", &note, next),
            Ok(Ended::Timeout(note)) => Self::finish(ctx, &mut st, "timeout", &note, next),
            Ok(Ended::Error(note)) => Self::finish(ctx, &mut st, "error", &note, next),
            Err(RoleError::UsageLimit(t)) => {
                // the pass did not happen: give the slot back so it runs after the limit
                let _ = forge.unlock(&key);
                self.settled_slot = None;
                self.last_check = None;
                st.runs_today = st.runs_today.saturating_sub(1);
                st.last_start = None;
                Self::finish(ctx, &mut st, "usage-limit", "Claude-Nutzungslimit erreicht — Durchlauf folgt danach", t);
                return Err(RoleError::UsageLimit(t));
            }
            Err(RoleError::Failed(e)) => {
                Self::finish(ctx, &mut st, "error", &e, next);
                return Err(RoleError::Failed(e));
            }
        }
        Ok(true)
    }
}
