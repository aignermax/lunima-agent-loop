//! A CodeGraph index for each coder worktree. Sessions get the codegraph tools from the
//! user's Claude setup, but without an index they answer "not initialized" and the agent
//! reads code with grep/sed through the shell — one model round trip, and a re-read of the
//! whole context, per lookup.

use std::path::Path;
use std::time::Duration;
use team_core::git::run_git;
use team_core::proc::{command, run, which};

/// Indexing a large repository takes a minute or two; past this the session starts without.
const INDEX_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What `codegraph init` writes into the project; never part of the agent's commits.
const ARTIFACTS: [&str; 2] = ["/.codegraph/", "/.cursor/rules/codegraph.mdc"];

/// Indexes `wt` unless it already has an index or the CLI is missing. Best effort: a
/// failure is logged and the session runs without the index.
pub fn ensure(wt: &Path) {
    // also for an index made some other way (a session may run `codegraph init` itself):
    // its config files must never reach a commit
    if let Err(e) = exclude_artifacts(wt) {
        crate::log::warn(&format!("CodeGraph-Index übersprungen ({e})"));
        return;
    }
    if wt.join(".codegraph").is_dir() {
        return;
    }
    let Some(cli) = which("codegraph") else { return };
    let mut cmd = command(&cli);
    cmd.args(["init", "-i"]).arg(wt);
    let failure = match run(cmd, Some(wt), INDEX_TIMEOUT) {
        Ok(out) if out.ok() => {
            crate::log::info(&format!("CodeGraph-Index für {} erstellt", wt.display()));
            return;
        }
        Ok(out) if out.timed_out => format!("nach {} s abgebrochen", INDEX_TIMEOUT.as_secs()),
        Ok(out) => format!("fehlgeschlagen: {}", out.stderr.trim().chars().take(300).collect::<String>()),
        Err(e) => format!("nicht gestartet: {e}"),
    };
    crate::log::warn(&format!("CodeGraph-Index {failure}"));
    // a half-built index would block the next attempt (the check above) and answer wrongly
    let _ = std::fs::remove_dir_all(wt.join(".codegraph"));
}

/// Adds the index files to the repository's own `info/exclude` (shared by all worktrees,
/// unlike a `.gitignore` edit it never shows up in a diff).
fn exclude_artifacts(wt: &Path) -> Result<(), String> {
    let common = run_git(Some(wt), &["rev-parse", "--git-common-dir"], false)?;
    let common = Path::new(common.trim());
    let common = if common.is_absolute() { common.to_path_buf() } else { wt.join(common) };
    let file = common.join("info").join("exclude");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let missing: Vec<&str> = ARTIFACTS.iter().copied().filter(|a| !text.lines().any(|l| l.trim() == *a)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut add = String::new();
    if !text.is_empty() && !text.ends_with('\n') {
        add.push('\n');
    }
    for a in missing {
        add.push_str(a);
        add.push('\n');
    }
    std::fs::write(&file, format!("{text}{add}")).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_files_stay_out_of_the_agents_commits() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        run_git(Some(repo), &["init", "-q"], false).unwrap();
        exclude_artifacts(repo).unwrap();
        exclude_artifacts(repo).unwrap();
        let exclude = std::fs::read_to_string(repo.join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(exclude.matches("/.codegraph/").count(), 1, "added once");
        std::fs::create_dir_all(repo.join(".codegraph")).unwrap();
        std::fs::write(repo.join(".codegraph").join("codegraph.db"), "x").unwrap();
        std::fs::create_dir_all(repo.join(".cursor").join("rules")).unwrap();
        std::fs::write(repo.join(".cursor").join("rules").join("codegraph.mdc"), "x").unwrap();
        std::fs::write(repo.join(".cursor").join("rules").join("team.mdc"), "x").unwrap();
        let status = run_git(Some(repo), &["status", "--porcelain", "--untracked-files=all"], false).unwrap();
        assert!(!status.contains("codegraph"), "{status}");
        assert!(status.contains(".cursor/rules/team.mdc"), "the repo's own cursor rules stay visible: {status}");
    }
}
