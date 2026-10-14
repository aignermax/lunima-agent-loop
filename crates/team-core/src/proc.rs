//! Running external tools (gh, git, claude, build/test commands) — no console windows on
//! Windows, hard timeouts, PATH lookup that understands `.exe`/`.cmd`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Captured result of a finished process.
#[derive(Debug, Clone)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == Some(0) && !self.timed_out
    }

    /// stdout on success, else the most useful error text.
    pub fn into_result(self) -> Result<String, String> {
        if self.ok() {
            return Ok(self.stdout);
        }
        if self.timed_out {
            return Err("Zeitüberschreitung".into());
        }
        let err = self.stderr.trim();
        Err(if err.is_empty() { self.stdout.trim().to_string() } else { err.to_string() })
    }
}

/// A command that never opens a console window (the agent runs without one).
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        // own process group, so `kill_tree` reaches grandchildren too
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Finds a program on PATH; on Windows also with PATHEXT extensions (claude.cmd, gh.exe …).
pub fn which(name: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Reads a pipe in the background. A grandchild may keep the pipe open after the child is
/// gone, so the result is collected with a deadline instead of an unbounded join.
struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl Drain {
    fn start(mut pipe: impl Read + Send + 'static) -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (tx, done) = mpsc::channel();
        let sink = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap_or_else(|p| p.into_inner()).extend_from_slice(&chunk[..n]);
            }
            let _ = tx.send(());
        });
        Self { buf, done }
    }

    fn collect(self, grace: Duration) -> String {
        let _ = self.done.recv_timeout(grace);
        let bytes = std::mem::take(&mut *self.buf.lock().unwrap_or_else(|p| p.into_inner()));
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// How long output may still trickle in after the process ended.
const DRAIN_GRACE: Duration = Duration::from_secs(10);

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            kill_tree(child);
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Kills the process and its children: `taskkill /T` on Windows, the process group
/// (see `command`) elsewhere.
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = command("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
}

/// Runs to completion (stdin closed), capturing stdout/stderr, killed after `timeout`.
pub fn run(cmd: Command, cwd: Option<&Path>, timeout: Duration) -> std::io::Result<Output> {
    run_with_input(cmd, cwd, None, timeout)
}

/// Like `run`, writing `input` to stdin first (e.g. a JSON request body for `gh api --input -`).
pub fn run_with_input(mut cmd: Command, cwd: Option<&Path>, input: Option<&[u8]>, timeout: Duration) -> std::io::Result<Output> {
    use std::io::Write;
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let stdin = if input.is_some() { Stdio::piped() } else { Stdio::null() };
    let mut child = cmd.stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    if let (Some(data), Some(mut pipe)) = (input, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
    }
    let out = Drain::start(child.stdout.take().expect("piped stdout"));
    let err = Drain::start(child.stderr.take().expect("piped stderr"));
    let status = wait_with_timeout(&mut child, timeout)?;
    Ok(Output {
        status: status.and_then(|s| s.code()),
        stdout: out.collect(DRAIN_GRACE),
        stderr: err.collect(DRAIN_GRACE),
        timed_out: status.is_none(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        if cfg!(windows) {
            let mut c = command("cmd");
            c.args(["/C", script]);
            c
        } else {
            let mut c = command("sh");
            c.args(["-c", script]);
            c
        }
    }

    #[test]
    fn captures_output_and_exit_code() {
        let out = run(shell("echo hello"), None, Duration::from_secs(20)).unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hello");
        let bad = run(shell("exit 3"), None, Duration::from_secs(20)).unwrap();
        assert_eq!(bad.status, Some(3));
        assert!(bad.into_result().is_err());
    }

    #[test]
    fn times_out() {
        let script = if cfg!(windows) { "ping -n 30 127.0.0.1 >NUL" } else { "sleep 30" };
        let out = run(shell(script), None, Duration::from_millis(500)).unwrap();
        assert!(out.timed_out && !out.ok());
    }

    #[test]
    fn timeout_is_bounded_even_if_a_grandchild_holds_the_pipes() {
        let script = if cfg!(windows) { "start /B ping -n 60 127.0.0.1 & ping -n 60 127.0.0.1 >NUL" } else { "sleep 60 & sleep 60" };
        let t = Instant::now();
        let out = run(shell(script), None, Duration::from_millis(500)).unwrap();
        assert!(out.timed_out);
        assert!(t.elapsed() < Duration::from_secs(20), "took {:?}", t.elapsed());
    }

    #[test]
    fn which_finds_a_system_tool() {
        let tool = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(which(tool).is_some());
        assert!(which("definitely-not-a-tool-xyz").is_none());
    }
}
