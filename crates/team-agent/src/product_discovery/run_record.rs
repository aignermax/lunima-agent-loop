//! `discovery/runs/<week>.json`: the week's progress, committed on the week's branch after
//! every step. Because it travels with the branch, a run that died on one machine is resumed
//! on any machine from the last finished stage, and the PR number and filed issues are known
//! without searching.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunRecord {
    pub week: String,
    /// scout | strategist | critic | apply, in the order finished
    pub stages_done: Vec<String>,
    pub pr: Option<u64>,
    /// item key → issue number (filed this week or found from earlier weeks)
    pub applied: BTreeMap<String, u64>,
    /// keys of issues this run created (the per-run issue cap counts these)
    pub created: Vec<String>,
    pub summary: String,
    pub critic_summary: String,
    /// validation problems that discarded stage output, privacy blocks, deferrals …
    pub notes: Vec<String>,
}

impl RunRecord {
    pub fn path(repo: &Path, week: &str) -> PathBuf {
        repo.join(super::model::ROOT).join("runs").join(format!("{week}.json"))
    }

    /// The record on the checked-out branch, or a fresh one.
    pub fn load(repo: &Path, week: &str) -> Self {
        std::fs::read_to_string(Self::path(repo, week)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| Self { week: week.into(), ..Default::default() })
    }

    pub fn save(&self, repo: &Path) -> Result<(), String> {
        let p = Self::path(repo, &self.week);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())
    }

    pub fn done(&self, stage: &str) -> bool {
        self.stages_done.iter().any(|s| s == stage)
    }

    pub fn finish(&mut self, stage: &str) {
        if !self.done(stage) {
            self.stages_done.push(stage.into());
        }
    }

    pub fn note(&mut self, text: impl Into<String>) {
        let t = text.into();
        if !self.notes.contains(&t) {
            self.notes.push(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_stage_bookkeeping() {
        let t = tempfile::tempdir().unwrap();
        let mut r = RunRecord::load(t.path(), "2026-w45");
        assert_eq!(r.week, "2026-w45");
        r.finish("scout");
        r.finish("scout");
        r.note("x");
        r.note("x");
        r.save(t.path()).unwrap();
        let back = RunRecord::load(t.path(), "2026-w45");
        assert_eq!((back.stages_done.len(), back.notes.len()), (1, 1));
        assert!(back.done("scout") && !back.done("critic"));
    }
}
