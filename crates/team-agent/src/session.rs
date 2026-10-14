//! Per-issue progress of the coder on this machine: `sessions/<owner_repo>/issue-<n>.json`.
//! (The Python agent keyed files by issue number only, so equal numbers in two repos
//! collided.) Attempts (errors) are counted separately from sessions (continuations).

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use team_core::state::write_atomic;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    pub repo: String,
    pub issue_number: u64,
    pub branch_name: String,
    pub started_at: String,
    pub last_session_at: String,
    pub total_turns_used: u32,
    /// Claude sessions that finished (continuations included)
    pub session_count: u32,
    /// runs that ended in an error (Claude crash, push failure …)
    pub attempts: u32,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub completed: bool,
    pub pr_url: Option<String>,
    pub last_output: String,
    pub notes: Vec<String>,
    /// coding finished and the PR exists — a resumed run continues with the review
    pub in_review: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        let now = Local::now().to_rfc3339();
        Self {
            repo: String::new(),
            issue_number: 0,
            branch_name: String::new(),
            started_at: now.clone(),
            last_session_at: now,
            total_turns_used: 0,
            session_count: 0,
            attempts: 0,
            total_tokens: 0,
            total_cost_usd: 0.0,
            completed: false,
            pr_url: None,
            last_output: String::new(),
            notes: vec![],
            in_review: false,
        }
    }
}

impl SessionState {
    pub fn new(repo: &str, issue: u64, branch: &str) -> Self {
        let mut s = Self { repo: repo.into(), issue_number: issue, branch_name: branch.into(), ..Default::default() };
        s.note("Session created");
        s
    }

    pub fn note(&mut self, text: &str) {
        self.notes.push(format!("[{}] {text}", Local::now().format("%Y-%m-%dT%H:%M:%S")));
    }

    pub fn add_session(&mut self, turns: u32, tokens: u64, cost: f64) {
        self.session_count += 1;
        self.total_turns_used += turns;
        self.total_tokens += tokens;
        self.total_cost_usd += cost;
        self.last_session_at = Local::now().to_rfc3339();
    }

    pub fn duration_sec(&self) -> Option<i64> {
        DateTime::parse_from_rfc3339(&self.started_at).ok().map(|t| (Local::now() - t.with_timezone(&Local)).num_seconds())
    }
}

/// Session files of one machine.
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(sessions_dir: &Path) -> Self {
        Self { dir: sessions_dir.to_path_buf() }
    }

    fn path(&self, repo: &str, issue: u64) -> PathBuf {
        self.dir.join(repo.replace('/', "_")).join(format!("issue-{issue}.json"))
    }

    /// Unreadable file → none (logged).
    pub fn load(&self, repo: &str, issue: u64) -> Option<SessionState> {
        let text = std::fs::read_to_string(self.path(repo, issue)).ok()?;
        match serde_json::from_str(&text) {
            Ok(s) => Some(s),
            Err(e) => {
                crate::log::warn(&format!("Sitzung {repo}#{issue} unlesbar ({e}) — beginne neu"));
                None
            }
        }
    }

    pub fn save(&self, s: &SessionState) {
        let text = serde_json::to_string_pretty(s).unwrap_or_default();
        if let Err(e) = write_atomic(&self.path(&s.repo, s.issue_number), text.as_bytes()) {
            crate::log::warn(&format!("Sitzung {}#{} nicht gespeichert: {e}", s.repo, s.issue_number));
        }
    }

    pub fn delete(&self, repo: &str, issue: u64) {
        let _ = std::fs::remove_file(self.path(repo, issue));
    }

    /// Unfinished sessions of a repository (work this machine has to resume), by issue number.
    pub fn open_in(&self, repo: &str) -> Vec<SessionState> {
        let dir = self.dir.join(repo.replace('/', "_"));
        let mut out: Vec<SessionState> = std::fs::read_dir(dir)
            .map(|it| it.flatten().filter_map(|e| std::fs::read_to_string(e.path()).ok()).filter_map(|t| serde_json::from_str::<SessionState>(&t).ok()).filter(|s| !s.completed).collect())
            .unwrap_or_default();
        out.sort_by_key(|s| s.issue_number);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_is_per_repo() {
        let t = tempfile::tempdir().unwrap();
        let store = Store::new(t.path());
        let mut a = SessionState::new("o/a", 7, "agent/issue-7-1");
        a.add_session(10, 1000, 0.5);
        store.save(&a);
        store.save(&SessionState::new("o/b", 7, "agent/issue-7-2"));
        let back = store.load("o/a", 7).unwrap();
        assert_eq!(back.branch_name, "agent/issue-7-1");
        assert_eq!((back.session_count, back.total_turns_used, back.total_tokens), (1, 10, 1000));
        assert_eq!(store.load("o/b", 7).unwrap().branch_name, "agent/issue-7-2");
        assert_eq!(store.open_in("o/a").len(), 1);
        store.delete("o/a", 7);
        assert!(store.load("o/a", 7).is_none());
        assert!(store.load("o/b", 7).is_some());
    }

    #[test]
    fn corrupt_file_counts_as_missing() {
        let t = tempfile::tempdir().unwrap();
        let store = Store::new(t.path());
        std::fs::create_dir_all(t.path().join("o_a")).unwrap();
        std::fs::write(t.path().join("o_a").join("issue-1.json"), "{").unwrap();
        assert!(store.load("o/a", 1).is_none());
        assert!(store.open_in("o/a").is_empty());
    }
}
