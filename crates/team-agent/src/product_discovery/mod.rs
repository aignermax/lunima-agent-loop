//! Product discovery: once a week, three separately prompted, sandboxed Claude sessions —
//! scout (evidence with provenance), strategist (hypotheses, opportunities, experiments,
//! Decision Records) and critic (tries to kill each proposal) — followed by deterministic
//! code that validates their output and files what survives. Design: docs/DISCOVERY.md.
//!
//! Nothing here contacts people, publishes pages or spends money: those are approval
//! requests for the maintainer. The Product Owner reads accepted Decision Records.

pub mod actions;
pub mod analytics;
pub mod context;
pub mod decisions;
pub mod ledger;
pub mod model;
pub mod page;
pub mod plan;
pub mod report;
pub mod run;
pub mod run_record;
pub mod stages;

#[cfg(test)]
mod tests;

use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::role::Role;
use chrono::{DateTime, Local};
use plan::Due;
use run::Ended;
use team_core::state::DiscoveryState;

pub const ROLE: &str = "product-discovery";

pub struct ProductDiscovery {
    clock: Box<dyn Fn() -> DateTime<Local> + Send>,
}

impl Default for ProductDiscovery {
    fn default() -> Self {
        Self { clock: Box::new(Local::now) }
    }
}

fn stamp(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

impl ProductDiscovery {
    #[cfg(test)]
    pub fn with_clock(clock: Box<dyn Fn() -> DateTime<Local> + Send>) -> Self {
        Self { clock }
    }

    pub fn target(ctx: &Ctx) -> String {
        let r = &ctx.cfg.product_discovery.repo;
        if r.trim().is_empty() { ctx.cfg.product_owner.repo.clone() } else { r.clone() }
    }

    fn save(ctx: &Ctx, st: &DiscoveryState) {
        if let Err(e) = st.save(&ctx.paths.state()) {
            crate::log::warn(&format!("Discovery-Status nicht gespeichert: {e}"));
        }
    }

    /// Takes the week's lock; `Ok(false)` = another machine has it (running or finished).
    fn lock(ctx: &Ctx, forge: &dyn crate::forge::Forge, key: &str) -> Result<bool, String> {
        if forge.try_lock(key, &ctx.machine)? {
            return Ok(true);
        }
        let Some(info) = forge.lock_info(key)? else { return forge.try_lock(key, &ctx.machine) };
        if info.owner == ctx.machine {
            return Ok(true); // this machine's own run, interrupted by a restart
        }
        if chrono::Utc::now() - info.created > plan::stale_after(&ctx.cfg.product_discovery) && !Self::finished_elsewhere(forge, key) {
            crate::log::warn(&format!("Discovery-Sperre {key} von {} ist veraltet — wird gebrochen", info.owner));
            forge.unlock(key)?;
            return forge.try_lock(key, &ctx.machine);
        }
        Ok(false)
    }

    /// A finished week keeps its lock as proof; the week's report says it finished.
    fn finished_elsewhere(forge: &dyn crate::forge::Forge, key: &str) -> bool {
        let week = key.trim_start_matches("discovery-");
        forge.open_issues(actions::LABEL_REPORT).ok().into_iter().flatten().any(|i| i.body.as_deref().is_some_and(|b| b.contains(&format!("<!-- discovery:report week={week} -->"))))
    }
}

impl Role for ProductDiscovery {
    fn name(&self) -> &'static str {
        ROLE
    }

    fn cycle(&mut self, ctx: &Ctx) -> RoleResult<bool> {
        let repo = Self::target(ctx);
        if repo.is_empty() {
            return Ok(false);
        }
        let cfg = &ctx.cfg.product_discovery;
        let now = (self.clock)();
        let week = plan::week_key(now);
        let mut st = DiscoveryState::load(&ctx.paths.state()).unwrap_or_default();
        let trigger = DiscoveryState::dir(&ctx.paths.state()).join(team_core::state::DISCOVERY_RUN_NOW_FILE);
        let forced = trigger.is_file();
        if forced {
            // one request = one try; a week that is already over stays over
            let _ = std::fs::remove_file(&trigger);
        }
        let due = if forced { plan::due_now(&st, now, cfg) } else { plan::due(&st, now, cfg) };
        if let Due::No(next) = due {
            let next = Some(stamp(next));
            if st.next_due != next {
                st.next_due = next;
                Self::save(ctx, &st);
            }
            return Ok(false);
        }
        let forge = (ctx.forge)(&repo);
        let key = plan::lock_key(&week);
        if st.week != week {
            st = DiscoveryState { week: week.clone(), ..Default::default() };
        }
        if !Self::lock(ctx, forge.as_ref(), &key)? {
            st.last_attempt = Some(stamp(now));
            st.last_result = "skipped".into();
            st.last_note = "diese Woche läuft oder lief auf einem anderen Rechner".into();
            Self::save(ctx, &st);
            return Ok(false);
        }
        st.attempts += 1;
        st.last_attempt = Some(stamp(now));
        st.last_start = Some(stamp(now));
        st.last_end = None;
        st.last_result = "running".into();
        st.last_note.clear();
        Self::save(ctx, &st);
        crate::log::info(&format!("Product Discovery {week} für {repo} startet (Versuch {})", st.attempts));
        let outcome = run::week(ctx, &repo, &week, &mut st);
        st.last_end = Some(stamp((self.clock)()));
        st.stage.clear();
        let result = match outcome {
            Ok(Ended::Done(note)) => {
                st.last_result = "ok".into();
                st.last_note = note;
                Ok(true)
            }
            Ok(Ended::Stopped(kind, note)) => {
                st.last_result = kind.into();
                st.last_note = note;
                if kind == "paused" {
                    // not a failed attempt: resume once unpaused, maybe on another machine
                    st.attempts = st.attempts.saturating_sub(1);
                    let _ = forge.unlock(&key);
                }
                Ok(false)
            }
            Err(RoleError::UsageLimit(t)) => {
                st.last_result = "usage-limit".into();
                st.last_note = format!("Claude-Nutzungslimit bis {}", t.format("%d.%m. %H:%M"));
                st.attempts = st.attempts.saturating_sub(1);
                let _ = forge.unlock(&key);
                Err(RoleError::UsageLimit(t))
            }
            Err(RoleError::Failed(e)) => {
                st.last_result = "error".into();
                st.last_note = e.chars().take(500).collect();
                let _ = forge.unlock(&key);
                Err(RoleError::Failed(e))
            }
        };
        st.next_due = match plan::due(&st, (self.clock)(), cfg) {
            Due::No(t) => Some(stamp(t)),
            Due::Run => None,
        };
        crate::log::info(&format!("Product Discovery {week} beendet: {} — {}", st.last_result, st.last_note));
        Self::save(ctx, &st);
        result
    }
}
