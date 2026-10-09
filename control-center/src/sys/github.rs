//! Live view on GitHub through the `gh` CLI (uses the machine's existing login).

use super::{hidden, run};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub created_at: String,
    #[serde(default)]
    pub labels: Vec<Label>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub created_at: String,
    pub base_ref_name: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub labels: Vec<Label>,
}

#[derive(Debug, Clone, Default)]
pub struct GithubData {
    pub recent_issues: Vec<Issue>,
    pub open_prs: Vec<PullRequest>,
}

impl Issue {
    pub fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|l| l.name.eq_ignore_ascii_case(name))
    }
}

fn gh(args: &[&str]) -> Result<String, String> {
    let mut cmd = hidden("gh");
    cmd.args(args);
    run(cmd)
}

/// Newest issues (any state) and open PRs of the repo.
pub fn fetch(repo: &str) -> Result<GithubData, String> {
    let issues = gh(&["issue", "list", "--repo", repo, "--state", "all", "--limit", "40", "--json", "number,title,state,url,createdAt,labels"])?;
    let prs = gh(&["pr", "list", "--repo", repo, "--state", "open", "--limit", "40", "--json", "number,title,url,createdAt,baseRefName,isDraft,labels"])?;
    let mut recent_issues: Vec<Issue> = serde_json::from_str(&issues).map_err(|e| format!("gh issue list: {e}"))?;
    recent_issues.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let open_prs = serde_json::from_str(&prs).map_err(|e| format!("gh pr list: {e}"))?;
    Ok(GithubData { recent_issues, open_prs })
}

/// A file from the repo's default branch, raw (e.g. `.agent.toml`).
pub fn fetch_raw(repo: &str, path: &str) -> Result<String, String> {
    gh(&["api", "-H", "Accept: application/vnd.github.raw", &format!("repos/{repo}/contents/{path}")])
}

/// `gh auth status` — Ok when logged in.
pub fn auth_status() -> Result<(), String> {
    gh(&["auth", "status"]).map(|_| ()).map_err(|e| e.lines().next().unwrap_or("gh nicht angemeldet").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gh_issue_json() {
        let json = r#"[{"number":1480,"title":"A","state":"OPEN","url":"u","createdAt":"2026-10-09T07:00:00Z","labels":[{"id":"x","name":"agent-task","color":"c"}]}]"#;
        let issues: Vec<Issue> = serde_json::from_str(json).unwrap();
        assert!(issues[0].has_label("Agent-Task"));
    }
}
