//! Where the team agent keeps its files.
//!
//! Windows `%LOCALAPPDATA%\lunima-team`, Linux `$XDG_DATA_HOME/lunima-team`
//! (default `~/.local/share/lunima-team`). `LUNIMA_TEAM_HOME` overrides both.

use std::path::{Path, PathBuf};

pub const ENV_HOME: &str = "LUNIMA_TEAM_HOME";

/// The data folder layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default location for this user, or `None` when the OS gives no home folder.
    pub fn default_root() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(ENV_HOME).filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        let base = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local").join("share")))
        }?;
        Some(base.join("lunima-team"))
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("team.toml")
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    pub fn sessions(&self) -> PathBuf {
        self.state().join("sessions")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn prompts(&self) -> PathBuf {
        self.root.join("prompts")
    }
    /// `work/<owner>_<repo>` — the role's clone of a repository.
    pub fn clone_dir(&self, role: &str, repo: &str) -> PathBuf {
        self.root.join("work").join(role).join(repo.replace('/', "_"))
    }
    /// `work/wt/<repo>_<branch tail>_<hash>` — one worktree per branch. Kept short on
    /// purpose: Git for Windows refuses a `$GIT_DIR` (`<clone>/.git/worktrees/<name>`) over
    /// 260 characters, and the build trees inside need headroom too.
    pub fn worktree(&self, repo: &str, branch: &str) -> PathBuf {
        let name = repo.rsplit('/').next().unwrap_or(repo);
        let tail = branch.rsplit('/').next().unwrap_or(branch);
        let short = |s: &str, n: usize| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.').take(n).collect::<String>();
        self.root.join("work").join("wt").join(format!("{}_{}_{:06x}", short(name, 16), short(tail, 24), fnv(&format!("{repo}
{branch}")) & 0xff_ffff))
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [self.state(), self.sessions(), self.logs(), self.prompts()] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// FNV-1a — a stable hash (std's hasher may change between Rust versions).
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_under_root() {
        let p = Paths::new("R");
        assert_eq!(p.config(), Path::new("R").join("team.toml"));
        assert_eq!(p.clone_dir("coder", "o/r"), Path::new("R").join("work").join("coder").join("o_r"));
        let wt = p.worktree("o/r", "agent/issue-7");
        assert!(wt.starts_with(Path::new("R").join("work").join("wt")));
        assert!(wt.file_name().unwrap().to_str().unwrap().starts_with("r_issue-7_"));
        assert_ne!(wt, p.worktree("o/r", "feat/issue-7"), "same tail, different branch");
        assert_ne!(wt, p.worktree("x/r", "agent/issue-7"), "same name, different owner");
        assert!(p.worktree("Akhetonics/akhetonics-desktop-something-long", "agent/issue-1234-1791962038").file_name().unwrap().len() < 50);
    }

    #[test]
    fn env_override_wins() {
        std::env::set_var(ENV_HOME, "X:/team");
        assert_eq!(Paths::default_root(), Some(PathBuf::from("X:/team")));
        std::env::remove_var(ENV_HOME);
    }
}
