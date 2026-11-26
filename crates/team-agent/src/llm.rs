//! Claude sessions as a trait (`ClaudeCli` = the real CLI, `fake::FakeLlm` in tests).

use chrono::{DateTime, Local};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use team_core::claude::{self, Outcome, Session};
use team_core::config::{Backend, Runner, TeamConfig};
use team_core::provider::ChildEnv;
use team_core::state::Heartbeat;

/// One Claude run.
#[derive(Debug, Clone, Default)]
pub struct PolicyRequest {
    pub role: team_core::project::SessionRole,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub policy: PolicyRequest,
    /// Explicit project identity, independent of worktree paths and log labels.
    pub repo: String,
    pub prompt: String,
    pub cwd: PathBuf,
    /// "" = CLI default
    pub model: String,
    pub max_turns: u32,
    pub env: ChildEnv,
    /// log file name stem, e.g. `coder_o_r_issue-7`
    pub label: String,
    /// hard limit for the whole session (`None` = inactivity watchdog only)
    pub max_duration: Option<Duration>,
    /// `Some` = sandboxed: only these built-in tools, no permission bypass
    pub tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmError {
    /// Admission failed before execution; do not penalize the task or publish a verdict.
    Unavailable(String),
    /// the subscription's usage limit — nothing is wrong with the task; retry after this time
    UsageLimit(team_core::config::Backend, DateTime<Local>),
    Failed(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::Unavailable(e) => write!(f, "Sitzung wartet: {e}"),
            LlmError::UsageLimit(backend, t) => write!(f, "{}-Nutzungslimit erreicht bis {}", backend.label(), t.format("%d.%m. %H:%M")),
            LlmError::Failed(e) => write!(f, "{e}"),
        }
    }
}

pub trait Llm: Send + Sync {
    fn run(&self, job: &Job) -> Result<Outcome, LlmError>;
    /// Check availability without executing tools or consuming a task attempt.
    fn admit(&self, _job: &Job) -> Result<(), LlmError> { Ok(()) }
}

/// The Claude Code CLI with the bundled commit-comment hook; raw stream-json goes to
/// `<logs>/<date>_<label>.jsonl`, the role's heartbeat is refreshed while it runs.
pub struct ClaudeCli {
    cfg: TeamConfig,
    pub settings_file: PathBuf,
    pub logs_dir: PathBuf,
    pub inactivity: Duration,
    pub heartbeat: Option<Arc<Heartbeat>>,
    /// the role whose sessions these are (`LUNIMA_ROLE` in their environment)
    pub role: String,
    pub usage_limit: Option<crate::daemon::UsageLimit>,
}

impl ClaudeCli {
    fn profiled(&self, job: &Job) -> bool {
        self.cfg.repo(&job.repo).is_some_and(|r| r.profile.is_some())
    }

    fn decision(&self, job: &Job) -> Result<team_core::project::Decision, LlmError> {
        if self.profiled(job) {
            let profile = self.cfg.repo(&job.repo).and_then(|r| r.profile.as_ref()).expect("profile checked");
            team_core::project::validate(&self.cfg, &job.repo, profile).map_err(LlmError::Unavailable)?;
            let mut decision = team_core::project::resolve(&self.cfg, &job.repo, job.policy.role, &job.policy.labels).map_err(LlmError::Unavailable)?;
            Self::carry_env(&mut decision.choice, &job.env);
            return Ok(decision);
        }
        Ok(team_core::project::Decision { backend: self.quota_backend(job), choice: self.choice(job)?, effort: None })
    }

    fn carry_env(choice: &mut team_core::provider::Choice, env: &ChildEnv) {
        let auth = ChildEnv::codex_subscription().remove;
        for (key, value) in &env.set {
            if !auth.contains(key) && !key.starts_with("ANTHROPIC_") && !key.starts_with("OPENAI_") && !key.starts_with("LUNIMA_CODEX_") && !choice.env.set.contains_key(key) {
                choice.env.set.insert(key.clone(), value.clone());
            }
        }
        for key in &env.remove {
            if !choice.env.remove.contains(key) { choice.env.remove.push(key.clone()); }
        }
    }

    fn cooldown(&self, backend: Backend) -> Option<DateTime<Local>> {
        self.usage_limit.as_ref()?.lock().unwrap_or_else(|p| p.into_inner()).get(&backend).copied().filter(|until| *until > Local::now())
    }

    fn prepare(&self, job: &Job) -> Result<team_core::project::Decision, LlmError> {
        self.prepare_with(job, |backend| match backend {
            Backend::Codex | Backend::OpenAi => team_core::codex::find_cli().is_some(),
            _ => team_core::claude::find_cli().is_some(),
        })
    }

    fn prepare_with(&self, job: &Job, installed: impl Fn(Backend) -> bool) -> Result<team_core::project::Decision, LlmError> {
        let decision = self.decision(job)?;
        let until = self.cooldown(decision.backend);
        if self.profiled(job) && (until.is_some() || !installed(decision.backend)) {
            if let Ok(mut fallback) = team_core::project::resolve_fallback(&self.cfg, &job.repo, job.policy.role) {
                if installed(fallback.backend) && self.cooldown(fallback.backend).is_none() {
                    Self::carry_env(&mut fallback.choice, &job.env);
                    return Ok(fallback);
                }
            }
            if until.is_none() { return Err(LlmError::Unavailable(format!("{}-CLI fehlt; kein verfügbares Ausweichmodell", decision.backend.label()))); }
        }
        if let Some(until) = until { return Err(LlmError::UsageLimit(decision.backend, until)); }
        Ok(decision)
    }

    /// Writes the hook settings next to the state and returns the runner.
    pub fn new(state_dir: &std::path::Path, logs_dir: PathBuf, inactivity: Duration, heartbeat: Option<Arc<Heartbeat>>, cfg: &TeamConfig) -> Self {
        let settings_file = state_dir.join("claude-settings.json");
        if std::fs::read_to_string(&settings_file).ok().as_deref() != Some(claude::HOOK_SETTINGS) {
            let _ = team_core::state::write_atomic(&settings_file, claude::HOOK_SETTINGS.as_bytes());
        }
        Self { cfg: cfg.clone(), settings_file, logs_dir, inactivity, heartbeat, role: String::new(), usage_limit: None }
    }
}

/// Dispatch without changing the session's tool boundary or normalized outcome.
pub fn run_session(backend: Backend, session: &Session<'_>) -> Result<Outcome, String> {
    match backend {
        Backend::Claude | Backend::Openrouter | Backend::Eco => claude::run(session).map_err(|e| e.to_string()),
        Backend::Codex => team_core::codex::run(session).map_err(|e| e.to_string()),
        Backend::OpenAi => team_core::codex::run_api(session),
    }
}

impl ClaudeCli {
    fn project_runner(&self, job: &Job) -> Runner {
        team_core::provider::repo_runner(&self.cfg, &job.repo)
    }

    #[cfg(test)]
    fn limited(&self, job: &Job) -> Option<DateTime<Local>> {
        let limit = self.usage_limit.as_ref()?.lock().unwrap_or_else(|p| p.into_inner());
        limit.get(&self.quota_backend(job)).copied().filter(|until| *until > Local::now())
    }

    /// Legacy Kimi sessions use the Claude executable but a separate API quota.
    fn quota_backend(&self, job: &Job) -> Backend {
        let backend = self.project_runner(job).backend;
        if backend == Backend::Claude && self.cfg.repo(&job.repo).is_some_and(|r| matches!(r.routing, team_core::config::Routing::Kimi | team_core::config::Routing::KimiOnly)) {
            if let Some(url) = job.env.set.get("ANTHROPIC_BASE_URL") {
                return if *url == self.cfg.providers.eco.base_url && job.env.set.get("ANTHROPIC_MODEL") == Some(&self.cfg.providers.eco.model) { Backend::Eco } else { Backend::Openrouter };
            }
        }
        backend
    }

    fn choice(&self, job: &Job) -> Result<team_core::provider::Choice, LlmError> {
        let Some(mut choice) = team_core::provider::runner_choice(&self.project_runner(job), &self.cfg.providers).map_err(LlmError::Failed)? else {
            // A Claude-only project must not inherit an endpoint from stale job state.
            let routing = self.cfg.repo(&job.repo).map(|r| r.routing).unwrap_or_default();
            if routing == team_core::config::Routing::Claude {
                let mut env = job.env.clone();
                let auth = ChildEnv::codex_subscription();
                env.set.retain(|key, _| !auth.remove.contains(key) && !key.starts_with("LUNIMA_CODEX_API_"));
                for key in auth.remove {
                    if !env.remove.contains(&key) { env.remove.push(key); }
                }
                return Ok(team_core::provider::Choice { model: job.model.clone(), env, why: "Claude-only project".into() });
            }
            return Ok(team_core::provider::Choice { model: job.model.clone(), env: job.env.clone(), why: "role policy".into() });
        };
        let auth = ChildEnv::codex_subscription().remove;
        for (key, value) in &job.env.set {
            if !auth.contains(key) && !key.starts_with("LUNIMA_CODEX_API_") && !choice.env.set.contains_key(key) {
                choice.env.set.insert(key.clone(), value.clone());
            }
        }
        for key in &job.env.remove {
            if !choice.env.remove.contains(key) {
                choice.env.remove.push(key.clone());
            }
        }
        Ok(choice)
    }

    fn settings(&self, choice: &team_core::provider::Choice) -> Result<PathBuf, LlmError> {
        if !choice.env.set.contains_key("ANTHROPIC_BASE_URL") {
            return Ok(self.settings_file.clone());
        }
        let mut settings: serde_json::Value = serde_json::from_str(claude::HOOK_SETTINGS).map_err(|e| LlmError::Failed(e.to_string()))?;
        if let Some(groups) = settings["hooks"]["PostToolUse"].as_array_mut() {
            for group in groups {
                if let Some(hooks) = group["hooks"].as_array_mut() {
                    for hook in hooks {
                        if hook["type"] == "agent" {
                            hook["model"] = choice.model.clone().into();
                        }
                    }
                }
            }
        }
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        choice.model.hash(&mut hash);
        let path = self.settings_file.with_file_name(format!("claude-settings-api-{:x}.json", hash.finish()));
        let text = serde_json::to_string(&settings).map_err(|e| LlmError::Failed(e.to_string()))?;
        if std::fs::read_to_string(&path).ok().as_deref() != Some(&text) {
            team_core::state::write_atomic(&path, text.as_bytes()).map_err(|e| LlmError::Failed(e.to_string()))?;
        }
        Ok(path)
    }

    fn deadline(&self, job: &Job) -> Option<Duration> {
        if self.profiled(job) {
            let budgets = team_core::project::budgets_for(&self.cfg, &job.repo);
            let complex = job.policy.labels.iter().any(|l| l.eq_ignore_ascii_case(&self.cfg.labels.complex));
            let limit = Duration::from_secs(if complex { budgets.timeout_minutes_complex } else { budgets.timeout_minutes_regular }.saturating_mul(60));
            return Some(job.max_duration.map_or(limit, |d| d.min(limit)));
        }
        job.max_duration.or_else(|| (matches!(self.project_runner(job).backend, Backend::Codex | Backend::OpenAi)).then_some(Duration::from_secs(2 * 60 * 60)))
    }
}

impl Llm for ClaudeCli {
    fn admit(&self, job: &Job) -> Result<(), LlmError> { self.prepare(job).map(|_| ()) }

    fn run(&self, job: &Job) -> Result<Outcome, LlmError> {
        let decision = self.prepare(job)?;
        let backend = decision.backend;
        let mut choice = decision.choice;
        if let Some(effort) = decision.effort {
            choice.env.set.insert("LUNIMA_CODEX_REASONING_EFFORT".into(), effort.as_str().into());
        }
        crate::log::info(&format!("{} · Modell {}", choice.why, if choice.model.is_empty() { "CLI-Standard" } else { &choice.model }));
        let settings_file = self.settings(&choice)?;
        let hb = self.heartbeat.clone();
        let tick = move || {
            if let Some(h) = &hb {
                h.touch();
            }
        };
        let log_file = self.logs_dir.join(format!("{}_{}.jsonl", Local::now().format("%Y-%m-%d_%H%M%S"), job.label));
        let inactivity = if self.profiled(job) { Duration::from_secs(team_core::project::budgets_for(&self.cfg, &job.repo).inactivity_minutes.max(1).saturating_mul(60)) } else { self.inactivity };
        let session = Session {
            prompt: job.prompt.clone(),
            cwd: job.cwd.clone(),
            model: choice.model,
            max_turns: job.max_turns,
            env: {
                let mut env = choice.env;
                if !self.role.is_empty() {
                    env.set.insert(team_core::chat::ENV_ROLE.into(), self.role.clone());
                }
                env
            },
            settings_file: Some(settings_file),
            log_file: Some(log_file),
            // long foreground commands are allowed up to the hard limit
            inactivity: job.max_duration.map_or(inactivity, |d| d.max(inactivity)),
            deadline: self.deadline(job),
            on_tick: Some(&tick),
            tools: job.tools.clone(),
            memory_guard: true,
        };
        let out = run_session(backend, &session).map_err(|e| LlmError::Failed(e.to_string()))?;
        if let Some(until) = out.usage_limit_until {
            return Err(LlmError::UsageLimit(backend, until));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_uses_runner_model_clean_auth_and_a_hard_deadline() {
        let t = tempfile::tempdir().unwrap();
        let mut cfg = TeamConfig::default();
        cfg.repos.push(team_core::config::RepoConfig { name: "o/r".into(), routing: team_core::config::Routing::Codex, ..Default::default() });
        let mut cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &cfg);
        let mut job = Job { policy: PolicyRequest::default(), repo: "o/r".into(), prompt: "test".into(), cwd: t.path().into(), model: "claude-premium".into(), max_turns: 1, env: ChildEnv::endpoint("https://example.invalid", "secret", "kimi"), label: "test".into(), max_duration: None, tools: Some(vec!["Read".into()]) };
        job.env.set.insert("LUNIMA_ROLE".into(), "product-discovery".into());
        assert_eq!(cli.choice(&job).unwrap().model, "");
        job.env.set.insert("GH_CONFIG_DIR".into(), "empty-gh".into());
        job.env.set.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        job.env.remove.extend(["GH_TOKEN".into(), "GITHUB_TOKEN".into(), "GH_ENTERPRISE_TOKEN".into()]);
        let env = cli.choice(&job).unwrap().env;
        assert_eq!(env.set.len(), 3);
        assert_eq!(env.set["GH_CONFIG_DIR"], "empty-gh");
        for key in ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN"] {
            assert!(env.remove.contains(&key.into()));
        }
        assert_eq!(cli.settings(&cli.choice(&job).unwrap()).unwrap(), cli.settings_file);
        assert_eq!(env.set["LUNIMA_ROLE"], "product-discovery");
        assert!(env.remove.contains(&"OPENAI_API_KEY".into()));
        assert_eq!(cli.deadline(&job), Some(Duration::from_secs(7200)));
        assert!(cli.settings_file.exists(), "Mixed projects can also need Claude hooks");
        job.max_duration = Some(Duration::from_secs(123));
        assert_eq!(cli.deadline(&job), job.max_duration);
        cli.cfg.repos[0].routing = team_core::config::Routing::Claude;
        assert_eq!(cli.choice(&job).unwrap().model, "claude-premium");
        assert!(!cli.choice(&job).unwrap().env.set.contains_key("ANTHROPIC_BASE_URL"));
        job.max_duration = None;
        assert_eq!(cli.deadline(&job), None);
    }

    #[test]
    fn company_job_ignores_global_api_and_stale_endpoint_environment() {
        let t = tempfile::tempdir().unwrap();
        let mut cfg = TeamConfig::default();
        cfg.runner.backend = Backend::Eco;
        let cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &cfg);
        let mut env = ChildEnv::endpoint("https://third-party.invalid", "must-not-leave", "kimi");
        env.set.insert("GH_CONFIG_DIR".into(), "sandbox".into());
        let job = Job { policy: PolicyRequest::default(), repo: "company/private".into(), prompt: String::new(), cwd: t.path().into(), model: "claude-test".into(), max_turns: 1, env, label: "test".into(), max_duration: None, tools: None };
        let choice = cli.choice(&job).unwrap();
        assert_eq!(cli.project_runner(&job).backend, Backend::Claude);
        assert_eq!(choice.model, "claude-test");
        assert_eq!(choice.env.set, std::collections::BTreeMap::from([("GH_CONFIG_DIR".into(), "sandbox".into())]));
        assert!(choice.env.remove.contains(&"ANTHROPIC_BASE_URL".into()));
    }

    #[test]
    fn project_decision_keeps_role_labels_sandbox_and_independent_deadlines() {
        let t = tempfile::tempdir().unwrap();
        let cfg: TeamConfig = toml::from_str("[[repos]]\nname='o/r'\nrouting='codex'\n[repos.profile]\nallowed_backends=['codex','claude']\n[repos.profile.standard]\nmodel='normal'\n[repos.profile.complex]\nmodel='complex-model'\neffort='high'\n[repos.profile.reviewer]\nbackend='claude'\nmodel='review-model'\n[repos.profile.budgets]\ntimeout_minutes_regular=3\ntimeout_minutes_complex=11\n").unwrap();
        let cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &cfg);
        let mut env = ChildEnv::endpoint("https://must-not-use.invalid", "secret", "kimi");
        env.set.insert("GH_CONFIG_DIR".into(), "empty-gh".into());
        env.set.insert("LUNIMA_CODEX_REASONING_EFFORT".into(), "low".into());
        env.remove.push("GH_TOKEN".into());
        let mut job = Job { policy: PolicyRequest { role: team_core::project::SessionRole::Coder, labels: vec!["complex".into()] }, repo: "o/r".into(), prompt: String::new(), cwd: t.path().into(), model: "stale-model".into(), max_turns: 1, env, label: "test".into(), max_duration: None, tools: Some(vec!["Read".into()]) };
        let decision = cli.decision(&job).unwrap();
        assert_eq!(decision.backend, Backend::Codex);
        assert_eq!(decision.choice.model, "complex-model");
        assert_eq!(decision.effort, Some(team_core::project::ReasoningEffort::High));
        assert_eq!(decision.choice.env.set["GH_CONFIG_DIR"], "empty-gh");
        assert!(decision.choice.env.remove.contains(&"GH_TOKEN".into()));
        assert!(!decision.choice.env.set.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!decision.choice.env.set.contains_key("LUNIMA_CODEX_REASONING_EFFORT"));
        assert_eq!(cli.deadline(&job), Some(Duration::from_secs(11 * 60)));
        job.policy.role = team_core::project::SessionRole::Reviewer;
        assert_eq!(cli.decision(&job).unwrap().choice.model, "review-model");
        job.policy.labels.clear();
        assert_eq!(cli.deadline(&job), Some(Duration::from_secs(3 * 60)));
        job.max_duration = Some(Duration::from_secs(20));
        assert_eq!(cli.deadline(&job), Some(Duration::from_secs(20)));
    }

    #[test]
    fn unavailable_model_is_admission_wait_and_allowed_fallback_obeys_its_own_quota() {
        let t = tempfile::tempdir().unwrap();
        let cfg: TeamConfig = toml::from_str("[[repos]]\nname='o/r'\nrouting='codex'\n[repos.profile]\nallowed_backends=['codex','claude']\n[repos.profile.fallback]\nbackend='claude'\nmodel='backup'\n").unwrap();
        let mut cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &cfg);
        let job = Job { policy: PolicyRequest::default(), repo: "o/r".into(), prompt: String::new(), cwd: t.path().into(), model: String::new(), max_turns: 1, env: ChildEnv::subscription(), label: "test".into(), max_duration: None, tools: None };
        let d = cli.prepare_with(&job, |b| b == Backend::Claude).unwrap();
        assert_eq!(d.backend, Backend::Claude);
        assert_eq!(d.choice.model, "backup");
        assert!(matches!(cli.prepare_with(&job, |_| false), Err(LlmError::Unavailable(_))));
        let until = Local::now() + chrono::Duration::minutes(5);
        cli.usage_limit = Some(Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::from([(Backend::Claude, until), (Backend::Codex, until)]))));
        assert!(matches!(cli.prepare_with(&job, |_| true), Err(LlmError::UsageLimit(Backend::Codex, _))));
    }

    #[test]
    fn quota_admission_uses_job_project_instead_of_chat_backend() {
        let t = tempfile::tempdir().unwrap();
        let mut cfg = TeamConfig::default();
        cfg.runner.backend = Backend::Eco;
        cfg.repos.push(team_core::config::RepoConfig { name: "o/codex".into(), routing: team_core::config::Routing::Codex, ..Default::default() });
        let mut cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &cfg);
        let until = Local::now() + chrono::Duration::minutes(5);
        cli.usage_limit = Some(Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::from([(Backend::Claude, until)]))));
        let mut job = Job { policy: PolicyRequest::default(), repo: "company/private".into(), prompt: String::new(), cwd: t.path().into(), model: String::new(), max_turns: 1, env: ChildEnv::subscription(), label: "test".into(), max_duration: None, tools: None };
        assert!(matches!(cli.run(&job), Err(LlmError::UsageLimit(Backend::Claude, t)) if t == until));
        job.repo = "o/codex".into();
        assert!(cli.limited(&job).is_none());
        cli.usage_limit.as_ref().unwrap().lock().unwrap().insert(Backend::Codex, until);
        assert_eq!(cli.limited(&job), Some(until));
        job.repo = "company/private".into();
        assert_eq!(cli.limited(&job), Some(until), "Codex cooldown must not overwrite Claude cooldown");
        cli.cfg.repos.push(team_core::config::RepoConfig { name: "o/kimi".into(), routing: team_core::config::Routing::KimiOnly, ..Default::default() });
        job.repo = "o/kimi".into();
        job.env = ChildEnv::endpoint(&cli.cfg.providers.openrouter.base_url, "test", &cli.cfg.providers.openrouter.model);
        assert!(cli.limited(&job).is_none(), "OpenRouter uses its own quota even through the Claude executable");
        assert_eq!(cli.quota_backend(&job), Backend::Openrouter);
    }
}

#[cfg(test)]
mod api_tests {
    use super::*;

    #[test]
    fn api_hook_uses_selected_model_and_discovery_env_survives_override() {
        let t = tempfile::tempdir().unwrap();
        let key = t.path().join("key");
        std::fs::write(&key, "test-key").unwrap();
        let mut cfg = TeamConfig::default();
        cfg.runner.backend = Backend::Openrouter;
        cfg.repos.push(team_core::config::RepoConfig { name: "o/r".into(), routing: team_core::config::Routing::Eco, ..Default::default() });
        cfg.providers.eco.key_file = key.display().to_string();
        cfg.providers.eco.model = "kimi-test".into();
        let cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &cfg);
        let mut env = ChildEnv::subscription();
        env.set.insert("GH_CONFIG_DIR".into(), "empty-gh".into());
        env.remove.push("GH_TOKEN".into());
        let job = Job { policy: PolicyRequest::default(), repo: "o/r".into(), prompt: String::new(), cwd: t.path().into(), model: "claude-test".into(), max_turns: 1, env, label: "test".into(), max_duration: None, tools: Some(vec!["Read".into()]) };
        let choice = cli.choice(&job).unwrap();
        assert_eq!(choice.model, "kimi-test");
        assert_eq!(choice.env.set["GH_CONFIG_DIR"], "empty-gh");
        assert_eq!(choice.env.set["ANTHROPIC_AUTH_TOKEN"], "test-key");
        assert!(choice.env.remove.contains(&"GH_TOKEN".into()));
        let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(cli.settings(&choice).unwrap()).unwrap()).unwrap();
        assert_eq!(settings["hooks"]["PostToolUse"][0]["hooks"][0]["model"], "kimi-test");
    }
}
