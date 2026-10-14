//! The UX reviewer: once per night simulated users (the personas of Lunima's
//! `docs/PERSONAS.md`) walk real journeys through the desktop app — first launch,
//! designing a circuit, importing a GDS … — driven by the computer-use tester in
//! `tools/ux-tester`. Findings become `ux-finding` issues without screenshots (the repo is
//! public; captures show the whole desktop) and the report feeds the Product Owner. It
//! never gates a merge.

mod plan;
mod report;
mod run;

#[cfg(test)]
mod tests;

pub use report::po_section;

use crate::ctx::{Ctx, RoleResult};
use crate::role::Role;
use chrono::{DateTime, Duration, Local};
use run::{Exec, Outcome};
use team_core::detail;
use team_core::state::UxState;

pub struct UxReviewer {
    exec: Exec,
    clock: Box<dyn Fn() -> DateTime<Local> + Send>,
    /// "not set up" was logged (once per process)
    warned: bool,
}

impl Default for UxReviewer {
    fn default() -> Self {
        Self { exec: Box::new(|line, cwd, timeout, env| crate::shell::run_shell_env(line, cwd, timeout, env)), clock: Box::new(Local::now), warned: false }
    }
}

fn stamp(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

impl UxReviewer {
    #[cfg(test)]
    pub fn with(exec: Exec, clock: Box<dyn Fn() -> DateTime<Local> + Send>) -> Self {
        Self { exec, clock, warned: false }
    }

    /// The repo to review, or why the role can't run.
    fn target(ctx: &Ctx) -> Result<String, &'static str> {
        let ux = &ctx.cfg.ux_review;
        let repo = if ux.repo.trim().is_empty() { ctx.cfg.product_owner.repo.clone() } else { ux.repo.clone() };
        if repo.is_empty() {
            return Err("kein Repo (ux_review.repo / product_owner.repo)");
        }
        if ux.tester_dir.trim().is_empty() || ux.api_key_file.trim().is_empty() {
            return Err("ux_review.tester_dir und ux_review.api_key_file fehlen in team.toml");
        }
        Ok(repo)
    }

    fn save(ctx: &Ctx, st: &UxState) {
        if let Err(e) = st.save(&ctx.paths.state()) {
            crate::log::warn(&format!("UX-Status nicht gespeichert: {e}"));
        }
    }
}

impl Role for UxReviewer {
    fn name(&self) -> &'static str {
        "ux-review"
    }

    fn cycle(&mut self, ctx: &Ctx) -> RoleResult<bool> {
        let repo = match Self::target(ctx) {
            Ok(r) => r,
            Err(why) => {
                if !self.warned {
                    crate::log::warn(&format!("UX-Review nicht eingerichtet: {why}"));
                    self.warned = true;
                }
                return Ok(false);
            }
        };
        let now = (self.clock)();
        let hour = ctx.cfg.ux_review.hour;
        let mut st = UxState::load(&ctx.paths.state()).unwrap_or_default();
        if !plan::due(&st, now, hour) {
            return Ok(false);
        }
        let day = plan::window_day(now, hour);
        let forge = (ctx.forge)(&repo);
        let key = plan::lock_key(day);
        if !forge.try_lock(&key, &ctx.machine)? && forge.lock_info(&key)?.is_some_and(|l| l.owner != ctx.machine) {
            st = UxState { day: day.to_string(), last_attempt: Some(stamp(now)), last_result: "skipped".into(), last_note: "lief heute auf einem anderen Rechner".into(), ..st };
            Self::save(ctx, &st);
            return Ok(false);
        }
        for old in 2..=4 {
            let _ = forge.unlock(&plan::lock_key(day - Duration::days(old)));
        }
        st.day = day.to_string();
        st.last_attempt = Some(stamp(now));
        st.last_start = Some(stamp(now));
        st.last_end = None;
        st.last_result = "running".into();
        st.last_note.clear();
        Self::save(ctx, &st);
        ctx.heartbeat.report("working", detail!["repo" => repo, "title" => "UX-Review (simulierte Nutzer)"]);
        crate::log::info(&format!("UX-Review für {repo} startet"));
        let outcome = run::review(ctx, &self.exec, &repo, day);
        let end = Some(stamp((self.clock)()));
        match outcome {
            Ok(Outcome::Done { report_dir, filed, goals, note }) => {
                st = UxState { last_end: end, last_result: "ok".into(), last_note: note, report_dir: Some(report_dir.display().to_string()), filed_issues: filed, goals, ..st };
            }
            Ok(Outcome::Locked) => {
                st = UxState { last_result: "skipped-locked".into(), last_note: "Bildschirm gesperrt — neuer Versuch in 30 Min.".into(), ..st };
            }
            Ok(Outcome::TimedOut { report_dir }) => {
                st = UxState { last_end: end, last_result: "timeout".into(), last_note: "Zeitlimit erreicht".into(), report_dir: report_dir.map(|p| p.display().to_string()).or(st.report_dir), ..st };
            }
            Err(e) => {
                crate::log::error(&format!("UX-Review: {e}"));
                st = UxState { last_end: end, last_result: "error".into(), last_note: e.chars().take(500).collect(), ..st };
            }
        }
        crate::log::info(&format!("UX-Review beendet: {} — {}", st.last_result, st.last_note));
        Self::save(ctx, &st);
        Ok(st.last_result == "ok")
    }
}
