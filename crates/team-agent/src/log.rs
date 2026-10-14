//! Plain-text log: stderr + `<logs>/agent.log` (shared by all role threads of this process,
//! rotated by size: agent.log → agent.log.1 → … keeping `BACKUPS`).

use chrono::Local;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 20 * 1024 * 1024;
const BACKUPS: u32 = 3;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

thread_local! {
    static ROLE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Sends the log to `<dir>/agent.log` from now on.
pub fn init(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    *FILE.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir.join("agent.log"));
}

/// Tags this thread's lines with the role name.
pub fn set_role(role: &str) {
    ROLE.with(|r| *r.borrow_mut() = role.to_string());
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

fn write(level: &str, msg: &str) {
    let role = ROLE.with(|r| r.borrow().clone());
    let tag = if role.is_empty() { String::new() } else { format!("[{role}] ") };
    let line = format!("{} [{level}] {tag}{msg}", Local::now().format("%Y-%m-%d %H:%M:%S"));
    eprintln!("{line}");
    let guard = FILE.lock().unwrap_or_else(|p| p.into_inner());
    let Some(path) = guard.as_ref() else { return };
    rotate(path);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn rotate(path: &Path) {
    if std::fs::metadata(path).map(|m| m.len() < MAX_BYTES).unwrap_or(true) {
        return;
    }
    let numbered = |n: u32| PathBuf::from(format!("{}.{n}", path.display()));
    let _ = std::fs::remove_file(numbered(BACKUPS));
    for n in (1..BACKUPS).rev() {
        let _ = std::fs::rename(numbered(n), numbered(n + 1));
    }
    let _ = std::fs::rename(path, numbered(1));
}
