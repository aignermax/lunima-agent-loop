//! git for clones, worktrees, commit and push. Network operations authenticate through
//! `gh auth git-credential` (the user's gh login) — no token in `.git/config` (the Python
//! agent embedded it in the remote URL), no credential pop-ups on Windows. Output is forced
//! to English (`LC_ALL=C`) because push errors are matched by text.

use crate::proc::{command, run};
use std::path::{Path, PathBuf};
use std::time::Duration;

const LOCAL: Duration = Duration::from_secs(120);
const NETWORK: Duration = Duration::from_secs(600);

fn git(dir: Option<&Path>, network: bool) -> std::process::Command {
    let mut cmd = command("git");
    if network {
        // empty helper first resets inherited helpers (e.g. Git Credential Manager)
        cmd.args(["-c", "credential.helper=", "-c", "credential.helper=!gh auth git-credential"]);
    }
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never").env("LC_ALL", "C").env("LANG", "C");
    cmd
}

/// Runs git; Ok(stdout) or Err(stderr).
pub fn run_git(dir: Option<&Path>, args: &[&str], network: bool) -> Result<String, String> {
    let mut cmd = git(dir, network);
    cmd.args(args);
    let timeout = if network { NETWORK } else { LOCAL };
    run(cmd, None, timeout).map_err(|e| format!("git: {e}"))?.into_result().map_err(|e| format!("git {}: {e}", args.join(" ")))
}

pub fn remote_url(repo: &str) -> String {
    format!("https://github.com/{repo}.git")
}

/// A role's own clone of a repository.
#[derive(Debug, Clone)]
pub struct RepoClone {
    pub dir: PathBuf,
    pub repo: String,
}

impl RepoClone {
    /// Clones if missing, then fetches. Never touches a working tree that isn't ours to reset.
    pub fn ensure(dir: &Path, repo: &str) -> Result<Self, String> {
        if !dir.join(".git").exists() {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            run_git(None, &["clone", &remote_url(repo), &dir.display().to_string()], true)?;
        }
        let c = Self { dir: dir.to_path_buf(), repo: repo.to_string() };
        c.fetch()?;
        Ok(c)
    }

    pub fn git(&self, args: &[&str]) -> Result<String, String> {
        run_git(Some(&self.dir), args, false)
    }

    pub fn git_net(&self, args: &[&str]) -> Result<String, String> {
        run_git(Some(&self.dir), args, true)
    }

    pub fn fetch(&self) -> Result<(), String> {
        self.git_net(&["fetch", "--prune", "origin"]).map(|_| ())
    }

    pub fn remote_branch_exists(&self, branch: &str) -> bool {
        self.git(&["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{branch}")]).is_ok()
    }

    /// The integration branch (e.g. `dev`, see `TeamConfig::integration_branch`) when it
    /// exists on origin, else the default branch.
    pub fn working_branch(&self, default_branch: &str, integration: &str) -> String {
        if !integration.is_empty() && self.remote_branch_exists(integration) { integration.into() } else { default_branch.into() }
    }

    fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.git(&["merge-base", "--is-ancestor", a, b]).is_ok()
    }

    /// Checks out `branch` exactly as on origin, discarding local state (role clones only).
    pub fn reset_to_remote(&self, branch: &str) -> Result<(), String> {
        self.git_net(&["fetch", "origin", &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")])?;
        self.git(&["checkout", "-B", branch, &format!("origin/{branch}")])?;
        self.git(&["reset", "--hard", &format!("origin/{branch}")])?;
        self.git(&["clean", "-fdx"]).map(|_| ())
    }

    /// Linked worktree for `branch` at `path`; created from `base` if the branch is new.
    /// An existing worktree is reused **as is** (uncommitted work of a previous session stays).
    pub fn worktree(&self, path: &Path, branch: &str, base: &str) -> Result<PathBuf, String> {
        if path.join(".git").exists() {
            return Ok(path.to_path_buf());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let p = path.display().to_string();
        let local = self.git(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok();
        let remote = format!("origin/{branch}");
        if local && self.remote_branch_exists(branch) && self.is_ancestor(branch, &remote) {
            // another machine (QA fix, PR feedback) pushed since — start from origin
            self.git(&["worktree", "add", "-B", branch, &p, &remote])?;
        } else if local {
            // ahead of / diverged from origin: keep the unpushed commits
            self.git(&["worktree", "add", &p, branch])?;
        } else if self.remote_branch_exists(branch) {
            self.git(&["worktree", "add", "-b", branch, &p, &format!("origin/{branch}")])?;
        } else {
            self.git(&["worktree", "add", "-b", branch, &p, &format!("origin/{base}")])?;
        }
        Ok(path.to_path_buf())
    }

    /// Removes a worktree (Windows: clears read-only files git leaves under .git/objects).
    pub fn remove_worktree(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        if self.git(&["worktree", "remove", "--force", &path.display().to_string()]).is_err() {
            clear_readonly(path);
            std::fs::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let _ = self.git(&["worktree", "prune"]);
        }
        Ok(())
    }
}

fn clear_readonly(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            clear_readonly(&p);
        } else if let Ok(meta) = std::fs::metadata(&p) {
            let mut perm = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&p, perm);
        }
    }
}

/// Result of `commit_and_push`.
#[derive(Debug, Clone, PartialEq)]
pub enum Pushed {
    /// new commits are on origin
    Yes,
    /// nothing new (no changes, nothing unpushed)
    Nothing,
}

/// `git add -A`, commit if anything changed, push when ahead of origin. A rejected push is
/// retried with `--force-with-lease` only if the remote branch is strictly behind ours.
pub fn commit_and_push(dir: &Path, branch: &str, message: &str) -> Result<Pushed, String> {
    let g = |a: &[&str]| run_git(Some(dir), a, false);
    let gn = |a: &[&str]| run_git(Some(dir), a, true);
    g(&["add", "-A"])?;
    if !g(&["status", "--porcelain"])?.trim().is_empty() {
        g(&["-c", "user.name=lunima-team-agent", "-c", "user.email=agent@lunima.invalid", "commit", "-q", "-m", message])?;
    }
    let remote = format!("origin/{branch}");
    let ahead = g(&["rev-list", "--count", &format!("{remote}..{branch}")]).ok().and_then(|n| n.trim().parse::<u32>().ok());
    if ahead == Some(0) {
        return Ok(Pushed::Nothing);
    }
    match gn(&["push", "--set-upstream", "origin", branch]) {
        Ok(_) => Ok(Pushed::Yes),
        Err(e) if e.contains("rejected") || e.contains("fetch first") || e.contains("non-fast-forward") => {
            gn(&["fetch", "origin", &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")])?;
            let behind = g(&["rev-list", "--count", &format!("{branch}..{remote}")])?.trim().parse::<u32>().unwrap_or(1);
            if behind == 0 {
                gn(&["push", "--force-with-lease", "origin", branch]).map(|_| Pushed::Yes)
            } else {
                Err(format!("Branch {branch} ist mit origin auseinandergelaufen — braucht einen Menschen"))
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
