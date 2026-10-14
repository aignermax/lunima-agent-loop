//! PRs this machine marked `qa-running` (`<state>/qa-claims.json`, `["owner/repo#12", …]`).
//! After a crash only these labels are ours to clear — another machine may be verifying
//! the others right now.

use crate::ctx::Ctx;
use std::path::{Path, PathBuf};
use team_core::state::write_atomic;

pub const FILE: &str = "qa-claims.json";

fn path(state: &Path) -> PathBuf {
    state.join(FILE)
}

/// Recorded claims (unreadable file → none).
pub fn load(state: &Path) -> Vec<String> {
    std::fs::read_to_string(path(state)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(state: &Path, list: &[String]) {
    let text = serde_json::to_string(list).unwrap_or_else(|_| "[]".into());
    if let Err(e) = write_atomic(&path(state), text.as_bytes()) {
        crate::log::warn(&format!("{FILE} nicht gespeichert: {e}"));
    }
}

/// `owner/repo#n`
pub fn key(repo: &str, number: u64) -> String {
    format!("{repo}#{number}")
}

pub fn add(state: &Path, repo: &str, number: u64) {
    let mut list = load(state);
    let k = key(repo, number);
    if !list.contains(&k) {
        list.push(k);
        save(state, &list);
    }
}

pub fn remove(state: &Path, repo: &str, number: u64) {
    let mut list = load(state);
    let k = key(repo, number);
    let before = list.len();
    list.retain(|x| x != &k);
    if list.len() != before {
        save(state, &list);
    }
}

/// Clears `qa-running` from every PR this machine had claimed; failed removals stay recorded.
pub fn recover(ctx: &Ctx) {
    let state = ctx.paths.state();
    let label = &ctx.cfg.labels.qa_running;
    let mut left = Vec::new();
    for entry in load(&state) {
        let Some((repo, n)) = entry.rsplit_once('#').and_then(|(r, n)| n.parse::<u64>().ok().map(|n| (r.to_string(), n))) else { continue };
        match (ctx.forge)(&repo).remove_label(n, label) {
            Ok(()) => crate::log::info(&format!("{label} von {repo}#{n} entfernt (Rest eines Absturzes)")),
            Err(e) => {
                crate::log::warn(&format!("{label} von {repo}#{n} nicht entfernt: {e}"));
                left.push(entry);
            }
        }
    }
    save(&state, &left);
}
