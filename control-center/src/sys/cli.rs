//! Drives the loop through its own CLI, so pause/resume/passes behave exactly like
//! the scheduled task (same state file, same logs, same locking).

use super::{hidden, run};
use std::path::Path;

/// Short commands (pause, resume, status): run and return their output.
pub fn run_cli(cli: &Path, root: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = hidden(cli);
    cmd.args(args).current_dir(root);
    run(cmd).map(|s| s.trim().to_string())
}

/// Long passes (own, run, work): start detached — progress shows up in logs/.
pub fn spawn_cli(cli: &Path, root: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = hidden(cli);
    cmd.args(args).current_dir(root);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// True while any `lunima-agent-loop` process (scheduled or manual) is running.
pub fn loop_running() -> bool {
    #[cfg(windows)]
    {
        let mut cmd = hidden("tasklist.exe");
        cmd.args(["/FI", "IMAGENAME eq lunima-agent-loop.exe", "/FO", "CSV", "/NH"]);
        run(cmd).map(|out| out.contains("lunima-agent-loop.exe")).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        let mut cmd = hidden("pgrep");
        cmd.args(["-x", "lunima-agent-loop"]);
        run(cmd).is_ok()
    }
}
