//! Runs a project command line (`build_cmd`, `test_cmd` from .agent.toml) through the
//! platform shell (`cmd /C` on Windows, `sh -c` elsewhere), killing the whole tree on timeout.

use std::path::Path;
use std::time::Duration;
use team_core::proc::{command, run};

#[derive(Debug, Clone, PartialEq)]
pub struct CmdResult {
    /// None = killed (timeout) or not started
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CmdResult {
    pub fn ok(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

pub fn run_shell(line: &str, cwd: &Path, timeout: Duration) -> Result<CmdResult, String> {
    run_shell_env(line, cwd, timeout, &[])
}

/// `run_shell` with extra environment variables for the command.
pub fn run_shell_env(line: &str, cwd: &Path, timeout: Duration, env: &[(&str, &std::ffi::OsStr)]) -> Result<CmdResult, String> {
    #[cfg(windows)]
    let mut cmd = {
        // verbatim: Rust's argument quoting (`\"`) is not understood by cmd.exe
        use std::os::windows::process::CommandExt;
        let mut c = command("cmd");
        c.raw_arg(format!("/S /C \"{line}\""));
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = command("sh");
        c.arg("-c").arg(line);
        c
    };
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = run(cmd, Some(cwd), timeout).map_err(|e| format!("`{line}` konnte nicht gestartet werden: {e}"))?;
    Ok(CmdResult { exit_code: out.status, stdout: out.stdout, stderr: out.stderr, timed_out: out.timed_out })
}

/// The last `max` characters, prefixed with `...\n` when cut.
pub fn tail(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    format!("...\n{}", text.chars().skip(n - max).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_and_output() {
        let t = tempfile::tempdir().unwrap();
        let ok = run_shell("echo hello", t.path(), Duration::from_secs(30)).unwrap();
        assert!(ok.ok() && ok.stdout.contains("hello"));
        let bad = run_shell("exit 3", t.path(), Duration::from_secs(30)).unwrap();
        assert_eq!(bad.exit_code, Some(3));
        assert!(!bad.ok());
    }

    #[test]
    fn quoted_arguments_reach_the_shell_intact() {
        let t = tempfile::tempdir().unwrap();
        let r = run_shell("echo \"a b\"> \"out file.txt\"", t.path(), Duration::from_secs(30)).unwrap();
        assert!(r.ok(), "{r:?}");
        assert!(std::fs::read_to_string(t.path().join("out file.txt")).unwrap().contains("a b"));
    }

    #[test]
    fn tail_cuts_from_the_front() {
        assert_eq!(tail("abc", 5), "abc");
        assert_eq!(tail("abcdef", 3), "...\ndef");
    }
}
