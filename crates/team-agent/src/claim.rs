//! Claiming work when several machines run the team agent with the same GitHub account.
//!
//! Labels and assignees can't be compared-and-set, so two machines could both see an issue
//! as free. Every claim therefore runs inside a short GitHub-side mutex: creating the hidden
//! ref `refs/team-locks/<key>` either succeeds or fails atomically. Inside the lock the
//! item is re-read and marked (label/assignee/comment) — the durable claim everyone sees —
//! and the lock is released again. A lock left behind by a crash is broken after
//! `STALE_AFTER`.

use crate::forge::Forge;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A claim takes seconds; a lock older than this belongs to a crashed machine.
pub const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// First sightings of locks held by others (to break stale ones).
#[derive(Default)]
pub struct Locks {
    seen: HashMap<String, Instant>,
}

#[derive(Debug, PartialEq)]
pub enum Claimed<T> {
    Yes(T),
    /// someone else holds the lock right now, or the item is no longer free
    No,
}

impl Locks {
    /// Runs `f` under the lock `key` of `forge`'s repository.
    pub fn with<T>(&mut self, forge: &dyn Forge, key: &str, f: impl FnOnce() -> Result<Claimed<T>, String>) -> Result<Claimed<T>, String> {
        let id = format!("{}:{key}", forge.name().to_lowercase());
        if !forge.try_lock(key)? {
            let first = *self.seen.entry(id.clone()).or_insert_with(Instant::now);
            if first.elapsed() < STALE_AFTER {
                return Ok(Claimed::No);
            }
            crate::log::warn(&format!("Sperre {key} in {} hängt seit über 10 Minuten — wird gebrochen", forge.name()));
            forge.unlock(key)?;
            self.seen.remove(&id);
            if !forge.try_lock(key)? {
                return Ok(Claimed::No);
            }
        }
        self.seen.remove(&id);
        let result = f();
        if let Err(e) = forge.unlock(key) {
            crate::log::warn(&format!("Sperre {key} nicht freigegeben: {e}"));
        }
        result
    }

    #[cfg(test)]
    pub fn age(&mut self, forge: &dyn Forge, key: &str, by: Duration) {
        let id = format!("{}:{key}", forge.name().to_lowercase());
        if let Some(t) = self.seen.get_mut(&id) {
            *t = t.checked_sub(by).unwrap_or(*t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeForge;

    #[test]
    fn second_claimer_backs_off_and_lock_is_released() {
        let f = FakeForge::new("o/r");
        let mut a = Locks::default();
        let mut b = Locks::default();
        let r = a.with(&f, "issue-1", || {
            // while A holds the lock, B gets nothing
            assert_eq!(b.with(&f, "issue-1", || Ok(Claimed::Yes(()))).unwrap(), Claimed::No);
            Ok(Claimed::Yes(1))
        });
        assert_eq!(r.unwrap(), Claimed::Yes(1));
        assert!(f.s.lock().unwrap().locks.is_empty());
        assert_eq!(b.with(&f, "issue-1", || Ok(Claimed::Yes(2))).unwrap(), Claimed::Yes(2));
    }

    #[test]
    fn errors_inside_still_release() {
        let f = FakeForge::new("o/r");
        let mut a = Locks::default();
        assert!(a.with::<()>(&f, "k", || Err("boom".into())).is_err());
        assert!(f.s.lock().unwrap().locks.is_empty());
    }

    #[test]
    fn stale_lock_is_broken() {
        let f = FakeForge::new("o/r");
        f.try_lock("issue-9").unwrap(); // crashed machine
        let mut a = Locks::default();
        assert_eq!(a.with(&f, "issue-9", || Ok(Claimed::Yes(()))).unwrap(), Claimed::No);
        a.age(&f, "issue-9", STALE_AFTER + Duration::from_secs(1));
        assert_eq!(a.with(&f, "issue-9", || Ok(Claimed::Yes(()))).unwrap(), Claimed::Yes(()));
    }
}
