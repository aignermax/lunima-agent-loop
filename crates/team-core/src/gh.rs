//! GitHub through the `gh` CLI — uses the user's existing `gh auth login`, so no token is
//! stored anywhere. Each call is one `gh api` invocation; lists are paginated with
//! `--paginate --jq '.[]'` (one JSON object per line).

use crate::proc::{command, run_with_input};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<User>,
    /// present when the "issue" is a pull request
    #[serde(default)]
    pub pull_request: Option<Value>,
    #[serde(default)]
    pub html_url: String,
}

impl Issue {
    pub fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|l| l.name.eq_ignore_ascii_case(name))
    }
    pub fn label_names(&self) -> Vec<String> {
        self.labels.iter().map(|l| l.name.clone()).collect()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Ref {
    #[serde(rename = "ref")]
    pub name: String,
    #[serde(default)]
    pub sha: String,
    /// repository the branch lives in (`None` when the fork was deleted)
    #[serde(default)]
    pub repo: Option<RepoName>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RepoName {
    pub full_name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub html_url: String,
    pub head: Ref,
    pub base: Ref,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub updated_at: String,
}

impl PullRequest {
    pub fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|l| l.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Comment {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub created_at: String,
}

/// Low-level `gh api` call. `body` goes to stdin as JSON.
pub fn api(method: &str, path: &str, body: Option<&Value>) -> Result<String, String> {
    let mut cmd = command("gh");
    cmd.args(["api", "-X", method, path, "-H", "Accept: application/vnd.github+json"]);
    let input = body.map(|b| b.to_string().into_bytes());
    if input.is_some() {
        cmd.args(["--input", "-"]);
    }
    let out = run_with_input(cmd, None, input.as_deref(), TIMEOUT).map_err(|e| format!("gh: {e}"))?;
    out.into_result().map_err(|e| format!("gh api {method} {path}: {e}"))
}

pub fn get_json<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T, String> {
    serde_json::from_str(&api("GET", path, None)?).map_err(|e| format!("{path}: {e}"))
}

/// All items of a paginated list endpoint (`jq` selects the array, e.g. `.[]` or `.items[]`).
pub fn list<T: for<'de> Deserialize<'de>>(path: &str, jq: &str) -> Result<Vec<T>, String> {
    let mut cmd = command("gh");
    cmd.args(["api", "--paginate", path, "--jq", jq]);
    let text = run_with_input(cmd, None, None, Duration::from_secs(180)).map_err(|e| format!("gh: {e}"))?.into_result().map_err(|e| format!("gh api {path}: {e}"))?;
    parse_ndjson(&text)
}

pub fn parse_ndjson<T: for<'de> Deserialize<'de>>(text: &str) -> Result<Vec<T>, String> {
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).map_err(|e| format!("{e}: {}", l.chars().take(120).collect::<String>()))).collect()
}

/// `gh` reports HTTP errors as `… (HTTP 404)`; matching the bare number would also hit
/// issue #404 or a branch name inside the request path that is part of the error text.
pub fn is_status(err: &str, code: u16) -> bool {
    err.contains(&format!("(HTTP {code})"))
}

/// Percent-encodes a path segment (label names may contain spaces, colons …).
pub fn encode(segment: &str) -> String {
    segment.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// The authenticated user (claim lock owner).
pub fn current_user() -> Result<String, String> {
    get_json::<User>("user").map(|u| u.login)
}

/// GitHub calls for one repository.
#[derive(Debug, Clone)]
pub struct Repo {
    pub name: String,
}

impl Repo {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string() }
    }

    fn p(&self, rest: &str) -> String {
        format!("repos/{}/{rest}", self.name)
    }

    pub fn default_branch(&self) -> Result<String, String> {
        let v: Value = get_json(&format!("repos/{}", self.name))?;
        v.get("default_branch").and_then(Value::as_str).map(String::from).ok_or_else(|| "default_branch fehlt".into())
    }

    /// Open issues with `label`, oldest first, pull requests excluded.
    pub fn open_issues(&self, label: &str) -> Result<Vec<Issue>, String> {
        let path = self.p(&format!("issues?state=open&labels={}&sort=created&direction=asc&per_page=100", encode(label)));
        Ok(list::<Issue>(&path, ".[]")?.into_iter().filter(|i| i.pull_request.is_none()).collect())
    }

    pub fn issue(&self, number: u64) -> Result<Issue, String> {
        get_json(&self.p(&format!("issues/{number}")))
    }

    pub fn add_labels(&self, number: u64, labels: &[&str]) -> Result<(), String> {
        api("POST", &self.p(&format!("issues/{number}/labels")), Some(&json!({ "labels": labels }))).map(|_| ())
    }

    /// Removing a label that isn't there is not an error.
    pub fn remove_label(&self, number: u64, label: &str) -> Result<(), String> {
        match api("DELETE", &self.p(&format!("issues/{number}/labels/{}", encode(label))), None) {
            Err(e) if is_status(&e, 404) => Ok(()),
            other => other.map(|_| ()),
        }
    }

    pub fn add_assignee(&self, number: u64, login: &str) -> Result<(), String> {
        api("POST", &self.p(&format!("issues/{number}/assignees")), Some(&json!({ "assignees": [login] }))).map(|_| ())
    }

    pub fn remove_assignees(&self, number: u64, logins: &[String]) -> Result<(), String> {
        if logins.is_empty() {
            return Ok(());
        }
        api("DELETE", &self.p(&format!("issues/{number}/assignees")), Some(&json!({ "assignees": logins }))).map(|_| ())
    }

    pub fn comment(&self, number: u64, body: &str) -> Result<(), String> {
        api("POST", &self.p(&format!("issues/{number}/comments")), Some(&json!({ "body": body }))).map(|_| ())
    }

    pub fn comments(&self, number: u64) -> Result<Vec<Comment>, String> {
        list(&self.p(&format!("issues/{number}/comments?per_page=100")), ".[]")
    }

    pub fn close_issue(&self, number: u64) -> Result<(), String> {
        api("PATCH", &self.p(&format!("issues/{number}")), Some(&json!({ "state": "closed" }))).map(|_| ())
    }

    pub fn open_prs(&self) -> Result<Vec<PullRequest>, String> {
        list(&self.p("pulls?state=open&sort=created&direction=asc&per_page=100"), ".[]")
    }

    pub fn pr(&self, number: u64) -> Result<PullRequest, String> {
        get_json(&self.p(&format!("pulls/{number}")))
    }

    /// The open PR whose head is `branch` (this repo's own branches only).
    pub fn pr_by_branch(&self, branch: &str) -> Result<Option<PullRequest>, String> {
        let owner = self.name.split('/').next().unwrap_or("");
        let prs: Vec<PullRequest> = list(&self.p(&format!("pulls?state=open&head={owner}:{}&per_page=100", encode(branch))), ".[]")?;
        Ok(prs.into_iter().find(|p| p.head.name == branch))
    }

    pub fn create_pr(&self, title: &str, body: &str, head: &str, base: &str) -> Result<PullRequest, String> {
        let text = api("POST", &self.p("pulls"), Some(&json!({ "title": title, "body": body, "head": head, "base": base })))?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    pub fn edit_pr_body(&self, number: u64, body: &str) -> Result<(), String> {
        api("PATCH", &self.p(&format!("pulls/{number}")), Some(&json!({ "body": body }))).map(|_| ())
    }

    /// Atomic claim between machines: creates the hidden ref `refs/team-locks/<key>`.
    /// `Ok(false)` when it already exists (someone else holds the lock). Not a branch, so it
    /// shows up neither in the branch list nor in PRs.
    pub fn try_lock(&self, key: &str) -> Result<bool, String> {
        let base = self.default_branch()?;
        let sha: Value = get_json(&self.p(&format!("git/ref/heads/{}", encode(&base))))?;
        let sha = sha.pointer("/object/sha").and_then(Value::as_str).ok_or("Sperre: SHA fehlt")?.to_string();
        match api("POST", &self.p("git/refs"), Some(&json!({ "ref": format!("refs/team-locks/{key}"), "sha": sha }))) {
            Ok(_) => Ok(true),
            Err(e) if is_status(&e, 422) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Releases a lock from `try_lock` (missing lock is fine).
    pub fn unlock(&self, key: &str) -> Result<(), String> {
        match api("DELETE", &self.p(&format!("git/refs/team-locks/{key}")), None) {
            Err(e) if is_status(&e, 404) || is_status(&e, 422) => Ok(()),
            other => other.map(|_| ()),
        }
    }

    /// `true` if a branch exists on GitHub.
    pub fn branch_exists(&self, branch: &str) -> Result<bool, String> {
        match api("GET", &self.p(&format!("branches/{}", encode(branch))), None) {
            Ok(_) => Ok(true),
            Err(e) if is_status(&e, 404) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// `full_name`s of the repositories of all search hits (issues/PRs).
pub fn search_repos(query: &str) -> Result<Vec<String>, String> {
    let path = format!("search/issues?per_page=100&q={}", encode(query));
    // `--jq` prints strings raw (unquoted), one per line
    let mut cmd = command("gh");
    cmd.args(["api", "--paginate", &path, "--jq", ".items[].repository_url"]);
    let text = run_with_input(cmd, None, None, Duration::from_secs(180)).map_err(|e| format!("gh: {e}"))?.into_result().map_err(|e| format!("gh search: {e}"))?;
    let mut repos: Vec<String> = text.lines().filter_map(|u| u.trim().split("/repos/").nth(1).map(String::from)).collect();
    repos.sort();
    repos.dedup();
    Ok(repos)
}

#[cfg(test)]
#[path = "gh_tests.rs"]
mod tests;
