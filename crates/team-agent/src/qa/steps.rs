//! The mechanical QA steps (`build_cmd`, `test_cmd`, `ui_test_cmd` from `.agent.toml`).

use crate::agent_toml::ProjectConfig;
use crate::shell::{run_shell, tail};
use std::path::Path;
use std::time::Duration;

const TAIL: usize = 2000;

/// Why a step did not run.
#[derive(Debug, Clone, PartialEq)]
pub enum Skip {
    NoCommand,
    BuildFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub name: String,
    /// `None` = ran; `Some` = skipped
    pub skipped: Option<Skip>,
    /// `None` when skipped or killed
    pub exit_code: Option<i32>,
    pub passed: bool,
    /// stdout then stderr (both — the Python agent hid stdout when stderr had anything)
    pub tail: String,
}

impl StepResult {
    pub fn ran(&self) -> bool {
        self.skipped.is_none()
    }

    fn skipped(name: &str, why: Skip) -> Self {
        Self { name: name.into(), skipped: Some(why), exit_code: None, passed: false, tail: String::new() }
    }

    /// A failed pseudo-step (e.g. a broken `.agent.toml`).
    pub fn failure(name: &str, message: &str) -> Self {
        Self { name: name.into(), skipped: None, exit_code: None, passed: false, tail: message.into() }
    }

    /// `ok`, `failed (exit 2)`, `skipped (no command configured)` …
    pub fn status(&self) -> String {
        match (&self.skipped, self.passed, self.exit_code) {
            (Some(Skip::NoCommand), ..) => "skipped (no command configured)".into(),
            (Some(Skip::BuildFailed), ..) => "skipped (build failed)".into(),
            (None, true, _) => "ok".into(),
            (None, false, Some(c)) => format!("failed (exit {c})"),
            (None, false, None) => "failed".into(),
        }
    }
}

fn run_step(name: &str, cmd: &str, dir: &Path, timeout_sec: u64) -> StepResult {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return StepResult::skipped(name, Skip::NoCommand);
    }
    match run_shell(cmd, dir, Duration::from_secs(timeout_sec)) {
        Err(e) => StepResult::failure(name, &e),
        Ok(r) => {
            let mut out = format!("{}{}", r.stdout, r.stderr);
            if r.timed_out {
                out.push_str(&format!("\ntimeout after {timeout_sec}s"));
            }
            StepResult { name: name.into(), skipped: None, exit_code: if r.timed_out { None } else { r.exit_code }, passed: r.ok(), tail: tail(out.trim(), TAIL) }
        }
    }
}

/// build → test → ui_test; a failed build skips the rest. Always three results.
pub fn run_steps(cfg: &ProjectConfig, dir: &Path) -> Vec<StepResult> {
    let build = run_step("build", &cfg.build_cmd, dir, cfg.command_timeout_sec);
    if build.ran() && !build.passed {
        return vec![build, StepResult::skipped("test", Skip::BuildFailed), StepResult::skipped("ui_test", Skip::BuildFailed)];
    }
    let test = run_step("test", &cfg.test_cmd, dir, cfg.command_timeout_sec);
    let ui = run_step("ui_test", &cfg.ui_test_cmd, dir, cfg.command_timeout_sec);
    vec![build, test, ui]
}

/// Passed iff something ran and nothing that ran failed.
pub fn mechanical_passed(steps: &[StepResult]) -> bool {
    steps.iter().any(StepResult::ran) && steps.iter().all(|s| !s.ran() || s.passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(build: &str, test: &str) -> ProjectConfig {
        ProjectConfig { build_cmd: build.into(), test_cmd: test.into(), command_timeout_sec: 60, ..Default::default() }
    }

    #[test]
    fn build_failure_short_circuits() {
        let t = tempfile::tempdir().unwrap();
        let s = run_steps(&cfg("exit 1", "exit 0"), t.path());
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].status(), "failed (exit 1)");
        assert_eq!(s[1].skipped, Some(Skip::BuildFailed));
        assert!(!mechanical_passed(&s));
    }

    #[test]
    fn nothing_configured_does_not_pass() {
        let t = tempfile::tempdir().unwrap();
        let s = run_steps(&cfg("", ""), t.path());
        assert!(s.iter().all(|x| !x.ran()));
        assert!(!mechanical_passed(&s));
    }

    #[test]
    fn tail_contains_stdout_and_stderr() {
        let t = tempfile::tempdir().unwrap();
        let s = run_step("test", "echo out-text && echo err-text 1>&2 && exit 2", t.path(), 60);
        assert_eq!(s.exit_code, Some(2));
        assert!(s.tail.contains("out-text") && s.tail.contains("err-text"));
    }
}
