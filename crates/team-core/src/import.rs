//! First-start import: builds a `team.toml` from the Python agent's `.env` and the PO
//! loop's `agent-loop.json`. Secrets (API keys, tokens) are never copied — Claude uses
//! the subscription login, GitHub the `gh` login; key *files* are referenced.

use crate::config::{RepoConfig, Routing, TeamConfig};
use std::collections::BTreeMap;
use std::path::Path;

/// `KEY=value` pairs as python-dotenv reads them (last wins, quotes, ` #` comments).
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        let t = t.strip_prefix("export ").unwrap_or(t);
        let Some((k, v)) = t.split_once('=') else { continue };
        let v = v.trim();
        let value = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_string(),
            _ => v.split(" #").next().unwrap_or("").trim().to_string(),
        };
        out.insert(k.trim().to_string(), value);
    }
    out
}

/// `/mnt/c/Users/x` → `C:\Users\x` on Windows (paths written for WSL); unchanged elsewhere.
pub fn native_path(p: &str) -> String {
    if !cfg!(windows) {
        return p.to_string();
    }
    let rest = match p.strip_prefix("/mnt/") {
        Some(r) => r,
        None => return p.to_string(),
    };
    let mut parts = rest.splitn(2, '/');
    match (parts.next(), parts.next()) {
        (Some(drive), tail) if drive.len() == 1 => format!("{}:\\{}", drive.to_ascii_uppercase(), tail.unwrap_or("").replace('/', "\\")),
        _ => p.to_string(),
    }
}

fn list(env: &BTreeMap<String, String>, key: &str) -> Vec<String> {
    env.get(key).map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default()
}

fn set_str(target: &mut String, env: &BTreeMap<String, String>, key: &str) {
    if let Some(v) = env.get(key).filter(|v| !v.is_empty()) {
        *target = v.clone();
    }
}

fn set_num<T: std::str::FromStr>(target: &mut T, env: &BTreeMap<String, String>, key: &str) {
    if let Some(v) = env.get(key).and_then(|v| v.parse().ok()) {
        *target = v;
    }
}

fn has(list: &[String], repo: &str) -> bool {
    list.iter().any(|r| r.eq_ignore_ascii_case(repo))
}

/// Applies the Python agent's settings.
pub fn apply_env(c: &mut TeamConfig, env: &BTreeMap<String, String>) {
    let mut repos = list(env, "AGENT_REPOS");
    if repos.is_empty() {
        repos = list(env, "AGENT_REPO");
    }
    let (kimi, force) = (list(env, "AGENT_OPENROUTER_REPOS"), list(env, "AGENT_OPENROUTER_FORCE_REPOS"));
    // repos only named in a provider list (e.g. found by discovery) keep their routing too
    for name in force.iter().chain(&kimi) {
        if !has(&repos, name) {
            repos.push(name.clone());
        }
    }
    for name in &repos {
        let routing = if has(&force, name) {
            Routing::KimiOnly
        } else if has(&kimi, name) {
            Routing::Kimi
        } else {
            Routing::Claude
        };
        c.repos.push(RepoConfig { name: name.clone(), coder: true, routing });
    }
    if let Some(org) = env.get("AGENT_DISCOVERY_ORG") {
        c.discovery.org = org.clone(); // explicit empty = discovery off
    }
    set_num(&mut c.discovery.interval_secs, env, "AGENT_DISCOVERY_INTERVAL_SEC");
    set_num(&mut c.discovery.expiry_days, env, "AGENT_DISCOVERY_EXPIRY_DAYS");
    set_num(&mut c.poll_seconds, env, "AGENT_POLL_INTERVAL");

    let l = &mut c.labels;
    set_str(&mut l.task, env, "AGENT_ISSUE_LABEL");
    if let Some(first) = list(env, "AGENT_PR_LABELS").into_iter().next() {
        l.pr = first;
    }
    set_str(&mut l.complex, env, "AGENT_COMPLEXITY_TAG");
    set_str(&mut l.critical, env, "AGENT_CRITICAL_LABEL");
    set_str(&mut l.eco, env, "AGENT_ECO_TAG");
    set_str(&mut l.claudeapi, env, "AGENT_CLAUDEAPI_TAG");

    let m = &mut c.models;
    set_str(&mut m.coder, env, "AGENT_CODER_MODEL");
    // Python: premium defaults to the coder model
    if env.get("AGENT_PREMIUM_CODER_MODEL").is_none_or(|v| v.is_empty()) && env.get("AGENT_CODER_MODEL").is_some_and(|v| !v.is_empty()) {
        m.premium = m.coder.clone();
    }
    set_str(&mut m.premium, env, "AGENT_PREMIUM_CODER_MODEL");
    set_str(&mut m.reviewer, env, "AGENT_REVIEWER_MODEL");
    set_str(&mut m.reviewer_critical, env, "AGENT_REVIEWER_MODEL_CRITICAL");

    let p = &mut c.providers;
    set_str(&mut p.openrouter.model, env, "AGENT_OPENROUTER_MODEL");
    set_str(&mut p.openrouter.base_url, env, "AGENT_OPENROUTER_BASE_URL");
    if let Some(f) = env.get("AGENT_OPENROUTER_KEY_FILE").filter(|f| !f.is_empty()) {
        p.openrouter.key_file = native_path(f);
    }
    set_str(&mut p.eco.model, env, "AGENT_ECO_MODEL");
    set_str(&mut p.eco.base_url, env, "AGENT_ECO_BASE_URL");
    if let Some(v) = env.get("AGENT_COMPLEX_USES_CLAUDE") {
        p.complex_uses_premium = v.eq_ignore_ascii_case("true");
    }

    let b = &mut c.budgets;
    set_num(&mut b.max_turns_regular, env, "AGENT_MAX_TURNS_REGULAR");
    set_num(&mut b.max_turns_complex, env, "AGENT_MAX_TURNS_COMPLEX");
    set_num(&mut b.max_review_rounds_regular, env, "AGENT_MAX_REVIEW_ROUNDS_REGULAR");
    set_num(&mut b.max_review_rounds_complex, env, "AGENT_MAX_REVIEW_ROUNDS_COMPLEX");
    set_num(&mut b.max_qa_fix_rounds, env, "AGENT_MAX_QA_FIX_ROUNDS");
    set_num(&mut b.reviewer_max_turns, env, "AGENT_REVIEWER_MAX_TURNS");
    set_num(&mut b.pr_feedback_max_rounds, env, "AGENT_PR_FEEDBACK_MAX_ROUNDS");
    set_num(&mut b.pr_feedback_max_turns, env, "AGENT_PR_FEEDBACK_MAX_TURNS");
    set_str(&mut c.pr_feedback_marker, env, "AGENT_PR_FEEDBACK_MARKER");
}

/// Applies the PO loop's agent-loop.json (the PO role itself stays off until it is ported).
pub fn apply_loop_config(c: &mut TeamConfig, json: &serde_json::Value) {
    let s = |k: &str| json.as_object().and_then(|o| o.iter().find(|(key, _)| key.eq_ignore_ascii_case(k))).and_then(|(_, v)| v.as_str()).map(String::from);
    let n = |k: &str| json.as_object().and_then(|o| o.iter().find(|(key, _)| key.eq_ignore_ascii_case(k))).and_then(|(_, v)| v.as_u64());
    let po = &mut c.product_owner;
    if let Some(v) = s("githubRepo") {
        po.repo = v;
    }
    if let Some(v) = s("integrationBranch") {
        po.integration_branch = v;
    }
    if let Some(v) = s("baseBranch") {
        po.base_branch = v;
    }
    if let Some(v) = n("ownerIntervalMinutes") {
        po.interval_minutes = v;
    }
    if let Some(v) = n("ownerTimeoutMinutes") {
        po.timeout_minutes = v;
    }
    if s("ownerRunner").is_some_and(|r| r.eq_ignore_ascii_case("claude")) {
        if let Some(m) = s("ownerModel") {
            c.models.product_owner = m;
        }
    }
    if let Some(v) = s("prLabel") {
        c.labels.pr = v;
    }
    if let Some(v) = s("runningLabel") {
        c.labels.running = v;
    }
    if let Some(v) = s("blockedLabel") {
        c.labels.needs_human = v;
    }
}

/// Builds a config from whatever of the two legacy files exists.
/// Builds a config from the legacy files. Each given file that cannot be read or parsed is
/// an error (a silently ignored typo would leave a config without repos).
pub fn from_legacy(env_file: Option<&Path>, loop_config: Option<&Path>) -> Result<TeamConfig, String> {
    let mut c = TeamConfig::default();
    if let Some(p) = env_file {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        apply_env(&mut c, &parse_env(&text));
    }
    if let Some(p) = loop_config {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let json = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", p.display()))?;
        apply_loop_config(&mut c, &json);
    }
    c.validate()?;
    Ok(c)
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
