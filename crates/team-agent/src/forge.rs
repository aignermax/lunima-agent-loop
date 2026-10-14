//! What the roles need from GitHub, as a trait — the real implementation is `gh::Repo`
//! (the user's `gh` login), tests use `fake::FakeForge`.

use team_core::gh::{self, Comment, Issue, PullRequest};

pub trait Forge: Send + Sync {
    fn name(&self) -> &str;
    fn default_branch(&self) -> Result<String, String>;
    fn open_issues(&self, label: &str) -> Result<Vec<Issue>, String>;
    fn issue(&self, number: u64) -> Result<Issue, String>;
    fn add_labels(&self, number: u64, labels: &[&str]) -> Result<(), String>;
    fn remove_label(&self, number: u64, label: &str) -> Result<(), String>;
    fn add_assignee(&self, number: u64, login: &str) -> Result<(), String>;
    fn remove_assignees(&self, number: u64, logins: &[String]) -> Result<(), String>;
    fn comment(&self, number: u64, body: &str) -> Result<(), String>;
    fn comments(&self, number: u64) -> Result<Vec<Comment>, String>;
    fn close_issue(&self, number: u64) -> Result<(), String>;
    fn open_prs(&self) -> Result<Vec<PullRequest>, String>;
    fn pr(&self, number: u64) -> Result<PullRequest, String>;
    fn pr_by_branch(&self, branch: &str) -> Result<Option<PullRequest>, String>;
    fn create_pr(&self, title: &str, body: &str, head: &str, base: &str) -> Result<PullRequest, String>;
    fn edit_pr_body(&self, number: u64, body: &str) -> Result<(), String>;
    /// atomic cross-machine lock (`refs/team-locks/<key>`); false = held by someone else
    fn try_lock(&self, key: &str, owner: &str) -> Result<bool, String>;
    /// owner + creation time of a held lock (None = free)
    fn lock_info(&self, key: &str) -> Result<Option<gh::LockInfo>, String>;
    fn unlock(&self, key: &str) -> Result<(), String>;
}

impl Forge for gh::Repo {
    fn name(&self) -> &str {
        &self.name
    }
    fn default_branch(&self) -> Result<String, String> {
        gh::Repo::default_branch(self)
    }
    fn open_issues(&self, label: &str) -> Result<Vec<Issue>, String> {
        gh::Repo::open_issues(self, label)
    }
    fn issue(&self, number: u64) -> Result<Issue, String> {
        gh::Repo::issue(self, number)
    }
    fn add_labels(&self, number: u64, labels: &[&str]) -> Result<(), String> {
        gh::Repo::add_labels(self, number, labels)
    }
    fn remove_label(&self, number: u64, label: &str) -> Result<(), String> {
        gh::Repo::remove_label(self, number, label)
    }
    fn add_assignee(&self, number: u64, login: &str) -> Result<(), String> {
        gh::Repo::add_assignee(self, number, login)
    }
    fn remove_assignees(&self, number: u64, logins: &[String]) -> Result<(), String> {
        gh::Repo::remove_assignees(self, number, logins)
    }
    fn comment(&self, number: u64, body: &str) -> Result<(), String> {
        gh::Repo::comment(self, number, body)
    }
    fn comments(&self, number: u64) -> Result<Vec<Comment>, String> {
        gh::Repo::comments(self, number)
    }
    fn close_issue(&self, number: u64) -> Result<(), String> {
        gh::Repo::close_issue(self, number)
    }
    fn open_prs(&self) -> Result<Vec<PullRequest>, String> {
        gh::Repo::open_prs(self)
    }
    fn pr(&self, number: u64) -> Result<PullRequest, String> {
        gh::Repo::pr(self, number)
    }
    fn pr_by_branch(&self, branch: &str) -> Result<Option<PullRequest>, String> {
        gh::Repo::pr_by_branch(self, branch)
    }
    fn create_pr(&self, title: &str, body: &str, head: &str, base: &str) -> Result<PullRequest, String> {
        gh::Repo::create_pr(self, title, body, head, base)
    }
    fn edit_pr_body(&self, number: u64, body: &str) -> Result<(), String> {
        gh::Repo::edit_pr_body(self, number, body)
    }
    fn try_lock(&self, key: &str, owner: &str) -> Result<bool, String> {
        gh::Repo::try_lock(self, key, owner)
    }
    fn lock_info(&self, key: &str) -> Result<Option<gh::LockInfo>, String> {
        gh::Repo::lock_info(self, key)
    }
    fn unlock(&self, key: &str) -> Result<(), String> {
        gh::Repo::unlock(self, key)
    }
}

/// Lower-cased label names (provider policy, complexity checks).
pub fn label_names(labels: &[gh::Label]) -> Vec<String> {
    labels.iter().map(|l| l.name.to_lowercase()).collect()
}

/// Agent PRs are recognised by their title (contract with QA, discovery and PR feedback).
pub const PR_TITLE_PREFIX: &str = "Agent:";

/// QA verdict comments start with these (contract between QA and the coder's QA-fix).
pub const QA_COMMENT_PREFIX: &str = "[qa-agent]";
pub const QA_FAILED_PREFIX: &str = "[qa-agent] **FAILED**";
