//! Claude sessions as a trait (`ClaudeCli` = the real CLI, `fake::FakeLlm` in tests).

use chrono::{DateTime, Local};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use team_core::claude::{self, Outcome, Session};
use team_core::config::{Backend, Providers, Runner, TeamConfig};
use team_core::provider::ChildEnv;
use team_core::state::Heartbeat;

/// One Claude run.
#[derive(Debug, Clone)]
pub struct Job {
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
    /// the subscription's usage limit — nothing is wrong with the task; retry after this time
    UsageLimit(DateTime<Local>),
    Failed(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::UsageLimit(t) => write!(f, "Runner-Nutzungslimit erreicht bis {}", t.format("%d.%m. %H:%M")),
            LlmError::Failed(e) => write!(f, "{e}"),
        }
    }
}

pub trait Llm: Send + Sync {
    fn run(&self, job: &Job) -> Result<Outcome, LlmError>;
}

/// The Claude Code CLI with the bundled commit-comment hook; raw stream-json goes to
/// `<logs>/<date>_<label>.jsonl`, the role's heartbeat is refreshed while it runs.
pub struct ClaudeCli {
    pub runner: Runner,
    providers: Providers,
    pub settings_file: PathBuf,
    pub logs_dir: PathBuf,
    pub inactivity: Duration,
    pub heartbeat: Option<Arc<Heartbeat>>,
    /// the role whose sessions these are (`LUNIMA_ROLE` in their environment)
    pub role: String,
}

impl ClaudeCli {
    /// Writes the hook settings next to the state and returns the runner.
    pub fn new(state_dir: &std::path::Path, logs_dir: PathBuf, inactivity: Duration, heartbeat: Option<Arc<Heartbeat>>, cfg: &TeamConfig) -> Self {
        let runner = cfg.runner.clone();
        let settings_file = state_dir.join("claude-settings.json");
        if !matches!(runner.backend, Backend::Codex | Backend::OpenAi) && std::fs::read_to_string(&settings_file).ok().as_deref() != Some(claude::HOOK_SETTINGS) {
            let _ = team_core::state::write_atomic(&settings_file, claude::HOOK_SETTINGS.as_bytes());
        }
        Self { runner, providers: cfg.providers.clone(), settings_file, logs_dir, inactivity, heartbeat, role: String::new() }
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
    fn choice(&self, job: &Job) -> Result<team_core::provider::Choice, LlmError> {
        let Some(mut choice) = team_core::provider::runner_choice(&self.runner, &self.providers).map_err(LlmError::Failed)? else {
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
        if matches!(self.runner.backend, Backend::Codex | Backend::OpenAi) || !choice.env.set.contains_key("ANTHROPIC_BASE_URL") {
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
        job.max_duration.or_else(|| (matches!(self.runner.backend, Backend::Codex | Backend::OpenAi)).then_some(Duration::from_secs(2 * 60 * 60)))
    }
}

impl Llm for ClaudeCli {
    fn run(&self, job: &Job) -> Result<Outcome, LlmError> {
        let choice = self.choice(job)?;
        let settings_file = self.settings(&choice)?;
        let hb = self.heartbeat.clone();
        let tick = move || {
            if let Some(h) = &hb {
                h.touch();
            }
        };
        let log_file = self.logs_dir.join(format!("{}_{}.jsonl", Local::now().format("%Y-%m-%d_%H%M%S"), job.label));
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
            inactivity: job.max_duration.map_or(self.inactivity, |d| d.max(self.inactivity)),
            deadline: self.deadline(job),
            on_tick: Some(&tick),
            tools: job.tools.clone(),
            memory_guard: true,
        };
        let out = run_session(self.runner.backend, &session).map_err(|e| LlmError::Failed(e.to_string()))?;
        if let Some(until) = out.usage_limit_until {
            return Err(LlmError::UsageLimit(until));
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
        let mut cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &TeamConfig { runner: Runner { backend: Backend::Codex, model: String::new() }, ..Default::default() });
        let mut job = Job { prompt: "test".into(), cwd: t.path().into(), model: "claude-premium".into(), max_turns: 1, env: ChildEnv::endpoint("https://example.invalid", "secret", "kimi"), label: "test".into(), max_duration: None, tools: Some(vec!["Read".into()]) };
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
        assert!(!cli.settings_file.exists(), "Codex does not install Claude hooks");
        job.max_duration = Some(Duration::from_secs(123));
        assert_eq!(cli.deadline(&job), job.max_duration);
        cli.runner.backend = Backend::Claude;
        assert_eq!(cli.choice(&job).unwrap().model, "claude-premium");
        assert_eq!(cli.choice(&job).unwrap().env, job.env);
        job.max_duration = None;
        assert_eq!(cli.deadline(&job), None);
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
        cfg.providers.openrouter.key_file = key.display().to_string();
        cfg.providers.openrouter.model = "kimi-test".into();
        let cli = ClaudeCli::new(t.path(), t.path().join("logs"), Duration::from_secs(60), None, &cfg);
        let mut env = ChildEnv::subscription();
        env.set.insert("GH_CONFIG_DIR".into(), "empty-gh".into());
        env.remove.push("GH_TOKEN".into());
        let job = Job { prompt: String::new(), cwd: t.path().into(), model: "claude-test".into(), max_turns: 1, env, label: "test".into(), max_duration: None, tools: Some(vec!["Read".into()]) };
        let choice = cli.choice(&job).unwrap();
        assert_eq!(choice.model, "kimi-test");
        assert_eq!(choice.env.set["GH_CONFIG_DIR"], "empty-gh");
        assert_eq!(choice.env.set["ANTHROPIC_AUTH_TOKEN"], "test-key");
        assert!(choice.env.remove.contains(&"GH_TOKEN".into()));
        let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(cli.settings(&choice).unwrap()).unwrap()).unwrap();
        assert_eq!(settings["hooks"]["PostToolUse"][0]["hooks"][0]["model"], "kimi-test");
    }
}
