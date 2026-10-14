//! Which model / endpoint a coder session uses (port of provider_policy.py):
//!
//! 0. `claudeapi` label → premium Claude model, in every repo
//! 1. repo routing `kimi-only` → OpenRouter; never falls back to Claude
//! 2. `eco` label → Moonshot/Kimi, only in repos cleared for third-party providers
//! 3. `complex` label → premium Claude model (when `complex_uses_premium`)
//! 4. repo routing `kimi` → OpenRouter (falls back to Claude without a key)
//! 5. default coder model
//!
//! Claude sessions run on the user's subscription: no API key is passed; any inherited
//! `ANTHROPIC_*` variable is removed from the child environment.

use crate::config::{Endpoint, Routing, TeamConfig};
use std::collections::BTreeMap;

/// Environment changes for the Claude CLI child process.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChildEnv {
    pub set: BTreeMap<String, String>,
    pub remove: Vec<String>,
}

/// Variables that would switch the CLI away from the subscription login.
pub const CLAUDE_AUTH_VARS: &[&str] = &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL", "ANTHROPIC_SMALL_FAST_MODEL", "CLAUDECODE"];

impl ChildEnv {
    /// Subscription login: strip anything that would override it.
    pub fn subscription() -> Self {
        Self { set: BTreeMap::new(), remove: CLAUDE_AUTH_VARS.iter().map(|s| s.to_string()).collect() }
    }

    /// An Anthropic-compatible third-party endpoint.
    pub fn endpoint(base_url: &str, key: &str, model: &str) -> Self {
        let mut set = BTreeMap::new();
        for (k, v) in [("ANTHROPIC_BASE_URL", base_url), ("ANTHROPIC_AUTH_TOKEN", key), ("ANTHROPIC_API_KEY", key), ("ANTHROPIC_MODEL", model), ("ANTHROPIC_SMALL_FAST_MODEL", model)] {
            set.insert(k.to_string(), v.to_string());
        }
        Self { set, remove: vec!["CLAUDECODE".into()] }
    }

    pub fn apply(&self, cmd: &mut std::process::Command) {
        for k in &self.remove {
            cmd.env_remove(k);
        }
        for (k, v) in &self.set {
            cmd.env(k, v);
        }
    }
}

/// The decision for one session.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// "" = Claude CLI default
    pub model: String,
    pub env: ChildEnv,
    /// human-readable reason, for the log
    pub why: String,
}

fn has(labels: &[String], tag: &str) -> bool {
    !tag.is_empty() && labels.iter().any(|l| l.eq_ignore_ascii_case(tag))
}

fn third_party(endpoint: &Endpoint, why: String) -> Option<Choice> {
    let key = endpoint.api_key()?;
    Some(Choice { model: endpoint.model.clone(), env: ChildEnv::endpoint(&endpoint.base_url, &key, &endpoint.model), why })
}

/// Errors only when a `kimi-only` repo has no OpenRouter key (never silently use Claude).
pub fn choose(cfg: &TeamConfig, repo: &str, labels: &[String]) -> Result<Choice, String> {
    let claude = |model: &str, why: String| Choice { model: model.to_string(), env: ChildEnv::subscription(), why };
    let routing = cfg.repo(repo).map(|r| r.routing).unwrap_or_default();
    let (l, p) = (&cfg.labels, &cfg.providers);

    if has(labels, &l.claudeapi) {
        return Ok(claude(&cfg.models.premium, format!("Label {} → Premium-Claude", l.claudeapi)));
    }
    if routing == Routing::KimiOnly {
        return third_party(&p.openrouter, format!("{repo}: nur Kimi"))
            .ok_or_else(|| format!("{repo} ist auf „nur Kimi“ gestellt, aber kein OpenRouter-Key konfiguriert — kein Ausweichen auf Claude"));
    }
    if has(labels, &l.eco) {
        if routing == Routing::Claude {
            // internal repo: code never leaves the Claude API
        } else if let Some(c) = third_party(&p.eco, format!("Label {} → {}", l.eco, p.eco.model)) {
            return Ok(c);
        }
    }
    if p.complex_uses_premium && has(labels, &l.complex) {
        return Ok(claude(&cfg.models.premium, format!("Label {} → Premium-Claude", l.complex)));
    }
    if routing == Routing::Kimi {
        if let Some(c) = third_party(&p.openrouter, format!("{repo}: Kimi")) {
            return Ok(c);
        }
    }
    Ok(claude(&cfg.models.coder, "Standard".into()))
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
