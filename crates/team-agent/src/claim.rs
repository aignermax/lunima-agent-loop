//! Claiming work when several machines run the team agent with the same GitHub account.
//!
//! Labels and assignees can't be compared-and-set, so two machines could both see an issue
//! as free. Every claim therefore runs inside a short GitHub-side mutex: creating the hidden
//! ref `refs/team-locks/<key>` either succeeds or fails atomically. Inside the lock the
//! item is re-read and marked (label/assignee/comment) — the durable claim everyone sees —
//! and the lock is released again. The lock commit carries its owner and GitHub's own
//! timestamp, so a lock older than `STALE_AFTER` is known to belong to a crashed claim
//! (no local observation, no machine clocks involved).

use crate::forge::Forge;
use std::time::Duration;

/// A claim takes seconds; a lock older than this belongs to a crashed machine.
pub const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// Cross-machine claim helper (stateless; kept as a type so roles own one).
#[derive(Default)]
pub struct Locks;

#[derive(Debug, PartialEq)]
pub enum Claimed<T> {
    Yes(T),
    /// someone else holds the lock right now, or the item is no longer free
    No,
}

impl Locks {
    /// Runs `f` under the lock `key` of `forge`'s repository, as `owner` (machine name).
    pub fn with<T>(&mut self, forge: &dyn Forge, key: &str, owner: &str, f: impl FnOnce() -> Result<Claimed<T>, String>) -> Result<Claimed<T>, String> {
        if !forge.try_lock(key, owner)? {
            let Some(info) = forge.lock_info(key)? else { return Ok(Claimed::No) }; // released meanwhile — next cycle
            let age = (chrono::Utc::now() - info.created).to_std().unwrap_or_default();
            if age < STALE_AFTER {
                return Ok(Claimed::No);
            }
            crate::log::warn(&format!("Sperre {key} in {} von {} ist {} min alt — wird gebrochen", forge.name(), info.owner, age.as_secs() / 60));
            forge.unlock(key)?;
            if !forge.try_lock(key, owner)? {
                return Ok(Claimed::No);
            }
        }
        let result = f();
        if let Err(e) = forge.unlock(key) {
            crate::log::warn(&format!("Sperre {key} nicht freigegeben: {e}"));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeForge;

    #[test]
    fn second_claimer_backs_off_and_lock_is_released() {
        let f = FakeForge::new("o/r");
        let r = Locks.with(&f, "issue-1", "A", || {
            // while A holds the lock, B gets nothing
            assert_eq!(Locks.with(&f, "issue-1", "B", || Ok(Claimed::Yes(()))).unwrap(), Claimed::No);
            Ok(Claimed::Yes(1))
        });
        assert_eq!(r.unwrap(), Claimed::Yes(1));
        assert!(f.s.lock().unwrap().locks.is_empty());
        assert_eq!(Locks.with(&f, "issue-1", "B", || Ok(Claimed::Yes(2))).unwrap(), Claimed::Yes(2));
    }

    #[test]
    fn errors_inside_still_release() {
        let f = FakeForge::new("o/r");
        assert!(Locks.with::<()>(&f, "k", "A", || Err("boom".into())).is_err());
        assert!(f.s.lock().unwrap().locks.is_empty());
    }

    #[test]
    fn only_an_old_lock_is_broken() {
        let f = FakeForge::new("o/r");
        f.try_lock("issue-9", "CRASHED").unwrap();
        assert_eq!(Locks.with(&f, "issue-9", "A", || Ok(Claimed::Yes(()))).unwrap(), Claimed::No, "fresh lock of another machine is respected");
        f.age_lock("issue-9", STALE_AFTER + Duration::from_secs(1));
        assert_eq!(Locks.with(&f, "issue-9", "A", || Ok(Claimed::Yes(()))).unwrap(), Claimed::Yes(()));
    }
}
