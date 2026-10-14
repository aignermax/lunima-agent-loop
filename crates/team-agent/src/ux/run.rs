//! One UX review: probe the desktop, build the app from the integration branch (only when
//! the commit changed), let the tester walk the persona journeys (customer mode: isolated
//! app, exact report folder), and — only for a complete run — file the findings as issues
//! without screenshots and publish the report for the team.

use super::{files, plan, report};
use crate::ctx::Ctx;
use crate::shell::{tail, CmdResult};
use chrono::{Local, NaiveDate};
use regex::Regex;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use team_core::config::UxReview;
use team_core::state::UxState;

/// Runs a command line (`crate::shell::run_shell_env` in production; scripted in tests).
pub type Exec = Box<dyn Fn(&str, &Path, Duration, &[(&str, &OsStr)]) -> Result<CmdResult, String> + Send>;

const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The tester's own check (OpenInputDesktop), run before anything expensive.
const PROBE: &str = "import customer_desktop; customer_desktop.require_unlocked_desktop()";

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done { report_dir: PathBuf, filed: Vec<u64>, goals: Vec<String>, note: String },
    Locked,
    TimedOut { report_dir: Option<PathBuf> },
}

pub fn quote(s: &str) -> String {
    format!("\"{s}\"")
}

/// `<python> "<tester_dir>/ux_tester.py" <args…>` — values quoted.
pub fn tester_line(ux: &UxReview, args: &[String]) -> String {
    let script = Path::new(&ux.tester_dir).join("ux_tester.py");
    let mut line = format!("{} {}", ux.python.trim(), quote(&script.display().to_string()));
    for a in args {
        line.push(' ');
        line.push_str(&if a.starts_with("--") { a.clone() } else { quote(a) });
    }
    line
}

pub fn test_args(ux: &UxReview, app: &Path, goals: &Path, identity: &Path, report: &Path) -> Vec<String> {
    let p = |x: &Path| x.display().to_string();
    vec!["--app".into(), p(app), "--customer-goals".into(), p(goals), "--customer-identity".into(), p(identity), "--exact-report-dir".into(), p(report), "--max-steps".into(), ux.max_steps.to_string(), "--model".into(), ux.model.clone()]
}

pub fn publish_args(ux: &UxReview, report: &Path, repo: &str, base: &str) -> Vec<String> {
    vec!["--publish-report".into(), report.display().to_string(), "--no-screenshots".into(), "--min-severity".into(), ux.min_severity.clone(), "--repo".into(), repo.into(), "--base-branch".into(), base.into()]
}

/// Issue numbers from the tester's `filed https://github.com/o/r/issues/N` lines.
pub fn filed_issues(output: &str) -> Vec<u64> {
    let re = Regex::new(r"filed https://github\.com/[^/\s]+/[^/\s]+/issues/(\d+)").expect("regex");
    re.captures_iter(output).filter_map(|c| c[1].parse().ok()).collect()
}

fn api_key(ux: &UxReview) -> Result<String, String> {
    let key = std::fs::read_to_string(&ux.api_key_file).map_err(|e| format!("API-Schlüsseldatei nicht lesbar: {e}"))?;
    Some(key.trim().to_string()).filter(|k| !k.is_empty()).ok_or_else(|| "API-Schlüsseldatei ist leer".into())
}

fn output(r: &CmdResult) -> String {
    format!("{}\n{}", r.stdout, r.stderr)
}

fn says_locked(text: &str) -> bool {
    text.contains("Desktop is locked") || text.contains("No interactive desktop")
}

/// Why a finished tester run is not usable (exit code, interrupted or partial session).
pub fn incomplete(run: &CmdResult, report: &Path) -> Option<String> {
    let data: Value = std::fs::read_to_string(report.join("findings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    if run.exit_code != Some(0) {
        return Some(format!("Tester beendet mit Exit {:?}", run.exit_code));
    }
    if data.is_null() || !report.join("report.md").is_file() {
        return Some("kein vollständiger Bericht (report.md/findings.json fehlen)".into());
    }
    if data.get("partial").and_then(Value::as_bool) != Some(false) || data.get("interrupted").and_then(Value::as_bool) != Some(false) {
        return Some("Tester-Sitzung unvollständig oder unterbrochen".into());
    }
    None
}

/// Persona journeys for tonight, with the product context on the first one.
fn goals_for(ux: &UxReview, clone: &Path, day: NaiveDate) -> Result<Vec<Value>, String> {
    let sample = files::find_sample_gds(clone).and_then(|p| files::stage_sample(&p));
    let mut goals = plan::pick(&plan::substitute(plan::load_goals(&ux.goals_file)?, sample.as_deref()), day, ux.goals_per_run);
    if let (Some(first), Some(context)) = (goals.first_mut(), files::product_context(clone)) {
        first["customer_context"] = Value::String(context);
    }
    Ok(goals)
}

/// The whole review. `Err` = something to fix (build, missing key, broken tester run).
pub fn review(ctx: &Ctx, exec: &Exec, repo: &str, day: NaiveDate, st: &mut UxState) -> Result<Outcome, String> {
    let ux = &ctx.cfg.ux_review;
    let key = api_key(ux)?;
    let tester = Path::new(&ux.tester_dir);
    if cfg!(windows) {
        let probe = exec(&format!("{} -c {}", ux.python.trim(), quote(PROBE)), tester, PROBE_TIMEOUT, &[])?;
        if !probe.ok() && says_locked(&output(&probe)) {
            return Ok(Outcome::Locked);
        }
    }
    let setup = crate::workspace::setup(ctx, "ux-review", repo)?;
    let branch = setup.working_branch.clone();
    let dir = &setup.clone.dir;
    let sha = setup.clone.git(&["rev-parse", &format!("origin/{branch}")])?.trim().to_string();
    // unchanged commit + app still built: keep the clone as is (a reset would `clean -x` the build)
    let current = setup.clone.git(&["rev-parse", "HEAD"]).map(|h| h.trim() == sha).unwrap_or(false);
    if !(current && st.built_sha == sha && files::find_app(dir, &ux.app_project).is_some()) {
        setup.clone.reset_to_remote(&branch)?;
        let build = exec(&format!("dotnet build {} -c Release", quote(&ux.app_project)), dir, BUILD_TIMEOUT, &[])?;
        if !build.ok() {
            return Err(format!("Build von {} fehlgeschlagen: {}", ux.app_project, tail(&output(&build), 600)));
        }
        st.built_sha = sha.clone();
    }
    let app = files::find_app(dir, &ux.app_project).ok_or_else(|| format!("{} nach dem Build nicht gefunden", ux.app_project))?;

    let goals = goals_for(ux, dir, day)?;
    let work = ctx.paths.state().join("ux");
    let reports = work.join("reports");
    let report = reports.join(Local::now().format("%Y-%m-%d_%H%M%S").to_string());
    std::fs::create_dir_all(&reports).map_err(|e| e.to_string())?;
    let (goals_file, identity) = (work.join("goals.json"), work.join("request.json"));
    std::fs::write(&goals_file, serde_json::to_string_pretty(&goals).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::write(&identity, json!({"repo": repo, "sha": sha, "pr": Value::Null, "policy": "ux-review"}).to_string()).map_err(|e| e.to_string())?;

    let utf8 = ("PYTHONUTF8", OsStr::new("1"));
    let timeout = Duration::from_secs(ux.timeout_minutes.max(1) * 60);
    let run = exec(&tester_line(ux, &test_args(ux, &app, &goals_file, &identity, &report)), tester, timeout, &[("ANTHROPIC_API_KEY", OsStr::new(&key)), utf8])?;
    if says_locked(&output(&run)) {
        return Ok(Outcome::Locked);
    }
    if run.timed_out {
        return Ok(Outcome::TimedOut { report_dir: report.join("report.md").is_file().then_some(report) });
    }
    if let Some(why) = incomplete(&run, &report) {
        return Err(format!("{why}: {}", tail(&output(&run), 400)));
    }
    // filing issues needs only gh — no API key
    let published = exec(&tester_line(ux, &publish_args(ux, &report, repo, &branch)), tester, PUBLISH_TIMEOUT, &[utf8])?;
    let filed = filed_issues(&output(&published));
    if !published.ok() {
        crate::log::warn(&format!("UX-Befunde nicht vollständig als Issues angelegt: {}", tail(&output(&published), 300)));
    }
    let text = std::fs::read_to_string(report.join("report.md")).unwrap_or_default();
    if let Err(e) = report::publish((ctx.forge)(repo).as_ref(), &text, Local::now()) {
        crate::log::warn(&format!("UX-Bericht nicht auf GitHub veröffentlicht: {e}"));
    }
    files::retain(&reports, &report);
    let ids = goals.iter().filter_map(|g| g["id"].as_str().map(String::from)).collect::<Vec<_>>();
    let note = format!("{} Abläufe ({}), {} Issues angelegt", ids.len(), ids.join(", "), filed.len());
    Ok(Outcome::Done { report_dir: report, filed, goals: ids, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_quote_paths_and_never_carry_the_key() {
        let ux = UxReview { tester_dir: "C:\\a b\\ux-tester".into(), python: "py -3".into(), api_key_file: "C:\\k\\secret.txt".into(), ..Default::default() };
        let line = tester_line(&ux, &test_args(&ux, Path::new("C:\\app\\CAP.Desktop.exe"), Path::new("g.json"), Path::new("i.json"), Path::new("C:\\r 1")));
        assert!(line.starts_with("py -3 \"C:\\a b\\ux-tester"), "{line}");
        assert!(line.contains("--customer-goals \"g.json\"") && line.contains("--exact-report-dir \"C:\\r 1\""), "{line}");
        assert!(!line.contains("secret") && !line.to_lowercase().contains("api_key"), "{line}");
        let p = tester_line(&ux, &publish_args(&ux, Path::new("C:\\r 1"), "o/r", "dev"));
        assert!(p.contains("--publish-report \"C:\\r 1\" --no-screenshots --min-severity \"major\" --repo \"o/r\" --base-branch \"dev\""), "{p}");
    }

    #[test]
    fn filed_issue_numbers_are_parsed() {
        let out = "  filed https://github.com/aignermax/Lunima/issues/1601  [major] Confusing\n  filed https://github.com/aignermax/Lunima/issues/1602  [critical] Hang\n  FAILED to file 'x'";
        assert_eq!(filed_issues(out), vec![1601, 1602]);
    }

    #[test]
    fn only_a_complete_session_counts() {
        let t = tempfile::tempdir().unwrap();
        let ok = CmdResult { exit_code: Some(0), stdout: String::new(), stderr: String::new(), timed_out: false };
        assert!(incomplete(&ok, t.path()).is_some(), "no report");
        std::fs::write(t.path().join("report.md"), "# r").unwrap();
        std::fs::write(t.path().join("findings.json"), r#"{"partial": false, "interrupted": true}"#).unwrap();
        assert!(incomplete(&ok, t.path()).unwrap().contains("unterbrochen"));
        std::fs::write(t.path().join("findings.json"), r#"{"partial": false, "interrupted": false}"#).unwrap();
        assert!(incomplete(&ok, t.path()).is_none());
        let failed = CmdResult { exit_code: Some(1), ..ok };
        assert!(incomplete(&failed, t.path()).unwrap().contains("Exit"));
    }
}
