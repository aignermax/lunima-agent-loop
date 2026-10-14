//! UX reviewer end to end: scripted tester/build commands, simulated GitHub, real git.

use super::*;
use crate::fake::{self, FakeForge, FakeLlm};
use crate::forge::Forge;
use crate::shell::CmdResult;
use chrono::TimeZone;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct World {
    tmp: tempfile::TempDir,
    origin: PathBuf,
    forge: Arc<FakeForge>,
    lines: Arc<Mutex<Vec<String>>>,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let origin = fake::origin(tmp.path(), &[("mzi_sample.gds", "gds")]);
    World { tmp, origin, forge: Arc::new(FakeForge::new("o/r")), lines: Arc::new(Mutex::new(vec![])) }
}

fn night() -> DateTime<Local> {
    Local.with_ymd_and_hms(2026, 10, 15, 2, 30, 0).unwrap()
}

fn ok(stdout: &str) -> CmdResult {
    CmdResult { exit_code: Some(0), stdout: stdout.into(), stderr: String::new(), timed_out: false }
}

/// The value after `flag "…"` in a command line.
fn arg(line: &str, flag: &str) -> PathBuf {
    let rest = &line[line.find(flag).unwrap() + flag.len() + 2..];
    PathBuf::from(&rest[..rest.find('"').unwrap()])
}

impl World {
    fn ctx(&self, machine: &str) -> Ctx {
        let dir = self.tmp.path().join(machine);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = fake::ctx(&dir, &self.origin, self.forge.clone(), Arc::new(FakeLlm::new()));
        ctx.machine = machine.into();
        let key = dir.join("key.txt");
        std::fs::write(&key, "sk-test-secret\n").unwrap();
        let ux = &mut ctx.cfg.ux_review;
        ux.repo = "o/r".into();
        ux.tester_dir = dir.join("tester").display().to_string();
        ux.api_key_file = key.display().to_string();
        ctx
    }

    /// Scripted commands: the build creates the app, the tester writes a report (or says
    /// the desktop is locked), publishing files issue 77.
    fn reviewer(&self, locked: bool) -> UxReviewer {
        let lines = self.lines.clone();
        let exec: run::Exec = Box::new(move |line: &str, cwd: &Path, _t, env: &[(&str, &std::ffi::OsStr)]| {
            lines.lock().unwrap().push(line.to_string());
            if line.starts_with("dotnet build") {
                let app = cwd.join("CAP.Desktop").join("bin").join("Release").join("net10.0").join("CAP.Desktop.exe");
                std::fs::create_dir_all(app.parent().unwrap()).unwrap();
                std::fs::write(app, "exe").unwrap();
                return Ok(ok("Build succeeded"));
            }
            assert!(env.iter().any(|(k, v)| *k == "ANTHROPIC_API_KEY" && *v == "sk-test-secret"), "key only via env");
            if line.contains("--customer-goals") {
                if locked {
                    return Ok(CmdResult { exit_code: Some(1), stdout: String::new(), stderr: "RuntimeError: Desktop is locked; customer review is blocked".into(), timed_out: false });
                }
                let report = arg(line, "--exact-report-dir");
                std::fs::create_dir_all(&report).unwrap();
                std::fs::write(report.join("report.md"), "# Lunima UX test\n\nNewcomer could not find how to start.").unwrap();
                return Ok(ok("Done: 40 actions, 2 findings, 0 hangs."));
            }
            Ok(ok("  filed https://github.com/o/r/issues/77  [major] Start is unclear"))
        });
        UxReviewer::with(exec, Box::new(night))
    }
}

#[test]
fn nightly_review_builds_walks_journeys_and_files_issues() {
    let w = world();
    let ctx = w.ctx("A");
    let mut ux = w.reviewer(false);
    assert_eq!(ux.cycle(&ctx), Ok(true));
    let st = UxState::load(&ctx.paths.state()).unwrap();
    assert_eq!((st.last_result.as_str(), st.day.as_str()), ("ok", "2026-10-15"));
    assert_eq!(st.filed_issues, vec![77]);
    assert_eq!(st.goals.len(), 3);
    let lines = w.lines.lock().unwrap().clone();
    assert_eq!(lines.len(), 3, "{lines:#?}");
    assert!(lines[1].contains("--customer-goals") && lines[1].contains("CAP.Desktop.exe"));
    assert!(lines[2].contains("--publish-report") && lines[2].contains("--no-screenshots"));
    assert!(lines.iter().all(|l| !l.contains("sk-test-secret")), "the key never appears in a command line");
    let goals = std::fs::read_to_string(ctx.paths.state().join("ux").join("goals.json")).unwrap();
    assert!(!goals.contains("{SAMPLE_GDS}"));
    assert_eq!(ux.cycle(&ctx), Ok(false), "once per night");
    // the PO sees the report
    let section = po_section(&ctx);
    assert!(section.contains("Newcomer could not find how to start") && section.contains("agent-task"), "{section}");
}

#[test]
fn locked_desktop_is_retried_later_in_the_window() {
    let w = world();
    let ctx = w.ctx("A");
    assert_eq!(w.reviewer(true).cycle(&ctx), Ok(false));
    let st = UxState::load(&ctx.paths.state()).unwrap();
    assert_eq!(st.last_result, "skipped-locked");
    assert!(plan::due(&st, night() + Duration::minutes(31), ctx.cfg.ux_review.hour));
    let mut later = w.reviewer(false);
    later.clock = Box::new(|| night() + Duration::minutes(31));
    assert_eq!(later.cycle(&ctx), Ok(true), "own day lock does not block the retry");
}

#[test]
fn only_one_machine_reviews_per_night() {
    let w = world();
    assert_eq!(w.reviewer(false).cycle(&w.ctx("A")), Ok(true));
    let b = w.ctx("B");
    assert_eq!(w.reviewer(false).cycle(&b), Ok(false));
    let st = UxState::load(&b.paths.state()).unwrap();
    assert_eq!(st.last_result, "skipped");
    assert!(w.forge.lock_info("ux-20261015").unwrap().is_some(), "the day's lock stays as proof");
}

#[test]
fn not_set_up_or_outside_the_window_does_nothing() {
    let w = world();
    let mut ctx = w.ctx("A");
    let mut day = w.reviewer(false);
    day.clock = Box::new(|| Local.with_ymd_and_hms(2026, 10, 15, 14, 0, 0).unwrap());
    assert_eq!(day.cycle(&ctx), Ok(false));
    ctx.cfg.ux_review.api_key_file.clear();
    assert_eq!(w.reviewer(false).cycle(&ctx), Ok(false));
    assert!(w.lines.lock().unwrap().is_empty());
}

#[test]
fn po_prompt_says_when_there_is_no_report() {
    let w = world();
    assert!(po_section(&w.ctx("A")).contains("No recent UX report"));
}
