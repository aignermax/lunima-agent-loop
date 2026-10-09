//! State of the loop's working clone. Uncommitted changes there block every worker
//! (`git checkout -B` refuses), which once silently stopped all work for days.

use super::{hidden, run};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct CloneStatus {
    pub exists: bool,
    /// Header from `git status -b`, e.g. "dev...origin/dev [behind 3]".
    pub branch_line: String,
    pub dirty: Vec<String>,
}

fn git(clone: &Path) -> std::process::Command {
    let mut cmd = hidden("git");
    cmd.arg("-C").arg(clone);
    cmd
}

/// Parses `git status --porcelain=v1 -b` output.
pub fn parse_status(text: &str) -> CloneStatus {
    let mut status = CloneStatus { exists: true, ..Default::default() };
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            status.branch_line = head.to_string();
        } else if line.len() > 3 {
            status.dirty.push(line[3..].trim_matches('"').to_string());
        }
    }
    status
}

pub fn status(clone: &Path) -> Result<CloneStatus, String> {
    if !clone.join(".git").exists() {
        return Ok(CloneStatus::default());
    }
    let mut cmd = git(clone);
    cmd.args(["status", "--porcelain=v1", "-b"]);
    run(cmd).map(|t| parse_status(&t))
}

/// Saves all uncommitted changes as a commit on a new local branch `wip/control-center-<stamp>`
/// and returns to the previous branch with a clean tree. Nothing is pushed or deleted.
pub fn rescue_wip(clone: &Path) -> Result<String, String> {
    let git_run = |args: &[&str]| {
        let mut cmd = git(clone);
        cmd.args(args);
        run(cmd).map_err(|e| format!("git {}: {e}", args.join(" ")))
    };
    let previous = git_run(&["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
    if previous == "HEAD" {
        return Err("Der Clone steht auf keinem Branch (detached HEAD) — bitte von Hand prüfen.".into());
    }
    let branch = format!("wip/control-center-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    git_run(&["switch", "-c", &branch])?;
    let saved = git_run(&["add", "-A"]).and_then(|_| {
        // hooks or signing must not leave the loop's clone stranded on the wip branch
        git_run(&[
            "-c", "user.name=lunima-po-center", "-c", "user.email=noreply@localhost", "-c", "commit.gpgsign=false",
            "commit", "--no-verify", "-q", "-m", "WIP rescued by the Control Center (was blocking the loop)",
        ])
    });
    if let Err(e) = saved {
        // back to where we were, changes still in the working tree; drop the empty branch
        let _ = git_run(&["reset", "-q"]);
        let _ = git_run(&["switch", &previous]);
        let _ = git_run(&["branch", "-D", &branch]);
        return Err(e);
    }
    git_run(&["switch", &previous])?;
    Ok(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_and_dirty_status() {
        let clean = parse_status("## dev...origin/dev\n");
        assert!(clean.dirty.is_empty());
        assert_eq!(clean.branch_line, "dev...origin/dev");
        let dirty = parse_status("## dev-ki...origin/dev-ki [behind 57]\n M src/a.cs\n?? \"docs/x y.md\"\n");
        assert_eq!(dirty.dirty, ["src/a.cs", "docs/x y.md"]);
    }

    #[test]
    fn rescue_refuses_detached_head_and_leaves_tree_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let g = |args: &[&str]| {
            let mut c = git(p);
            c.args(args);
            run(c).unwrap()
        };
        g(&["init", "-q", "-b", "dev"]);
        std::fs::write(p.join("a.txt"), "1").unwrap();
        g(&["add", "-A"]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
        g(&["switch", "-q", "--detach"]);
        std::fs::write(p.join("a.txt"), "2").unwrap();
        assert!(rescue_wip(p).unwrap_err().contains("detached"));
        assert_eq!(status(p).unwrap().dirty, ["a.txt"]);
    }

    #[test]
    fn rescue_moves_changes_to_wip_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let g = |args: &[&str]| {
            let mut c = git(p);
            c.args(args);
            run(c).unwrap()
        };
        g(&["init", "-q", "-b", "dev"]);
        std::fs::write(p.join("a.txt"), "1").unwrap();
        g(&["add", "-A"]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
        std::fs::write(p.join("a.txt"), "2").unwrap();
        std::fs::write(p.join("new.txt"), "n").unwrap();
        let branch = rescue_wip(p).unwrap();
        assert!(branch.starts_with("wip/control-center-"));
        assert!(status(p).unwrap().dirty.is_empty());
        assert_eq!(g(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "dev");
        assert_eq!(g(&["show", &format!("{branch}:a.txt")]).trim(), "2");
    }
}
