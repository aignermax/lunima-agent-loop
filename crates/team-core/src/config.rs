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
    /// nightly simulated-user walk through the desktop app (needs an unlocked desktop)
    pub ux_review: bool,
    /// weekly evidence-driven product discovery (scout → strategist → critic)
    pub product_discovery: bool,
}

impl Default for Roles {
    fn default() -> Self {
        Self { coder: true, qa: true, pr_feedback: true, product_owner: false, ux_review: false, product_discovery: false }
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
    /// cumulative tokens per issue before the coder stops (third-party providers cost money)
    pub max_tokens_regular: u64,
    pub max_tokens_complex: u64,
    /// failed attempts (errors) per issue before the coder gives up
    pub max_attempts: u32,
    /// run the repo's `test_cmd` (.agent.toml) before the reviewer
    pub test_gate: bool,
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
            max_tokens_regular: 20_000_000,
            max_tokens_complex: 50_000_000,
            max_attempts: 3,
            test_gate: true,
        }
    }
}

/// Memory guard (`crate::mem`): 0 = automatic (kill floor 10 % of RAM within 1–4 GB,
/// start floor twice that).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Resources {
    /// a running session/build/test is killed with all its processes below this much free RAM
    pub min_free_mb: u64,
    /// no role starts new work below this much free RAM
    pub start_free_mb: u64,
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
    /// passes per day on this machine (0 = no cap)
    pub max_runs_per_day: u32,
    /// fewer open task issues than this = backlog needs seeding (a pass is worth it)
    pub backlog_min: usize,
    pub max_turns: u32,
    /// optional customer/UX review run before each pass (shell command, "" = off); it gets
    /// the evidence file path in `LUNIMA_CUSTOMER_FEEDBACK` and the PO reads that file
    pub customer_command: String,
    /// evidence older than this is not acceptable for a merge
    pub customer_max_age_hours: u32,
    /// time budget of the customer command (desktop reviews + checkout/build per review)
    pub customer_timeout_minutes: u64,
}

impl Default for ProductOwner {
    fn default() -> Self {
        Self {
            repo: String::new(),
            integration_branch: "dev".into(),
            base_branch: "main".into(),
            interval_minutes: 240,
            timeout_minutes: 60,
            max_runs_per_day: 6,
            backlog_min: 5,
            max_turns: 150,
            customer_command: String::new(),
            customer_max_age_hours: 24,
            customer_timeout_minutes: 95,
        }
    }
}

/// `[ux_review]`: the nightly UX report. The computer-use tester (`tools/ux-tester` of a
/// lunima-agent-loop checkout) drives the real desktop app as simulated users; it needs
/// the Anthropic API (a subscription can't operate a desktop), so its key comes from a
/// key file and is handed to the tester process only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UxReview {
    /// "" = the Product Owner's repo
    pub repo: String,
    /// earliest local hour; the run window lasts `WINDOW_HOURS`
    pub hour: u32,
    /// folder with `ux_tester.py` (e.g. `C:\Users\me\lunima-agent-loop\tools\ux-tester`)
    pub tester_dir: String,
    /// interpreter command line, e.g. `py -3` or `python3`
    pub python: String,
    /// file with the Anthropic API key (never shown, never logged)
    pub api_key_file: String,
    pub model: String,
    pub max_steps: u32,
    /// findings at or above this severity become issues: minor | major | critical
    pub min_severity: String,
    /// project folder of the desktop app in the repo
    pub app_project: String,
    /// "" = built-in persona journeys
    pub goals_file: String,
    /// persona journeys per night (rotating)
    pub goals_per_run: usize,
    pub timeout_minutes: u64,
}

impl UxReview {
    pub const WINDOW_HOURS: u32 = 4;
}

impl Default for UxReview {
    fn default() -> Self {
        Self {
            repo: String::new(),
            hour: 2,
            tester_dir: String::new(),
            python: if cfg!(windows) { "py -3".into() } else { "python3".into() },
            api_key_file: String::new(),
            model: "claude-opus-5-5".into(),
            max_steps: 150,
            min_severity: "major".into(),
            app_project: "CAP.Desktop".into(),
            goals_file: String::new(),
            goals_per_run: 3,
            timeout_minutes: 180,
        }
    }
}

/// `[product_discovery]`: the weekly discovery run (see docs/DISCOVERY.md). Not to be
/// confused with `[discovery]`, which finds org repos with open agent tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProductDiscovery {
    /// "" = the Product Owner's repo
    pub repo: String,
    /// window start: weekday (mon … sun) and local hour; a missed window runs later that week
    pub weekday: String,
    pub hour: u32,
    /// "" = `models.product_owner`
    pub model: String,
    /// per stage (scout, strategist, critic)
    pub max_turns: u32,
    pub timeout_minutes: u64,
    /// runs per week that may end in an error before the week is given up
    pub max_attempts: u32,
    /// planning limit for the notional/real model cost of one week (not a permission to spend)
    pub weekly_budget_usd: f64,
    /// cost assumed for a stage until the ledger has history
    pub stage_estimate_usd: f64,
    pub max_open_opportunities: usize,
    pub max_new_experiments: usize,
    /// GitHub issues one run may create in total (opportunities, experiments, requests)
    pub max_new_issues: usize,
    pub max_spikes: usize,
    /// person-level outreach material (never committed); "" = `<state>/discovery/private`
    pub private_dir: String,
    /// "none" | "goatcounter" | "file"
    pub analytics: String,
    /// goatcounter: site code (`<code>.goatcounter.com`); file: path of a metrics JSON file
    pub analytics_site: String,
    /// goatcounter: file holding the API token (never logged)
    pub analytics_token_file: String,
    /// local notes the stages may read (copied into the run's context, never committed),
    /// e.g. an internal architecture review that is not in the repository
    pub context_files: Vec<String>,
}

impl Default for ProductDiscovery {
    fn default() -> Self {
        Self {
            repo: String::new(),
            weekday: "mon".into(),
            hour: 6,
            model: String::new(),
            max_turns: 120,
            timeout_minutes: 45,
            max_attempts: 3,
            weekly_budget_usd: 25.0,
            stage_estimate_usd: 4.0,
            max_open_opportunities: 3,
            max_new_experiments: 2,
            max_new_issues: 6,
            max_spikes: 1,
            private_dir: String::new(),
            analytics: "none".into(),
            analytics_site: String::new(),
            analytics_token_file: String::new(),
            context_files: Vec::new(),
        }
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
    pub ux_review: UxReview,
    pub product_discovery: ProductDiscovery,
    pub resources: Resources,
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
            ux_review: UxReview::default(),
            product_discovery: ProductDiscovery::default(),
            resources: Resources::default(),
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
    /// Writes the file atomically. Keys this version doesn't know (written by a newer agent
    /// or by hand) are kept; comments are not (TOML round trip).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut doc: toml::Table = toml::Table::try_from(self).map_err(|e| e.to_string())?;
        if let Some(old) = std::fs::read_to_string(path).ok().and_then(|t| t.parse::<toml::Table>().ok()) {
            keep_unknown(&mut doc, old);
        }
        let text = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// Applies settings edited elsewhere onto the file's current content: everything but
    /// the repository list (the Projects page owns that).
    pub fn with_repos_of(mut self, current: &TeamConfig) -> Self {
        self.repos = current.repos.clone();
        self
    }

    pub fn repo(&self, name: &str) -> Option<&RepoConfig> {
        self.repos.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }

    /// `repo` is the one the Product Owner plans and merges for.
    pub fn is_po_repo(&self, repo: &str) -> bool {
        !self.product_owner.repo.is_empty() && self.product_owner.repo.eq_ignore_ascii_case(repo)
    }

    /// Repos the coder works on (configured), discovered ones are added by the agent.
    pub fn coder_repos(&self) -> Vec<String> {
        self.repos.iter().filter(|r| r.coder).map(|r| r.name.clone()).collect()
    }

    /// Plausibility checks before saving or starting.
    /// Branch agent work targets in `repo` (if it exists there): the PO's integration branch
    /// for the PO repo, `dev` elsewhere.
    pub fn integration_branch(&self, repo: &str) -> String {
        if self.is_po_repo(repo) && !self.product_owner.integration_branch.is_empty() {
            self.product_owner.integration_branch.clone()
        } else {
            "dev".into()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for r in &self.repos {
            if r.name.split('/').filter(|p| !p.is_empty()).count() != 2 {
                return Err(format!("Repo „{}“ muss die Form owner/name haben.", r.name));
            }
        }
        let r = &self.resources;
        if r.start_free_mb != 0 && r.start_free_mb < r.min_free_mb {
            return Err("resources.start_free_mb muss mindestens resources.min_free_mb sein (sonst startet Arbeit, die sofort wieder beendet würde).".into());
        }
        let pd = &self.product_discovery;
        if !["mon", "tue", "wed", "thu", "fri", "sat", "sun"].contains(&pd.weekday.to_lowercase().as_str()) {
            return Err(format!("product_discovery.weekday „{}“: mon … sun erwartet.", pd.weekday));
        }
        if pd.hour > 23 {
            return Err("product_discovery.hour muss 0–23 sein.".into());
        }
        if pd.weekly_budget_usd.is_nan() || pd.weekly_budget_usd < 0.0 {
            return Err("product_discovery.weekly_budget_usd darf nicht negativ sein.".into());
        }
        if !["none", "goatcounter", "file"].contains(&pd.analytics.as_str()) {
            return Err(format!("product_discovery.analytics „{}“: none | goatcounter | file erwartet.", pd.analytics));
        }
        if self.poll_seconds == 0 {
            return Err("poll_seconds muss größer als 0 sein.".into());
        }
        Ok(())
    }
}

/// Copies keys of `old` that `new` lacks, recursively through tables.
fn keep_unknown(new: &mut toml::Table, old: toml::Table) {
    for (k, v) in old {
        match (new.get_mut(&k), v) {
            (None, v) => {
                new.insert(k, v);
            }
            (Some(toml::Value::Table(n)), toml::Value::Table(o)) => keep_unknown(n, o),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
