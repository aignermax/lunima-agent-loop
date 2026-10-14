//! Everything a role needs for one cycle: configuration, folders, GitHub, Claude, heartbeat.

use crate::forge::Forge;
use crate::llm::{Llm, LlmError};
use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use team_core::config::TeamConfig;
use team_core::paths::Paths;
use team_core::state::Heartbeat;

pub type ForgeFactory = Arc<dyn Fn(&str) -> Arc<dyn Forge> + Send + Sync>;
pub type RemoteFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone)]
pub struct Ctx {
    pub cfg: TeamConfig,
    pub paths: Paths,
    /// this computer (claim comments, history)
    pub machine: String,
    /// the gh login (assignee of claimed issues)
    pub login: String,
    pub llm: Arc<dyn Llm>,
    pub heartbeat: Arc<Heartbeat>,
    pub forge: ForgeFactory,
    /// clone URL of a repository (GitHub; a local path in tests)
    pub remote: RemoteFn,
    pub tools: Tools,
}

impl Ctx {
    pub fn render(&self, name: &str, values: &HashMap<&str, String>) -> Result<String, String> {
        let (text, warning) = team_core::prompts::render(&self.paths.prompts(), name, values)?;
        if let Some(w) = warning {
            crate::log::warn(&format!("Prompt-Override {name}: {w} — Standardtext verwendet"));
        }
        Ok(text)
    }
    pub fn logs(&self) -> PathBuf {
        self.paths.logs()
    }
}

/// python-dev-tools (`~/.cap-tools`) as referenced by the prompts (`<TOOLS_DIR>`, `<TOOLS_PYTHON>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Tools {
    pub dir: String,
    pub python: String,
}

impl Default for Tools {
    fn default() -> Self {
        Self { dir: "tools".into(), python: if cfg!(windows) { "python".into() } else { "python3".into() } }
    }
}

impl Tools {
    pub const REQUIRED: &'static [&'static str] = &["semantic_search.py", "smart_test.py", "build_errors.py", "find_symbol.py", "dotnet_deps.py"];

    /// `~/.cap-tools` when complete (venv python if present), else defaults.
    pub fn detect(home: Option<PathBuf>) -> Self {
        let Some(dir) = home.map(|h| h.join(".cap-tools")) else { return Self::default() };
        if !Self::REQUIRED.iter().all(|f| dir.join(f).is_file()) {
            return Self::default();
        }
        let venv = if cfg!(windows) { dir.join("venv").join("Scripts").join("python.exe") } else { dir.join("venv").join("bin").join("python3") };
        let python = if venv.is_file() { venv.display().to_string() } else { Self::default().python };
        Self { dir: dir.display().to_string(), python }
    }

    pub fn insert(&self, values: &mut HashMap<&str, String>) {
        values.insert("tools_dir", self.dir.clone());
        values.insert("tools_python", self.python.clone());
    }
}

/// Why a cycle ended early.
#[derive(Debug, Clone, PartialEq)]
pub enum RoleError {
    /// subscription limit — the daemon pauses the role until then
    UsageLimit(DateTime<Local>),
    Failed(String),
}

impl std::fmt::Display for RoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleError::UsageLimit(t) => write!(f, "{}", LlmError::UsageLimit(*t)),
            RoleError::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl From<String> for RoleError {
    fn from(e: String) -> Self {
        RoleError::Failed(e)
    }
}

impl From<LlmError> for RoleError {
    fn from(e: LlmError) -> Self {
        match e {
            LlmError::UsageLimit(t) => RoleError::UsageLimit(t),
            LlmError::Failed(m) => RoleError::Failed(m),
        }
    }
}

pub type RoleResult<T = ()> = Result<T, RoleError>;
