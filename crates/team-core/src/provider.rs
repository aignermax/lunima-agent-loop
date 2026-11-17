//! Which model / endpoint a coder session uses (port of provider_policy.py):
//!
//! Codex runner selection overrides all of the following rules.
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

use crate::config::{Backend, Endpoint, Providers, Routing, Runner, TeamConfig};
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

    /// ChatGPT login: do not inherit API credentials or third-party routing.
    pub fn codex_subscription() -> Self {
        let mut env = Self::subscription();
        env.remove.extend(["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"].iter().map(|s| s.to_string()));
        env
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

/// Explicit global runner choices take precedence over all role models and labels.
pub fn runner_choice(runner: &Runner, providers: &Providers) -> Result<Option<Choice>, String> {
    match runner.backend {
        Backend::Claude => Ok(None),
        Backend::Codex => Ok(Some(Choice { model: runner.model.clone(), env: ChildEnv::codex_subscription(), why: "Codex (ChatGPT) - runner override".into() })),
        Backend::OpenAi => {
            let endpoint = &providers.openai;
            if endpoint.base_url.trim().is_empty() {
                return Err("OpenAI API: Endpoint fehlt".into());
            }
            if endpoint.model.trim().is_empty() {
                return Err("OpenAI API: Modell fehlt".into());
            }
            let key = endpoint.api_key().ok_or("OpenAI API: API-Key fehlt; kein Ausweichen auf ein Abo")?;
            let mut env = ChildEnv::codex_subscription();
            env.set.insert("LUNIMA_CODEX_API_BASE_URL".into(), endpoint.base_url.clone());
            env.set.insert("LUNIMA_CODEX_API_KEY".into(), key);
            Ok(Some(Choice { model: endpoint.model.clone(), env, why: "OpenAI Responses API - runner override".into() }))
        }
        Backend::Openrouter | Backend::Eco => {
            let endpoint = if runner.backend == Backend::Openrouter { &providers.openrouter } else { &providers.eco };
            if endpoint.base_url.trim().is_empty() || endpoint.model.trim().is_empty() {
                return Err(format!("{}: Endpoint und Modell fehlen", runner.backend.label()));
            }
            third_party(endpoint, format!("{} - runner override", runner.backend.label())).map(Some).ok_or_else(|| format!("{}: API-Key fehlt; kein Ausweichen auf ein Abo", runner.backend.label()))
        }
    }
}

/// Errors only when a `kimi-only` repo has no OpenRouter key (never silently use Claude).
pub fn choose(cfg: &TeamConfig, repo: &str, labels: &[String]) -> Result<Choice, String> {
    if let Some(choice) = runner_choice(&cfg.runner, &cfg.providers)? {
        return Ok(choice);
    }
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
