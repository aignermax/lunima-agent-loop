//! Uses a local bare repository as "origin" (no network, no GitHub).
use super::*;

fn sh(dir: &Path, args: &[&str]) -> String {
    run_git(Some(dir), args, false).unwrap_or_else(|e| panic!("{e}"))
}

/// origin (bare) with branches main + dev, and a clone of it.
fn setup() -> (tempfile::TempDir, RepoClone) {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let seed = tmp.path().join("seed");
    run_git(None, &["init", "-q", "--bare", "-b", "main", &origin.display().to_string()], false).unwrap();
    run_git(None, &["clone", "-q", &origin.display().to_string(), &seed.display().to_string()], false).unwrap();
    std::fs::write(seed.join("a.txt"), "1").unwrap();
    sh(&seed, &["add", "-A"]);
    sh(&seed, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
    sh(&seed, &["push", "-q", "origin", "main"]);
    sh(&seed, &["push", "-q", "origin", "main:dev"]);
    let dir = tmp.path().join("clone");
    run_git(None, &["clone", "-q", &origin.display().to_string(), &dir.display().to_string()], false).unwrap();
    let c = RepoClone { dir, repo: "o/r".into() };
    c.git(&["fetch", "-q", "origin"]).unwrap();
    (tmp, c)
}

#[test]
fn working_branch_prefers_dev() {
    let (_t, c) = setup();
    assert_eq!(c.working_branch("main", "dev"), "dev");
    assert_eq!(c.working_branch("main", "integration"), "main");
    assert!(!c.remote_branch_exists("nope"));
}

#[test]
fn strictly_behind_means_an_outgrown_snapshot() {
    let (tmp, c) = setup();
    assert!(!c.is_strictly_behind("dev", "main"), "equal branches");
    assert!(!c.is_strictly_behind("main", "main"));
    commit_on_origin(tmp.path(), &c, "main", "main", "m.txt");
    assert!(c.is_strictly_behind("dev", "main"));
    assert!(!c.is_strictly_behind("main", "dev"));
    commit_on_origin(tmp.path(), &c, "dev", "dev", "d.txt");
    assert!(!c.is_strictly_behind("dev", "main"), "a dev with commits of its own is live");
}

/// Merges `from` (an origin branch) into origin's `into` with a merge commit.
fn merge_on_origin(tmp: &Path, c: &RepoClone, from: &str, into: &str) {
    let seed = tmp.join("seed");
    sh(&seed, &["fetch", "-q", "origin"]);
    sh(&seed, &["checkout", "-q", "-B", into, &format!("origin/{into}")]);
    sh(&seed, &["-c", "user.name=t", "-c", "user.email=t@t", "merge", "-q", "--no-ff", "-m", &format!("merge {from}"), &format!("origin/{from}")]);
    sh(&seed, &["push", "-q", "origin", into]);
    c.git(&["fetch", "-q", "origin"]).unwrap();
}

fn commit_on_origin(tmp: &Path, c: &RepoClone, branch: &str, base: &str, file: &str) {
    let seed = tmp.join("seed");
    sh(&seed, &["fetch", "-q", "origin"]);
    sh(&seed, &["checkout", "-q", "-B", branch, &format!("origin/{base}")]);
    std::fs::write(seed.join(file), file).unwrap();
    sh(&seed, &["add", "-A"]);
    sh(&seed, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", file]);
    sh(&seed, &["push", "-q", "origin", branch]);
    c.git(&["fetch", "-q", "origin"]).unwrap();
}

#[test]
fn git_flow_dev_outgrown_by_its_release_merges_is_live() {
    let (tmp, c) = setup();
    commit_on_origin(tmp.path(), &c, "dev", "dev", "feature.txt");
    merge_on_origin(tmp.path(), &c, "dev", "main");
    commit_on_origin(tmp.path(), &c, "main", "main", "version-bump.txt");
    assert!(c.is_strictly_behind("dev", "main"));
    assert!(!c.is_bypassed("dev", "main"), "release merges and CI commits keep dev the way in");
}

#[test]
fn dev_is_bypassed_once_features_merge_straight_into_main() {
    let (tmp, c) = setup();
    commit_on_origin(tmp.path(), &c, "feat/x", "main", "x.txt");
    merge_on_origin(tmp.path(), &c, "feat/x", "main");
    assert!(c.is_bypassed("dev", "main"));
    assert!(!c.is_bypassed("main", "main"));
}

#[test]
fn worktree_commit_push_and_reuse() {
    let (tmp, c) = setup();
    let wt = tmp.path().join("wt").join("agent_issue-7");
    let path = c.worktree(&wt, "agent/issue-7", "dev").unwrap();
    std::fs::write(path.join("b.txt"), "new").unwrap();
    assert_eq!(commit_and_push(&path, "agent/issue-7", "Agent: implement #7").unwrap(), Pushed::Yes);
    assert_eq!(commit_and_push(&path, "agent/issue-7", "again").unwrap(), Pushed::Nothing);
    // uncommitted work in an existing worktree survives a second `worktree` call
    std::fs::write(path.join("wip.txt"), "unfinished").unwrap();
    let again = c.worktree(&wt, "agent/issue-7", "dev").unwrap();
    assert!(again.join("wip.txt").exists());
    c.remove_worktree(&wt).unwrap();
    assert!(!wt.exists());
}

#[test]
fn worktree_starts_from_origin_when_the_local_branch_is_stale() {
    let (tmp, c) = setup();
    let wt = tmp.path().join("wt").join("one");
    let path = c.worktree(&wt, "agent/issue-8", "dev").unwrap();
    std::fs::write(path.join("a.txt"), "mine").unwrap();
    commit_and_push(&path, "agent/issue-8", "first").unwrap();
    c.remove_worktree(&wt).unwrap();
    // another machine pushes on top
    let other = tmp.path().join("other");
    run_git(None, &["clone", "-q", "-b", "agent/issue-8", &tmp.path().join("origin.git").display().to_string(), &other.display().to_string()], false).unwrap();
    std::fs::write(other.join("c.txt"), "theirs").unwrap();
    commit_and_push(&other, "agent/issue-8", "second").unwrap();
    c.fetch().unwrap();
    let again = c.worktree(&tmp.path().join("wt").join("two"), "agent/issue-8", "dev").unwrap();
    assert!(again.join("c.txt").exists(), "worktree must contain the other machine's commit");
    // a kept worktree that fell behind follows origin when reused
    std::fs::write(other.join("d.txt"), "more").unwrap();
    commit_and_push(&other, "agent/issue-8", "third").unwrap();
    c.fetch().unwrap();
    let reused = c.worktree(&tmp.path().join("wt").join("two"), "agent/issue-8", "dev").unwrap();
    assert!(reused.join("d.txt").exists());
}

#[test]
fn reset_to_remote_discards_local_changes() {
    let (_t, c) = setup();
    std::fs::write(c.dir.join("junk.txt"), "x").unwrap();
    c.reset_to_remote("dev").unwrap();
    assert!(!c.dir.join("junk.txt").exists());
    assert_eq!(c.git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap().trim(), "dev");
}

#[test]
fn remote_url_has_no_token() {
    assert_eq!(remote_url("Akhetonics/khepri"), "https://github.com/Akhetonics/khepri.git");
}
