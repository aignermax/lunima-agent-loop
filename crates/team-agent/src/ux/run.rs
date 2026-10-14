//! One UX review: build the desktop app from the integration branch, let the tester walk
//! the persona journeys (customer mode: isolated app, exact report folder), then file the
//! findings as issues without screenshots (a second tester call, `--publish-report`).

use super::plan;
use crate::ctx::Ctx;
use crate::shell::{tail, CmdResult};
use chrono::NaiveDate;
use regex::Regex;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use team_core::config::UxReview;

/// Runs a command line (`crate::shell::run_shell_env` in production; scripted in tests).
pub type Exec = Box<dyn Fn(&str, &Path, Duration, &[(&str, &OsStr)]) -> Result<CmdResult, String> + Send>;

const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SKIP_DIRS: &[&str] = &[".git", "bin", "obj", "node_modules", "target"];

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done { report_dir: PathBuf, filed: Vec<u64>, goals: Vec<String>, note: String },
    Locked,
    TimedOut { report_dir: Option<PathBuf> },
}

pub fn quote(s: &str) -> String {
    format!("\"{s}\"")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, keep: &dyn Fn(&Path) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) || keep(&p) {
                walk(&p, out, keep);
            }
        } else if keep(&p) {
            out.push(p);
        }
    }
}

/// A `.gds` in the clone for the GDS-import journey (names with "mzi" first, then shortest path).
pub fn find_sample_gds(clone: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    walk(clone, &mut found, &|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gds")));
    found.sort_by_key(|p| (!p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("mzi")), p.as_os_str().len()));
    found.into_iter().next()
}

/// `<project>/bin/Release/**/<project>.exe` (else `.dll`), newest first.
pub fn find_app(clone: &Path, project: &str) -> Option<PathBuf> {
    let root = clone.join(project).join("bin").join("Release");
    for ext in ["exe", "dll"] {
        let mut found = Vec::new();
        let want = format!("{project}.{ext}").to_lowercase();
        walk(&root, &mut found, &|p| p.is_dir() || p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == want));
        found.retain(|p| p.is_file());
        found.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        if let Some(p) = found.pop() {
            return Some(p);
        }
    }
    None
}

/// `<python> "<tester_dir>/ux_tester.py" <args…>` — all arguments quoted.
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

/// The whole review. `Err` = something to fix (build, missing key, tester crash).
pub fn review(ctx: &Ctx, exec: &Exec, repo: &str, day: NaiveDate) -> Result<Outcome, String> {
    let ux = &ctx.cfg.ux_review;
    let key = api_key(ux)?;
    let setup = crate::workspace::setup(ctx, "ux-review", repo)?;
    let branch = setup.working_branch.clone();
    setup.clone.reset_to_remote(&branch)?;
    let sha = setup.clone.git(&["rev-parse", "HEAD"])?.trim().to_string();
    let dir = &setup.clone.dir;
    let build = exec(&format!("dotnet build {} -c Release", quote(&ux.app_project)), dir, BUILD_TIMEOUT, &[])?;
    if !build.ok() {
        return Err(format!("Build von {} fehlgeschlagen: {}", ux.app_project, tail(&output(&build), 600)));
    }
    let app = find_app(dir, &ux.app_project).ok_or_else(|| format!("{} nach dem Build nicht gefunden", ux.app_project))?;

    let all = plan::substitute(plan::load_goals(&ux.goals_file)?, find_sample_gds(dir).as_deref());
    let goals = plan::pick(&all, day, ux.goals_per_run);
    let work = ctx.paths.state().join("ux");
    let report = work.join("reports").join(chrono::Local::now().format("%Y-%m-%d_%H%M%S").to_string());
    std::fs::create_dir_all(work.join("reports")).map_err(|e| e.to_string())?;
    let (goals_file, identity) = (work.join("goals.json"), work.join("request.json"));
    std::fs::write(&goals_file, serde_json::to_string_pretty(&goals).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::write(&identity, json!({"repo": repo, "sha": sha, "pr": Value::Null, "policy": "ux-review"}).to_string()).map_err(|e| e.to_string())?;

    let env: [(&str, &OsStr); 2] = [("ANTHROPIC_API_KEY", OsStr::new(&key)), ("PYTHONUTF8", OsStr::new("1"))];
    let tester = Path::new(&ux.tester_dir);
    let timeout = Duration::from_secs(ux.timeout_minutes.max(1) * 60);
    let run = exec(&tester_line(ux, &test_args(ux, &app, &goals_file, &identity, &report)), tester, timeout, &env)?;
    let text = output(&run);
    if text.contains("Desktop is locked") || text.contains("No interactive desktop") {
        return Ok(Outcome::Locked);
    }
    let has_report = report.join("report.md").is_file();
    if run.timed_out {
        return Ok(Outcome::TimedOut { report_dir: has_report.then_some(report) });
    }
    if !has_report {
        return Err(format!("Tester ohne Bericht beendet (Exit {:?}): {}", run.exit_code, tail(&text, 600)));
    }
    let published = exec(&tester_line(ux, &publish_args(ux, &report, repo, &branch)), tester, PUBLISH_TIMEOUT, &env)?;
    let filed = filed_issues(&output(&published));
    if !published.ok() {
        crate::log::warn(&format!("UX-Befunde nicht vollständig als Issues angelegt: {}", tail(&output(&published), 300)));
    }
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
        assert!(line.contains("--max-steps \"150\"") && line.contains("--model \"claude-opus-5-5\""), "{line}");
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
    fn finds_sample_gds_and_app() {
        let t = tempfile::tempdir().unwrap();
        let w = |rel: &str| {
            let p = t.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        };
        w("docs/examples/deep/ring.gds");
        w("samples/mzi_lab.gds");
        w("CAP.Desktop/obj/stale.gds");
        w("CAP.Desktop/bin/Release/net10.0/CAP.Desktop.dll");
        w("CAP.Desktop/bin/Release/net10.0/win-arm64/CAP.Desktop.exe");
        assert!(find_sample_gds(t.path()).unwrap().ends_with("mzi_lab.gds"));
        assert!(find_app(t.path(), "CAP.Desktop").unwrap().ends_with("CAP.Desktop.exe"));
        assert!(find_app(t.path(), "Other").is_none());
        assert!(find_sample_gds(&t.path().join("CAP.Desktop")).is_none(), "bin/obj are skipped");
    }
}
