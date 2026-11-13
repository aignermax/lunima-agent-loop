//! `~/akhetonics-workspace`: read-only clones of akhetonics-desktop's dependency repos, so
//! the coder can search NuGet-consumed sources locally (`coder::prompt::workspace_note`).
//! Kept at the tip of `dev` (else the default branch), at most once an hour.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use team_core::git::{remote_url, run_git, RepoClone};

pub const DIR: &str = "akhetonics-workspace";

/// The repo whose sessions get the workspace.
pub const CONSUMER: &str = "Akhetonics/akhetonics-desktop";

pub const REPOS: &[&str] = &[
    "Akhetonics/akhetonics-desktop",
    "Akhetonics/SAPPHIRE-Compiler",
    "Akhetonics/raycore-isa",
    "Akhetonics/raycore-assembler",
    "Akhetonics/phridge-blades-simulator",
    "Akhetonics/Phridge-Dispatcher",
    "Akhetonics/raycore-vulkan-icd",
    "Akhetonics/raycore-vulkan-layer",
    "aignermax/Lunima",
];

const EVERY: Duration = Duration::from_secs(3600);
static LAST: Mutex<Option<Instant>> = Mutex::new(None);

/// Refreshes the workspace under `home` unless that happened within the last hour. A repo
/// that fails keeps its previous state.
pub fn refresh_if_due(home: &Path) {
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    let ws = home.join(DIR);
    for repo in REPOS {
        if let Err(e) = sync(&ws, repo, &remote_url(repo)) {
            crate::log::warn(&format!("Workspace {repo}: {e} — bleibt wie er ist"));
        }
    }
}

/// Clones if missing, then moves to the tip of `dev` (else the default branch). Local edits
/// are dropped (nobody works in these clones); ignored files such as search indexes stay.
pub fn sync(ws: &Path, repo: &str, url: &str) -> Result<(), String> {
    let dir = ws.join(repo.rsplit('/').next().unwrap_or(repo));
    if dir.join(".git").exists() {
        // clones made by the old WSL scripts carry a token in the URL; the gh login is used now
        run_git(Some(&dir), &["remote", "set-url", "origin", url], false)?;
    }
    let clone = RepoClone::ensure_from(&dir, repo, url)?;
    let default = default_branch(&clone)?;
    let tip = format!("origin/{}", clone.working_branch(&default, "dev"));
    let branch = &tip["origin/".len()..];
    clone.git(&["reset", "-q", "--hard", "HEAD"])?;
    clone.git(&["clean", "-q", "-fd"])?;
    clone.git(&["checkout", "-q", "-B", branch, &tip])?;
    clone.git(&["reset", "-q", "--hard", &tip]).map(|_| ())
}

/// origin's HEAD branch, asked from the server (old clones lack `refs/remotes/origin/HEAD`).
fn default_branch(clone: &RepoClone) -> Result<String, String> {
    let out = clone.git_net(&["ls-remote", "--symref", "origin", "HEAD"])?;
    out.lines().find_map(|l| l.strip_prefix("ref: refs/heads/")).and_then(|r| r.split_whitespace().next()).map(str::to_string).ok_or_else(|| "default branch unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        run_git(Some(dir), args, false).unwrap();
    }

    #[test]
    fn clones_then_follows_dev_and_keeps_ignored_files() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = crate::fake::origin(tmp.path(), &[(".gitignore", "index/\n")]);
        let url = origin.display().to_string();
        let ws = tmp.path().join("ws");
        sync(&ws, "o/dep", &url).unwrap();
        let dep = ws.join("dep");
        assert_eq!(run_git(Some(&dep), &["rev-parse", "--abbrev-ref", "HEAD"], false).unwrap().trim(), "dev");
        std::fs::create_dir_all(dep.join("index")).unwrap();
        std::fs::write(dep.join("index").join("embeddings.json"), "{}").unwrap();
        std::fs::write(dep.join("README.md"), "local edit").unwrap();
        let seed = tmp.path().join("seed");
        std::fs::write(seed.join("new.txt"), "on dev").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "dev work"]);
        git(&seed, &["push", "-q", "origin", "main:dev"]);
        sync(&ws, "o/dep", &url).unwrap();
        assert!(dep.join("new.txt").exists(), "at the tip of dev");
        assert_eq!(std::fs::read_to_string(dep.join("README.md")).unwrap(), "seed", "local edits dropped");
        assert!(dep.join("index").join("embeddings.json").exists(), "ignored files stay");
    }
}
