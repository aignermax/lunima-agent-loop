//! The team agent's log folder: `agent.log` (+ rotated `.1`…) with every role's text log,
//! and `<role>/<YYYY-MM-DD_HHMMSS>_<label>.jsonl` with each Claude session's raw stream-json.

use super::activity::{parse_pass, Pass};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const PO_ROLE: &str = "product-owner";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogKind {
    /// agent.log or a rotated copy
    Agent,
    /// one Claude session of a role (folder name)
    Session(String),
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

impl LogFile {
    pub fn role(&self) -> Option<&str> {
        match &self.kind {
            LogKind::Session(r) => Some(r),
            LogKind::Agent => None,
        }
    }
}

/// A product-owner session with the file it came from.
#[derive(Debug, Clone)]
pub struct PassEntry {
    pub file: LogFile,
    pub pass: Pass,
}

/// How a PO pass ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Running,
    Succeeded,
    Failed(String),
    Aborted,
}

impl PassEntry {
    /// `running`: the product owner reports `working` *and* this is its newest log.
    pub fn outcome(&self, running: bool) -> Outcome {
        match (self.pass.finished, self.pass.is_error) {
            (true, true) => Outcome::Failed(self.pass.summary.clone().unwrap_or_default()),
            (true, false) => Outcome::Succeeded,
            (false, _) if running => Outcome::Running,
            (false, _) => Outcome::Aborted,
        }
    }
}

/// "2026-10-09_083052_po_o_r.jsonl" → start time.
pub fn started_from_name(name: &str) -> Option<DateTime<Local>> {
    let n = NaiveDateTime::parse_from_str(name.get(..17)?, "%Y-%m-%d_%H%M%S").ok()?;
    Local.from_local_datetime(&n).earliest()
}

fn file_entry(path: PathBuf, kind: LogKind) -> Option<LogFile> {
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let started = match kind {
        LogKind::Session(_) => started_from_name(&name),
        LogKind::Agent => None,
    };
    Some(LogFile { started, modified: meta.modified().ok(), size: meta.len(), name, path, kind })
}

/// agent.log first (current, then rotated), then all session logs newest first.
pub fn list(dir: &Path) -> Vec<LogFile> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut agent = Vec::new();
    let mut sessions = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            let Ok(files) = std::fs::read_dir(&path) else { continue };
            sessions.extend(files.flatten().filter_map(|f| file_entry(f.path(), LogKind::Session(name.clone()))));
        } else if name.starts_with("agent.log") {
            agent.extend(file_entry(path, LogKind::Agent));
        }
    }
    agent.sort_by(|a, b| a.name.len().cmp(&b.name.len()).then(a.name.cmp(&b.name)));
    sessions.sort_by(|a, b| (b.started, b.modified).cmp(&(a.started, a.modified)));
    agent.extend(sessions);
    agent
}

/// The newest session log of `role`.
pub fn newest_of<'a>(files: &'a [LogFile], role: &str) -> Option<&'a LogFile> {
    files.iter().find(|f| f.role() == Some(role))
}

/// Parses PO logs once per (path, mtime, size); later calls are cheap.
#[derive(Default)]
pub struct PassCache {
    entries: HashMap<PathBuf, (Option<SystemTime>, u64, Pass)>,
}

impl PassCache {
    /// The newest `limit` product-owner passes.
    pub fn passes(&mut self, files: &[LogFile], limit: usize) -> Vec<PassEntry> {
        files.iter().filter(|f| f.role() == Some(PO_ROLE)).take(limit).map(|f| PassEntry { file: f.clone(), pass: self.get(f) }).collect()
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
    fn lists_agent_log_then_sessions_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let po = tmp.path().join(PO_ROLE);
        let coder = tmp.path().join("coder");
        std::fs::create_dir_all(&po).unwrap();
        std::fs::create_dir_all(&coder).unwrap();
        std::fs::write(tmp.path().join("agent.log.1"), "old").unwrap();
        std::fs::write(tmp.path().join("agent.log"), "new").unwrap();
        let result = r#"{"type":"result","is_error":false,"result":"ok"}"#;
        std::fs::write(po.join("2026-10-01_100000_po_o_r.jsonl"), result).unwrap();
        std::fs::write(po.join("2026-10-02_100000_po_o_r.jsonl"), "").unwrap();
        std::fs::write(coder.join("2026-10-02_110000_coder_o_r_issue-5.jsonl"), "").unwrap();
        let files = list(tmp.path());
        assert_eq!(files[0].name, "agent.log");
        assert_eq!(files[1].name, "agent.log.1");
        assert_eq!(files[2].role(), Some("coder"));
        assert!(files[2].started.is_some());
        assert_eq!(newest_of(&files, PO_ROLE).unwrap().name, "2026-10-02_100000_po_o_r.jsonl");
        let mut cache = PassCache::default();
        let passes = cache.passes(&files, 10);
        assert_eq!(passes.len(), 2);
        assert_eq!(passes[0].outcome(true), Outcome::Running);
        assert_eq!(passes[0].outcome(false), Outcome::Aborted);
        assert_eq!(passes[1].pass.summary.as_deref(), Some("ok"));
        assert_eq!(passes[1].outcome(false), Outcome::Succeeded);
    }
}
