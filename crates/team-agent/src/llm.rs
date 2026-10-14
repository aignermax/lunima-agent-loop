//! Claude sessions as a trait (`ClaudeCli` = the real CLI, `fake::FakeLlm` in tests).

use chrono::{DateTime, Local};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use team_core::claude::{self, Outcome, Session};
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
            LlmError::UsageLimit(t) => write!(f, "Claude-Nutzungslimit erreicht bis {}", t.format("%d.%m. %H:%M")),
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
    pub settings_file: PathBuf,
    pub logs_dir: PathBuf,
    pub inactivity: Duration,
    pub heartbeat: Option<Arc<Heartbeat>>,
}

impl ClaudeCli {
    /// Writes the hook settings next to the state and returns the runner.
    pub fn new(state_dir: &std::path::Path, logs_dir: PathBuf, inactivity: Duration, heartbeat: Option<Arc<Heartbeat>>) -> Self {
        let settings_file = state_dir.join("claude-settings.json");
        if std::fs::read_to_string(&settings_file).ok().as_deref() != Some(claude::HOOK_SETTINGS) {
            let _ = team_core::state::write_atomic(&settings_file, claude::HOOK_SETTINGS.as_bytes());
        }
        Self { settings_file, logs_dir, inactivity, heartbeat }
    }
}

impl Llm for ClaudeCli {
    fn run(&self, job: &Job) -> Result<Outcome, LlmError> {
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
            model: job.model.clone(),
            max_turns: job.max_turns,
            env: job.env.clone(),
            settings_file: Some(self.settings_file.clone()),
            log_file: Some(log_file),
            inactivity: self.inactivity,
            on_tick: Some(&tick),
        };
        let out = claude::run(&session).map_err(|e| LlmError::Failed(e.to_string()))?;
        if let Some(until) = out.usage_limit_until {
            return Err(LlmError::UsageLimit(until));
        }
        Ok(out)
    }
}
