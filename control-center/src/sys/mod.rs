//! Everything that talks to the outside world: processes, scheduled task, git, gh, registry.

pub mod autostart;
pub mod cli;
pub mod git;
pub mod github;
pub mod schedtask;
pub mod wsl;

use std::path::Path;
use std::process::{Command, Output};

/// A command that never flashes a console window (this app runs without one).
pub fn hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs to completion; Err carries stderr (or the spawn error) for display.
pub fn run(mut cmd: Command) -> Result<String, String> {
    let out: Output = cmd.output().map_err(|e| e.to_string())?;
    result_of(&out)
}

fn result_of(out: &Output) -> Result<String, String> {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() { stdout.trim().to_string() } else { stderr })
    }
}

/// Like `run`, but kills the process after `timeout` (a hanging wsl.exe must not freeze
/// the collector). Returns the raw output so callers can read stdout of non-zero exits.
pub fn run_output_timeout(mut cmd: Command, timeout: std::time::Duration) -> Result<Output, String> {
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => return child.wait_with_output().map_err(|e| e.to_string()),
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Zeitüberschreitung nach {} s", timeout.as_secs()));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

/// `run` with a timeout.
pub fn run_timeout(cmd: Command, timeout: std::time::Duration) -> Result<String, String> {
    result_of(&run_output_timeout(cmd, timeout)?)
}

/// Opens a URL or folder with the system handler.
pub fn open(target: &str) {
    #[cfg(windows)]
    let _ = hidden("explorer.exe").arg(target).spawn();
    #[cfg(not(windows))]
    let _ = hidden(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }).arg(target).spawn();
}

pub fn open_path(path: &Path) {
    open(&path.display().to_string());
}

/// Runs a PowerShell snippet (no profile) and returns its stdout.
pub fn powershell(script: &str) -> Result<String, String> {
    let mut cmd = hidden("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    run(cmd)
}
