//! `.agent.toml` — a repository's opt-in and commands for the agent roles:
//!
//! ```toml
//! agents_enabled = ["coder", "qa", "pr-feedback"]
//! build_cmd = "dotnet build"
//! test_cmd = "dotnet test"
//! ui_test_cmd = ""
//! command_timeout_sec = 1800
//! qa_review_enabled = true
//! ```

use std::path::Path;
use toml::Value;

pub const FILE: &str = ".agent.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectConfig {
    pub build_cmd: String,
    pub test_cmd: String,
    pub ui_test_cmd: String,
    pub agents_enabled: Vec<String>,
    pub command_timeout_sec: u64,
    pub qa_review_enabled: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self { build_cmd: String::new(), test_cmd: String::new(), ui_test_cmd: String::new(), agents_enabled: vec!["coder".into()], command_timeout_sec: 1800, qa_review_enabled: false }
    }
}

impl ProjectConfig {
    pub fn enabled(&self, role: &str) -> bool {
        self.agents_enabled.iter().any(|r| r == role)
    }

    /// Missing file → defaults.
    pub fn load(dir: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(dir.join(FILE)) {
            Ok(t) => Self::parse(&t),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Broken TOML is an error (the Python agent silently fell back to defaults, which made a
    /// typo look like "QA disabled").
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut c = Self::default();
        if text.trim().is_empty() {
            return Ok(c);
        }
        let v: toml::Table = text.parse().map_err(|e| format!("{FILE}: {e}"))?;
        let s = |k: &str| v.get(k).map(as_text).unwrap_or_default();
        c.build_cmd = s("build_cmd");
        c.test_cmd = s("test_cmd");
        c.ui_test_cmd = s("ui_test_cmd");
        if let Some(list) = v.get("agents_enabled").map(as_list).filter(|l| !l.is_empty()) {
            c.agents_enabled = list;
        }
        match v.get("command_timeout_sec") {
            Some(Value::Integer(n)) if *n > 0 => c.command_timeout_sec = *n as u64,
            Some(Value::String(t)) => c.command_timeout_sec = t.trim().parse().map_err(|_| format!("{FILE}: command_timeout_sec ist keine Zahl"))?,
            Some(other) => return Err(format!("{FILE}: command_timeout_sec ungültig: {other}")),
            None => {}
        }
        c.qa_review_enabled = match v.get("qa_review_enabled") {
            Some(Value::Boolean(b)) => *b,
            Some(Value::String(t)) => t.trim().eq_ignore_ascii_case("true"),
            _ => false,
        };
        Ok(c)
    }
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        other => other.to_string().trim().to_string(),
    }
}

fn as_list(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        Value::Array(a) => a.iter().map(as_text).filter(|x| !x.is_empty()).collect(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_parsing() {
        assert_eq!(ProjectConfig::parse("").unwrap(), ProjectConfig::default());
        let c = ProjectConfig::parse("agents_enabled = \"coder, qa\"\nbuild_cmd = \" dotnet build \"\ncommand_timeout_sec = 60\nqa_review_enabled = \"false\"").unwrap();
        assert!(c.enabled("qa") && !c.enabled("pr-feedback"));
        assert_eq!(c.build_cmd, "dotnet build");
        assert_eq!(c.command_timeout_sec, 60);
        assert!(!c.qa_review_enabled, "the string \"false\" must not enable the review");
        let c = ProjectConfig::parse("agents_enabled = [\"qa\", \"pr-feedback\"]\nqa_review_enabled = true").unwrap();
        assert_eq!(c.agents_enabled, vec!["qa", "pr-feedback"]);
        assert!(c.qa_review_enabled);
        assert_eq!(ProjectConfig::parse("agents_enabled = []").unwrap().agents_enabled, vec!["coder"]);
    }

    #[test]
    fn broken_toml_is_an_error() {
        assert!(ProjectConfig::parse("agents_enabled = [").is_err());
        assert!(ProjectConfig::parse("command_timeout_sec = \"soon\"").is_err());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(ProjectConfig::load(t.path()).unwrap(), ProjectConfig::default());
    }
}
