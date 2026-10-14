//! `team.toml` — the one configuration file of the team (no secrets: Claude runs on the
//! user's subscription login, GitHub on the user's `gh` login). Every field has a default,
//! so a nearly empty file works.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// How a repo's coder sessions are routed (see `provider`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Routing {
    /// Claude (subscription) — the default for internal repos
    #[default]
    Claude,
    /// Kimi via OpenRouter allowed (labels can still escalate to Claude)
    Kimi,
    /// Kimi via OpenRouter mandatory; only the `claudeapi` label escapes it
    KimiOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    /// owner/name
    pub name: String,
    /// the coder works on this repo
    pub coder: bool,
    pub routing: Routing,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self { name: String::new(), coder: true, routing: Routing::Claude }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roles {
    pub coder: bool,
    pub qa: bool,
    pub pr_feedback: bool,
    pub product_owner: bool,
}

impl Default for Roles {
    fn default() -> Self {
        Self { coder: true, qa: true, pr_feedback: true, product_owner: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Discovery {
    /// GitHub organisation searched for open `agent-task` issues; empty = off
    pub org: String,
    pub interval_secs: u64,
    pub expiry_days: i64,
}

impl Default for Discovery {
    fn default() -> Self {
        Self { org: "Akhetonics".into(), interval_secs: 120, expiry_days: 8 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Labels {
    pub task: String,
    /// put on every coder PR (the PO reviews and merges those)
    pub pr: String,
    /// claim marker while an issue is being worked on (by any machine)
    pub running: String,
    pub needs_human: String,
    pub complex: String,
    pub critical: String,
    pub eco: String,
    pub claudeapi: String,
    pub qa_passed: String,
    pub qa_failed: String,
    pub qa_running: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            task: "agent-task".into(),
            pr: "agent-pr".into(),
            running: "agent-running".into(),
            needs_human: "needs-human".into(),
            complex: "complex".into(),
            critical: "critical".into(),
            eco: "eco".into(),
            claudeapi: "claudeapi".into(),
            qa_passed: "qa-passed".into(),
            qa_failed: "qa-failed".into(),
            qa_running: "qa-running".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Models {
    /// coder default ("" = Claude CLI default)
    pub coder: String,
    /// complex issues and the claudeapi label
    pub premium: String,
    pub reviewer: String,
    pub reviewer_critical: String,
    pub product_owner: String,
}

impl Default for Models {
    fn default() -> Self {
        let opus = "claude-opus-5-5".to_string();
        Self { coder: opus.clone(), premium: opus.clone(), reviewer: opus.clone(), reviewer_critical: opus.clone(), product_owner: opus }
    }
}

/// An Anthropic-compatible third-party endpoint (OpenRouter, Moonshot).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
    /// file holding the API key (keeps the secret out of team.toml)
    pub key_file: String,
    /// environment variable holding the API key (alternative to key_file)
    pub key_env: String,
}

impl Endpoint {
    /// The API key from key_file or key_env, if any.
    pub fn api_key(&self) -> Option<String> {
        if !self.key_file.is_empty() {
            if let Ok(k) = std::fs::read_to_string(&self.key_file) {
                let k = k.trim().to_string();
                if !k.is_empty() {
                    return Some(k);
                }
            }
        }
        if !self.key_env.is_empty() {
            return std::env::var(&self.key_env).ok().filter(|k| !k.trim().is_empty());
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    pub openrouter: Endpoint,
    pub eco: Endpoint,
    /// complex issues run the coder on the premium Claude model
    pub complex_uses_premium: bool,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            openrouter: Endpoint { base_url: "https://openrouter.ai/api".into(), model: "moonshotai/kimi-k3".into(), ..Default::default() },
            eco: Endpoint { base_url: "https://api.moonshot.ai/anthropic".into(), model: "kimi-k2-thinking".into(), key_env: "MOONSHOT_API_KEY".into(), ..Default::default() },
            complex_uses_premium: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budgets {
    pub max_turns_regular: u32,
    pub max_turns_complex: u32,
    pub max_review_rounds_regular: u32,
    pub max_review_rounds_complex: u32,
    pub max_qa_fix_rounds: u32,
    pub reviewer_max_turns: u32,
    pub pr_feedback_max_rounds: u32,
    pub pr_feedback_max_turns: u32,
    /// coder sessions per issue (continuations after max turns)
    pub max_sessions: u32,
    /// a Claude session without any output for this long is killed
    pub inactivity_minutes: u64,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_turns_regular: 150,
            max_turns_complex: 500,
            max_review_rounds_regular: 1,
            max_review_rounds_complex: 2,
            max_qa_fix_rounds: 2,
            reviewer_max_turns: 80,
            pr_feedback_max_rounds: 3,
            pr_feedback_max_turns: 120,
            max_sessions: 20,
            inactivity_minutes: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProductOwner {
    /// the repo the PO plans and merges for ("" = no PO)
    pub repo: String,
    pub integration_branch: String,
    pub base_branch: String,
    pub interval_minutes: u64,
    pub timeout_minutes: u64,
}

impl Default for ProductOwner {
    fn default() -> Self {
        Self { repo: String::new(), integration_branch: "dev".into(), base_branch: "main".into(), interval_minutes: 240, timeout_minutes: 60 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeamConfig {
    /// shown in claim comments; default: the computer name
    pub machine: String,
    pub poll_seconds: u64,
    pub roles: Roles,
    pub repos: Vec<RepoConfig>,
    pub discovery: Discovery,
    pub labels: Labels,
    pub models: Models,
    pub providers: Providers,
    pub budgets: Budgets,
    pub product_owner: ProductOwner,
    /// `@agent` comments on PRs trigger the PR-feedback role
    pub pr_feedback_marker: String,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            machine: machine_name(),
            poll_seconds: 15,
            roles: Roles::default(),
            repos: Vec::new(),
            discovery: Discovery::default(),
            labels: Labels::default(),
            models: Models::default(),
            providers: Providers::default(),
            budgets: Budgets::default(),
            product_owner: ProductOwner::default(),
            pr_feedback_marker: "@agent".into(),
        }
    }
}

/// The computer's name (COMPUTERNAME / HOSTNAME / /etc/hostname).
pub fn machine_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "agent".into())
}

impl TeamConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Writes via a temp file so a crash never leaves a half-written config.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn repo(&self, name: &str) -> Option<&RepoConfig> {
        self.repos.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }

    /// Repos the coder works on (configured), discovered ones are added by the agent.
    pub fn coder_repos(&self) -> Vec<String> {
        self.repos.iter().filter(|r| r.coder).map(|r| r.name.clone()).collect()
    }

    /// Plausibility checks before saving or starting.
    pub fn validate(&self) -> Result<(), String> {
        for r in &self.repos {
            if r.name.split('/').filter(|p| !p.is_empty()).count() != 2 {
                return Err(format!("Repo „{}“ muss die Form owner/name haben.", r.name));
            }
        }
        if self.poll_seconds == 0 {
            return Err("poll_seconds muss größer als 0 sein.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
