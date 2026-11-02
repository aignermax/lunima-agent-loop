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
    /// killed by the memory guard (`crate::mem`)
    pub memory: Option<String>,
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
        if let Some(m) = self.memory {
            return Err(format!("abgebrochen — {m}"));
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
    crate::mem::apply_lean_env(&mut cmd);
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

/// How a wait ended without the process exiting on its own.
enum Stop {
    Timeout,
    Memory(String),
}

/// How often the memory guard looks while a process runs.
const MEMORY_CHECK: Duration = Duration::from_millis(500);

fn wait_with_timeout(child: &mut Child, timeout: Duration, memory: &dyn Fn() -> Option<String>) -> std::io::Result<Result<ExitStatus, Stop>> {
    let deadline = Instant::now() + timeout;
    let mut checked = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Ok(status));
        }
        let stop = if Instant::now() >= deadline {
            Some(Stop::Timeout)
        } else if checked.elapsed() >= MEMORY_CHECK {
            checked = Instant::now();
            memory().map(Stop::Memory)
        } else {
            None
        };
        if let Some(stop) = stop {
            kill_tree(child);
            let _ = child.wait();
            return Ok(Err(stop));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// All descendants of `pid` (Linux: via the parent pids in /proc). Needed because a child
/// may move into its own process group or session (a shell tool starting `dotnet test`),
/// which a group kill would miss.
#[cfg(target_os = "linux")]
fn descendants(pid: u32) -> Vec<u32> {
    let parents: Vec<(u32, u32)> = std::fs::read_dir("/proc")
        .map(|it| {
            it.flatten()
                .filter_map(|e| {
                    let p: u32 = e.file_name().to_str()?.parse().ok()?;
                    let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
                    // `pid (comm) state ppid …` — comm may contain spaces, so split after ')'
                    let ppid = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
                    Some((p, ppid))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = vec![];
    let mut frontier = vec![pid];
    while let Some(p) = frontier.pop() {
        for &(c, pp) in &parents {
            if pp == p && !out.contains(&c) {
                out.push(c);
                frontier.push(c);
            }
        }
    }
    out
}

/// Kills the process and its children: `taskkill /T` on Windows, the process group
/// (see `command`) elsewhere.
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = command("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    #[cfg(target_os = "linux")]
    {
        // collected first: once the parent is gone, orphans are re-parented and untraceable
        let pids: Vec<String> = descendants(child.id()).iter().map(u32::to_string).collect();
        if !pids.is_empty() {
            let _ = Command::new("kill").arg("-KILL").args(&pids).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
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
pub fn run_with_input(cmd: Command, cwd: Option<&Path>, input: Option<&[u8]>, timeout: Duration) -> std::io::Result<Output> {
    run_guarded(cmd, cwd, input, timeout, &crate::mem::must_kill)
}

fn run_guarded(mut cmd: Command, cwd: Option<&Path>, input: Option<&[u8]>, timeout: Duration, memory: &dyn Fn() -> Option<String>) -> std::io::Result<Output> {
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
    let status = wait_with_timeout(&mut child, timeout, memory)?;
    Ok(Output {
        status: status.as_ref().ok().and_then(|s| s.code()),
        stdout: out.collect(DRAIN_GRACE),
        stderr: err.collect(DRAIN_GRACE),
        timed_out: matches!(status, Err(Stop::Timeout)),
        memory: match status {
            Err(Stop::Memory(m)) => Some(m),
            _ => None,
        },
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
    fn memory_guard_kills_the_whole_tree() {
        // the grandchild leaves the process group, like a tool starting its own session
        let script = if cfg!(windows) { "ping -n 60 127.0.0.1 >NUL" } else { "setsid sleep 61 & echo started; wait" };
        let t = Instant::now();
        let out = run_guarded(shell(script), None, None, Duration::from_secs(60), &|| Some("Arbeitsspeicher knapp: test".into())).unwrap();
        assert_eq!(out.memory.as_deref(), Some("Arbeitsspeicher knapp: test"));
        assert!(!out.timed_out && !out.ok() && t.elapsed() < Duration::from_secs(20));
        assert!(out.into_result().unwrap_err().contains("Arbeitsspeicher knapp"));
        #[cfg(target_os = "linux")]
        {
            let orphans = std::process::Command::new("pgrep").args(["-f", "^sleep 61$"]).output().unwrap();
            assert!(String::from_utf8_lossy(&orphans.stdout).trim().is_empty(), "detached grandchild survived");
        }
    }

    #[test]
    fn lean_build_environment_reaches_children() {
        let script = if cfg!(windows) { "echo %DOTNET_gcServer%" } else { "echo $MSBUILDDISABLENODEREUSE" };
        let out = run(shell(script), None, Duration::from_secs(20)).unwrap();
        assert!(out.stdout.contains(if cfg!(windows) { "0" } else { "1" }));
    }

    #[test]
    fn which_finds_a_system_tool() {
        let tool = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(which(tool).is_some());
        assert!(which("definitely-not-a-tool-xyz").is_none());
    }
}
