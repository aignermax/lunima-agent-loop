//! Projects: which repositories the team works on and how (issue agent on/off, Claude or
//! Kimi, which roles the repo's own .agent.toml enables).

use super::envfile::EnvFile;
use std::collections::BTreeMap;
use std::path::Path;

pub const REPOS_KEY: &str = "AGENT_REPOS";
pub const KIMI_KEY: &str = "AGENT_OPENROUTER_REPOS";
pub const FORCE_KEY: &str = "AGENT_OPENROUTER_FORCE_REPOS";

/// Which coder model family a repo gets (see provider_policy.py).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routing {
    /// Claude API (default for internal repos)
    Claude,
    /// Kimi via OpenRouter allowed (labels can still escalate to Claude)
    KimiAllowed,
    /// Kimi via OpenRouter mandatory, only `claudeapi` escapes it
    KimiForced,
}

impl Routing {
    pub fn label(self) -> &'static str {
        match self {
            Routing::Claude => "Claude",
            Routing::KimiAllowed => "Kimi (OpenRouter)",
            Routing::KimiForced => "nur Kimi",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub repo: String,
    /// listed in AGENT_REPOS
    pub manual: bool,
    /// found by the agent's org discovery
    pub discovered: bool,
    /// the PO loop's repo
    pub po: bool,
    pub routing: Routing,
}

impl Project {
    /// The issue agent works on it (manual list or discovery).
    pub fn handled(&self) -> bool {
        self.manual || self.discovered
    }
}

fn contains(list: &[String], repo: &str) -> bool {
    list.iter().any(|r| r.eq_ignore_ascii_case(repo))
}

/// Repos the agent discovered (keys of .sessions/discovered-repos.json).
pub fn read_discovered(agent_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(agent_dir.join(".sessions").join("discovered-repos.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&t).ok())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// All known repos, de-duplicated case-insensitively, sorted.
pub fn list(env: &EnvFile, discovered: &[String], po_repo: &str) -> Vec<Project> {
    let manual = env.list(REPOS_KEY);
    let (kimi, force) = (env.list(KIMI_KEY), env.list(FORCE_KEY));
    let mut all: BTreeMap<String, String> = BTreeMap::new();
    for r in manual.iter().chain(discovered).chain(kimi.iter()).chain(force.iter()).chain(std::iter::once(&po_repo.to_string())) {
        if !r.is_empty() {
            all.entry(r.to_lowercase()).or_insert_with(|| r.clone());
        }
    }
    all.into_values()
        .map(|repo| {
            let routing = if contains(&force, &repo) {
                Routing::KimiForced
            } else if contains(&kimi, &repo) {
                Routing::KimiAllowed
            } else {
                Routing::Claude
            };
            Project { manual: contains(&manual, &repo), discovered: contains(discovered, &repo), po: repo.eq_ignore_ascii_case(po_repo), routing, repo }
        })
        .collect()
}

/// Edits one comma-list key: adds or removes `repo` (case-insensitive), keeping the rest.
pub fn set_listed(env: &mut EnvFile, key: &str, repo: &str, listed: bool) {
    let mut items: Vec<String> = env.list(key).into_iter().filter(|r| !r.eq_ignore_ascii_case(repo)).collect();
    if listed {
        items.push(repo.to_string());
    }
    env.set(key, &items.join(","));
}

/// Applies a routing choice to the two OpenRouter lists.
pub fn set_routing(env: &mut EnvFile, repo: &str, routing: Routing) {
    set_listed(env, KIMI_KEY, repo, routing != Routing::Claude);
    set_listed(env, FORCE_KEY, repo, routing == Routing::KimiForced);
}

/// `agents_enabled = ["coder", "qa"]` from a .agent.toml (defaults to coder only, like the agent).
pub fn parse_agents_enabled(toml: &str) -> Vec<String> {
    let Some(line) = toml.lines().map(str::trim).find(|l| l.starts_with("agents_enabled")) else { return vec!["coder".into()] };
    let Some(list) = line.split_once('=').map(|(_, v)| v) else { return vec!["coder".into()] };
    list.trim_matches(|c: char| c.is_whitespace() || c == '[' || c == ']')
        .split(',')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "projects_tests.rs"]
mod tests;
