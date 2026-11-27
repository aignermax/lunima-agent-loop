//! The agent binary as the launcher of sessions on the hidden desktop (`team_core::desktop`):
//! what runs through it lands on that desktop and behaves exactly as if started directly.
#![cfg(windows)]

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use team_core::desktop::{ARG, ARG_NAME, ARG_RAW, DESKTOP};

const AGENT: &str = env!("CARGO_BIN_EXE_lunima-team-agent");

fn output(mut cmd: Command, stdin: &str) -> (Option<i32>, String, String) {
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn launched(raw: bool, program: &str) -> Command {
    let mut c = Command::new(AGENT);
    c.arg(if raw { ARG_RAW } else { ARG }).arg(program);
    c
}

#[test]
fn the_program_and_its_children_run_on_the_hidden_desktop() {
    let (code, own, _) = output(
        {
            let mut c = Command::new(AGENT);
            c.arg(ARG_NAME);
            c
        },
        "",
    );
    assert_eq!(code, Some(0));
    if own.trim().ends_with(&format!("\\{DESKTOP}")) {
        // run by an agent session on this repo: the change of desktop cannot be seen from here
        eprintln!("skipped: the test itself runs on the hidden desktop");
        return;
    }
    // the program started through the launcher is itself the agent, asked where it runs
    let mut c = launched(false, AGENT);
    c.arg(ARG_NAME);
    let (code, name, err) = output(c, "");
    assert_eq!(code, Some(0), "{err}");
    assert!(name.trim().ends_with(&format!("\\{DESKTOP}")), "not on the hidden desktop: {name} {err}");
    assert_eq!(name.trim().split('\\').next(), own.trim().split('\\').next(), "same window station as the user");
}

#[test]
fn raw_command_lines_stdin_stdout_stderr_and_exit_code_pass_through() {
    let mut c = launched(true, "cmd");
    c.raw_arg("/S /C \"findstr x& echo err 1>&2& exit 7\"");
    let (code, out, err) = output(c, "x1\r\ny2\r\n");
    assert_eq!(code, Some(7));
    assert_eq!(out.trim(), "x1");
    assert_eq!(err.trim(), "err");
}

#[test]
fn a_cmd_cli_gets_the_same_arguments_as_when_started_directly() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("show args.cmd");
    std::fs::write(&script, "@echo off\r\necho [%*]\r\n").unwrap();
    // the shapes the Codex runner passes: `key="value"` settings, spaces, a lone `-`
    let args = ["-c", "model_provider=\"lunima_api\"", "-c", "windows.sandbox=\"unelevated\"", "two words", "-"];
    let mut direct = Command::new(&script);
    direct.args(args);
    let mut wrapped = launched(false, script.to_str().unwrap());
    wrapped.args(args);
    let (d, w) = (output(direct, ""), output(wrapped, ""));
    assert_eq!(d.0, Some(0), "{d:?}");
    assert_eq!(w, d);
}

#[test]
fn a_kill_after_timeout_reaches_the_program_behind_the_launcher() {
    let mut c = team_core::proc::command(AGENT);
    c.args([ARG, "ping", "-n", "60", "127.0.0.1"]);
    let t = Instant::now();
    let out = team_core::proc::run(c, None, Duration::from_millis(1500)).unwrap();
    assert!(out.timed_out);
    // a surviving ping would hold the pipes open for the 10 s drain grace
    assert!(t.elapsed() < Duration::from_secs(8), "took {:?}", t.elapsed());
}
