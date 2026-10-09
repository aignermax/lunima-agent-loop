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

/// Repos the agent currently works on via discovery: registry entries whose `last_seen`
/// is within the expiry window (the agent ignores older ones; only its coder prunes them).
pub fn read_discovered(agent_dir: &Path, expiry_days: i64, now: chrono::NaiveDateTime) -> Vec<String> {
    let Some(map) = std::fs::read_to_string(agent_dir.join(".sessions").join("discovered-repos.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&t).ok())
    else {
        return Vec::new();
    };
    let cutoff = now - chrono::Duration::days(expiry_days);
    map.into_iter()
        .filter(|(_, v)| {
            v.get("last_seen")
                .and_then(|s| s.as_str())
                .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok())
                .is_some_and(|t| t >= cutoff)
        })
        .map(|(k, _)| k)
        .collect()
}

/// Manual repos as the agent reads them: AGENT_REPOS, else the legacy AGENT_REPO.
pub fn manual_repos(env: &EnvFile) -> Vec<String> {
    let list = env.list(REPOS_KEY);
    if !list.is_empty() {
        return list;
    }
    env.get("AGENT_REPO").filter(|r| !r.trim().is_empty()).map(|r| vec![r.trim().to_string()]).unwrap_or_default()
}

/// All known repos, de-duplicated case-insensitively, sorted.
pub fn list(env: &EnvFile, discovered: &[String], po_repo: &str) -> Vec<Project> {
    let manual = manual_repos(env);
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

/// Turns the issue agent on/off for a repo. Refuses to empty the manual list: an empty
/// AGENT_REPOS makes the agent fall back to AGENT_REPO / a hard-coded default repo.
pub fn set_handled(env: &mut EnvFile, repo: &str, handled: bool) -> Result<(), String> {
    let mut items: Vec<String> = manual_repos(env).into_iter().filter(|r| !r.eq_ignore_ascii_case(repo)).collect();
    if handled {
        items.push(repo.to_string());
    }
    if items.is_empty() {
        return Err("Mindestens ein Repo muss eingetragen bleiben — sonst fällt der Agent auf ein Standard-Repo zurück.".into());
    }
    env.assign(REPOS_KEY, &items.join(","));
    Ok(())
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

/// Strips a `# comment` that is outside a TOML string.
fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '#') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `agents_enabled = [...]` from a .agent.toml — single- or multi-line, with comments.
/// Missing or empty means `["coder"]`, exactly like the agent's own loader.
pub fn parse_agents_enabled(toml: &str) -> Vec<String> {
    let text: String = toml.lines().map(strip_toml_comment).collect::<Vec<_>>().join("\n");
    let roles: Vec<String> = text
        .find("agents_enabled")
        .and_then(|i| text[i..].find('[').map(|o| i + o))
        .and_then(|start| text[start..].find(']').map(|end| &text[start + 1..start + end]))
        .map(|inner| {
            inner.split(',').map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string()).filter(|s| !s.is_empty()).collect()
        })
        .unwrap_or_default();
    if roles.is_empty() { vec!["coder".into()] } else { roles }
}

#[cfg(test)]
#[path = "projects_tests.rs"]
mod tests;
