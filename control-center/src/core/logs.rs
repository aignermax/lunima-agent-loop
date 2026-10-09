//! The loop's log folder: PO passes (`*_owner.jsonl`), worker runs (`*_task-<n>.jsonl`)
//! and the daily console mirror (`loop-<date>.log`).

use super::activity::{parse_pass, Pass};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Owner,
    Task(u32),
    Loop,
    Other,
}

#[derive(Debug, Clone)]
pub struct LogFile {
    pub path: PathBuf,
    pub name: String,
    pub kind: LogKind,
    pub started: Option<DateTime<Local>>,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

/// A PO pass with the file it came from.
#[derive(Debug, Clone)]
pub struct PassEntry {
    pub file: LogFile,
    pub pass: Pass,
    /// Exit code the loop recorded for this pass (state.json) — works for any runner.
    pub record_exit: Option<i32>,
}

/// How a PO pass ended, combining its log with the loop's own run record.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Running,
    Succeeded,
    Failed(String),
    Aborted,
}

impl PassEntry {
    /// `running`: a loop process is active *and* this is the newest log.
    pub fn outcome(&self, running: bool) -> Outcome {
        match (self.pass.finished, self.pass.is_error, self.record_exit) {
            (true, true, _) => Outcome::Failed(self.pass.summary.clone().unwrap_or_default()),
            (true, false, _) | (false, _, Some(0)) => Outcome::Succeeded,
            (false, _, Some(code)) => Outcome::Failed(format!("Exit-Code {code}")),
            (false, _, None) if running => Outcome::Running,
            (false, _, None) => Outcome::Aborted,
        }
    }
}

/// Matches each pass (newest first) to the owner run record written while it ran.
pub fn attach_records(passes: &mut [PassEntry], runs: &[crate::core::state::RunRecord]) {
    let mut until: Option<DateTime<Local>> = None;
    for entry in passes.iter_mut() {
        let Some(start) = entry.file.started else { continue };
        entry.record_exit = runs
            .iter()
            .filter(|r| r.kind == "owner")
            .filter_map(|r| crate::core::state::parse_time(&r.timestamp).map(|t| (t, r.exit_code)))
            .find(|(t, _)| *t >= start && until.is_none_or(|u| *t < u))
            .map(|(_, code)| code);
        until = Some(start);
    }
}

/// True if `file` is the newest PO or worker log — i.e. a running loop is in that pass.
pub fn is_newest_run_log(files: &[LogFile], file: &LogFile) -> bool {
    files.iter().find(|f| matches!(f.kind, LogKind::Owner | LogKind::Task(_))).is_some_and(|f| f.path == file.path)
}

/// "2026-10-09_083052_owner.jsonl" → kind + start time.
pub fn classify_name(name: &str) -> (LogKind, Option<DateTime<Local>>) {
    if let Some(date) = name.strip_prefix("loop-").and_then(|r| r.strip_suffix(".log")) {
        let t = NaiveDateTime::parse_from_str(&format!("{date} 00:00:00"), "%Y-%m-%d %H:%M:%S").ok();
        return (LogKind::Loop, t.and_then(|n| Local.from_local_datetime(&n).single()));
    }
    let started = name
        .get(..17)
        .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d_%H%M%S").ok())
        .and_then(|n| Local.from_local_datetime(&n).single());
    let rest = name.get(18..).unwrap_or("");
    let kind = if rest.starts_with("owner") {
        LogKind::Owner
    } else if let Some(n) = rest.strip_prefix("task-").and_then(|r| r.split('.').next()).and_then(|n| n.parse().ok()) {
        LogKind::Task(n)
    } else {
        LogKind::Other
    };
    (kind, started)
}

/// All log files, newest first (by start time, then modification time).
pub fn list(dir: &Path) -> Vec<LogFile> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<LogFile> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let name = e.file_name().to_string_lossy().into_owned();
            let (kind, started) = classify_name(&name);
            Some(LogFile { path: e.path(), name, kind, started, modified: meta.modified().ok(), size: meta.len() })
        })
        .collect();
    files.sort_by(|a, b| (b.started, b.modified).cmp(&(a.started, a.modified)));
    files
}

/// Parses PO logs once per (path, mtime, size); later calls are cheap.
#[derive(Default)]
pub struct PassCache {
    entries: HashMap<PathBuf, (Option<SystemTime>, u64, Pass)>,
}

impl PassCache {
    /// The newest `limit` PO passes.
    pub fn passes(&mut self, files: &[LogFile], limit: usize) -> Vec<PassEntry> {
        files
            .iter()
            .filter(|f| f.kind == LogKind::Owner)
            .take(limit)
            .map(|f| PassEntry { file: f.clone(), pass: self.get(f), record_exit: None })
            .collect()
    }

    fn get(&mut self, file: &LogFile) -> Pass {
        if let Some((m, s, p)) = self.entries.get(&file.path) {
            if *m == file.modified && *s == file.size {
                return p.clone();
            }
        }
        let text = std::fs::read(&file.path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let pass = parse_pass(&text);
        self.entries.insert(file.path.clone(), (file.modified, file.size, pass.clone()));
        pass
    }
}

/// Last `max_lines` lines of a text file (lossy UTF-8).
pub fn tail(path: &Path, max_lines: usize) -> String {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(max_lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_log_names() {
        assert_eq!(classify_name("2026-10-09_083052_owner.jsonl").0, LogKind::Owner);
        assert_eq!(classify_name("2026-08-14_131255_task-879.jsonl").0, LogKind::Task(879));
        assert_eq!(classify_name("loop-2026-10-09.log").0, LogKind::Loop);
        assert_eq!(classify_name("notes.txt").0, LogKind::Other);
        assert!(classify_name("2026-10-09_083052_owner.jsonl").1.is_some());
    }

    #[test]
    fn lists_newest_first_and_caches_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let result = r#"{"type":"result","is_error":false,"result":"ok"}"#;
        std::fs::write(tmp.path().join("2026-10-01_100000_owner.jsonl"), result).unwrap();
        std::fs::write(tmp.path().join("2026-10-02_100000_owner.jsonl"), "").unwrap();
        std::fs::write(tmp.path().join("2026-10-02_110000_task-5.jsonl"), "").unwrap();
        let files = list(tmp.path());
        assert_eq!(files[0].kind, LogKind::Task(5));
        let mut cache = PassCache::default();
        let passes = cache.passes(&files, 10);
        assert_eq!(passes.len(), 2);
        assert!(!passes[0].pass.finished);
        assert_eq!(passes[1].pass.summary.as_deref(), Some("ok"));
    }
}
