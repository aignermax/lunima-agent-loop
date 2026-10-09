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
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() { stdout.trim().to_string() } else { stderr })
    }
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
