//! A role is one kind of team member; the daemon calls `cycle` in a loop on its own thread.

use crate::ctx::{Ctx, RoleResult};

pub trait Role: Send {
    /// `coder`, `qa`, `pr-feedback`, `product-owner` — key in control.json / status-<role>.json
    fn name(&self) -> &'static str;

    /// One unit of work (at most one issue / PR / comment). `Ok(true)` = something was done,
    /// so the daemon polls again right away instead of sleeping.
    fn cycle(&mut self, ctx: &Ctx) -> RoleResult<bool>;

    /// Once at start-up (e.g. clean up claims this machine left behind when it crashed).
    fn recover(&mut self, _ctx: &Ctx) {}
}

/// Exponential backoff after failed cycles: base, 2·base, 4·base … capped at 15 minutes.
pub fn backoff_secs(failures: u32, base: u64) -> u64 {
    if failures == 0 {
        return base;
    }
    base.saturating_mul(1u64 << failures.min(20)).min(900)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_matches_python() {
        assert_eq!(backoff_secs(0, 15), 15);
        assert_eq!(backoff_secs(1, 15), 30);
        assert_eq!(backoff_secs(3, 15), 120);
        assert_eq!(backoff_secs(6, 15), 900);
        assert_eq!(backoff_secs(100, 15), 900);
    }
}
