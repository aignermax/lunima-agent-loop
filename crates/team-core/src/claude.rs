//! Runs one headless Claude Code session:
//! `claude -p --output-format stream-json --verbose --max-turns N --dangerously-skip-permissions
//!  [--model M] [--settings F]`, prompt on stdin (no command-line length limits, no `.cmd`
//! escaping). stdout and stderr are read concurrently (the Python agent read them only at
//! exit and could stall). Every event counts as activity; a silent session is killed with
//! its whole process tree. Usage limits of the subscription are detected with reset time.

use crate::proc::{command, kill_tree, which};
use crate::provider::ChildEnv;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod parse;
pub use parse::{parse_events, usage_limit_reset, Outcome, Usage};

/// The commit-comment hook shipped with the agent (passed via `--settings`).
pub const HOOK_SETTINGS: &str = include_str!("../assets/claude-settings.json");

pub struct Session<'a> {
    pub prompt: String,
    pub cwd: PathBuf,
    /// "" = CLI default
    pub model: String,
    pub max_turns: u32,
    pub env: ChildEnv,
    pub settings_file: Option<PathBuf>,
    /// raw stream-json is appended here (the Control Center shows these logs)
    pub log_file: Option<PathBuf>,
    pub inactivity: Duration,
    /// hard limit for the whole session (`None` = only the inactivity watchdog)
    pub deadline: Option<Duration>,
    /// called about once a minute while the session runs (heartbeat)
    pub on_tick: Option<&'a (dyn Fn() + Sync)>,
}

#[derive(Debug)]
pub enum Error {
    NotInstalled,
    Spawn(String),
    /// no output for this many seconds — killed
    Stuck(u64),
    /// exited without a result event
    Crashed { code: Option<i32>, stderr_tail: String },
    /// ran longer than the session's deadline (seconds) — killed
    TimedOut(u64),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled => write!(f, "Claude-CLI nicht gefunden (claude nicht im PATH)"),
            Error::Spawn(e) => write!(f, "Claude konnte nicht gestartet werden: {e}"),
            Error::Stuck(s) => write!(f, "Claude hing ({s} s ohne Ausgabe) und wurde beendet"),
            Error::Crashed { code, stderr_tail } => write!(f, "Claude beendet ohne Ergebnis (Exit {code:?}): {stderr_tail}"),
            Error::TimedOut(s) => write!(f, "{TIMED_OUT_TEXT} ({s} s) — Claude wurde beendet"),
        }
    }
}

/// The CLI to run: `CLAUDE_CLI_PATH`, else `claude` on PATH (prefers claude.exe over .cmd).
pub fn find_cli() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CLI_PATH").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| which("claude"))
}

pub fn args(s: &Session) -> Vec<String> {
    let mut a: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--dangerously-skip-permissions", "--max-turns"].iter().map(|x| x.to_string()).collect();
    a.push(s.max_turns.to_string());
    if !s.model.is_empty() {
        a.extend(["--model".to_string(), s.model.clone()]);
    }
    if let Some(f) = &s.settings_file {
        a.extend(["--settings".to_string(), f.display().to_string()]);
    }
    a
}

enum Line {
    Out(String),
    Done,
}

/// After the `result` event the CLI should exit; past this it is killed and the result used.
const RESULT_GRACE: Duration = Duration::from_secs(60);

fn is_result_event(line: &str) -> bool {
    line.contains("\"result\"") && serde_json::from_str::<serde_json::Value>(line).ok().and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|t| t == "result")).unwrap_or(false)
}

/// Start of `Error::TimedOut`'s message (callers classify failures by it).
pub const TIMED_OUT_TEXT: &str = "Zeitlimit der Sitzung überschritten";

/// `true` once a session started at `started` has used up its `deadline`.
pub fn over_deadline(started: Instant, deadline: Option<Duration>) -> bool {
    deadline.is_some_and(|d| started.elapsed() >= d)
}

pub fn run(s: &Session) -> Result<Outcome, Error> {
    let cli = find_cli().ok_or(Error::NotInstalled)?;
    let mut cmd = command(&cli);
    cmd.args(args(s)).current_dir(&s.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    s.env.apply(&mut cmd);
    cmd.env("GIT_TERMINAL_PROMPT", "0").env_remove("CLAUDE_CODE_ENTRYPOINT");
    // the subscription login would hand the agent the user's claude.ai connectors (mail,
    // drive …) — an autonomous agent gets none of them; a repo's own .mcp.json still works
    cmd.env("ENABLE_CLAUDEAI_MCP_SERVERS", "false");
    let mut child = cmd.spawn().map_err(|e| Error::Spawn(e.to_string()))?;

    let mut stdin = child.stdin.take().expect("piped stdin");
    let prompt = s.prompt.clone();
    std::thread::spawn(move || {
        let _ = stdin.write_all(prompt.as_bytes());
    });

    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().expect("piped stdout");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(Line::Out(line)).is_err() {
                return;
            }
        }
        let _ = tx.send(Line::Done);
    });
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf).into_owned();
        text.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect::<String>()
    });

    let mut log = s.log_file.as_ref().and_then(|p| {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
    });
    let mut lines = Vec::new();
    let started = Instant::now();
    let mut result_at: Option<Instant> = None;
    let mut last_activity = Instant::now();
    let mut last_tick = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Line::Out(l)) => {
                if let Some(f) = log.as_mut() {
                    let _ = writeln!(f, "{l}");
                }
                if result_at.is_none() && is_result_event(&l) {
                    result_at = Some(Instant::now());
                }
                lines.push(l);
                last_activity = Instant::now();
            }
            Ok(Line::Done) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_tick.elapsed() >= Duration::from_secs(60) {
            last_tick = Instant::now();
            if let Some(tick) = s.on_tick {
                tick();
            }
        }
        // the final result is in, but something (MCP server, hook child) keeps the CLI alive
        if result_at.is_some_and(|t| t.elapsed() >= RESULT_GRACE) {
            kill_tree(&mut child);
            break;
        }
        if last_activity.elapsed() >= s.inactivity {
            kill_tree(&mut child);
            let _ = child.wait();
            return Err(Error::Stuck(s.inactivity.as_secs()));
        }
        if result_at.is_none() && over_deadline(started, s.deadline) {
            kill_tree(&mut child);
            let _ = child.wait();
            return Err(Error::TimedOut(s.deadline.map(|d| d.as_secs()).unwrap_or(0)));
        }
    }
    let status = child.wait().map_err(|e| Error::Spawn(e.to_string()))?;
    let stderr_tail = stderr_reader.join().unwrap_or_default();
    match parse_events(&lines, s.max_turns) {
        Some(outcome) => Ok(outcome),
        None => Err(Error::Crashed { code: status.code(), stderr_tail }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_is_optional_and_measured_from_start() {
        let start = Instant::now() - Duration::from_secs(120);
        assert!(!over_deadline(start, None));
        assert!(!over_deadline(start, Some(Duration::from_secs(600))));
        assert!(over_deadline(start, Some(Duration::from_secs(60))));
        assert!(Error::TimedOut(60).to_string().starts_with(TIMED_OUT_TEXT));
    }

    #[test]
    fn recognises_the_result_event() {
        assert!(is_result_event(r#"{"type":"result","subtype":"success","result":"x"}"#));
        assert!(!is_result_event(r#"{"type":"assistant","message":{"content":"result"}}"#));
    }

    #[test]
    fn command_line_is_headless_stream_json() {
        let s = Session {
            prompt: "x".into(),
            cwd: ".".into(),
            model: "claude-opus-5-5".into(),
            max_turns: 150,
            env: ChildEnv::subscription(),
            settings_file: Some("hooks.json".into()),
            log_file: None,
            inactivity: Duration::from_secs(1800),
            deadline: None,
            on_tick: None,
        };
        let a = args(&s);
        assert_eq!(&a[..4], ["-p", "--output-format", "stream-json", "--verbose"]);
        assert!(a.windows(2).any(|w| w == ["--max-turns", "150"]));
        assert!(a.windows(2).any(|w| w == ["--model", "claude-opus-5-5"]));
        assert!(a.windows(2).any(|w| w == ["--settings", "hooks.json"]));
        assert!(!a.iter().any(|x| x == "x"), "prompt goes to stdin, not argv");
    }

    #[test]
    fn hook_settings_are_valid_json_with_hooks() {
        let v: serde_json::Value = serde_json::from_str(HOOK_SETTINGS).unwrap();
        assert!(v["hooks"]["PostToolUse"].is_array());
    }
}

/// Real session against the installed CLI on the subscription login.
/// Run explicitly: `cargo test -p team-core live_subscription_session -- --ignored --nocapture`
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn live_subscription_session() {
        let tmp = std::env::temp_dir();
        let s = Session {
            prompt: "Reply with exactly: PONG".into(),
            cwd: tmp,
            model: "claude-opus-5-5".into(),
            max_turns: 2,
            env: ChildEnv::subscription(),
            settings_file: None,
            log_file: None,
            inactivity: Duration::from_secs(120),
            deadline: None,
            on_tick: None,
        };
        let o = run(&s).expect("session");
        println!("text={:?} turns={} tokens={} cost={} subtype={}", o.text, o.num_turns, o.usage.total(), o.cost_usd, o.subtype);
        assert!(o.text.contains("PONG"), "{o:?}");
        assert!(!o.is_error);
    }
}
