//! PRs this machine marked `qa-running` (`<state>/qa-claims.json`, `["owner/repo#12", …]`).
//! After a crash only these labels are ours to clear — another machine may be verifying
//! the others right now.

use crate::ctx::Ctx;
use std::path::{Path, PathBuf};
use team_core::state::write_atomic;

pub const FILE: &str = "qa-claims.json";
/// The coder's PR claims (`agent-running` on PRs in review or QA fix).
pub const CODER_FILE: &str = "coder-pr-claims.json";

/// Recorded claims (unreadable file → none).
pub fn load(state: &Path) -> Vec<String> {
    load_from(state, FILE)
}

pub fn load_from(state: &Path, file: &str) -> Vec<String> {
    std::fs::read_to_string(state.join(file)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(state: &Path, file: &str, list: &[String]) {
    let text = serde_json::to_string(list).unwrap_or_else(|_| "[]".into());
    let path: PathBuf = state.join(file);
    if let Err(e) = write_atomic(&path, text.as_bytes()) {
        crate::log::warn(&format!("{file} nicht gespeichert: {e}"));
    }
}

/// `owner/repo#n`
pub fn key(repo: &str, number: u64) -> String {
    format!("{repo}#{number}")
}

pub fn add(state: &Path, repo: &str, number: u64) {
    add_to(state, FILE, repo, number);
}

pub fn add_to(state: &Path, file: &str, repo: &str, number: u64) {
    let mut list = load_from(state, file);
    let k = key(repo, number);
    if !list.contains(&k) {
        list.push(k);
        save(state, file, &list);
    }
}

pub fn remove(state: &Path, repo: &str, number: u64) {
    remove_from(state, FILE, repo, number);
}

pub fn remove_from(state: &Path, file: &str, repo: &str, number: u64) {
    let mut list = load_from(state, file);
    let k = key(repo, number);
    let before = list.len();
    list.retain(|x| x != &k);
    if list.len() != before {
        save(state, file, &list);
    }
}

/// Clears `qa-running` from every PR this machine had claimed; failed removals stay recorded.
pub fn recover(ctx: &Ctx) {
    recover_file(ctx, FILE, &ctx.cfg.labels.qa_running);
}

/// Removes `label` from every PR recorded in `file`.
pub fn recover_file(ctx: &Ctx, file: &str, label: &str) {
    let state = ctx.paths.state();
    let mut left = Vec::new();
    for entry in load_from(&state, file) {
        let Some((repo, n)) = entry.rsplit_once('#').and_then(|(r, n)| n.parse::<u64>().ok().map(|n| (r.to_string(), n))) else { continue };
        match (ctx.forge)(&repo).remove_label(n, label) {
            Ok(()) => crate::log::info(&format!("{label} von {repo}#{n} entfernt (Rest eines Absturzes)")),
            Err(e) => {
                crate::log::warn(&format!("{label} von {repo}#{n} nicht entfernt: {e}"));
                left.push(entry);
            }
        }
    }
    save(&state, file, &left);
}
