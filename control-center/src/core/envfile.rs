//! A `.env` file edited in place: comments, blank lines and order survive; only the
//! changed `KEY=value` lines are rewritten. Secrets are never shown.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct EnvFile {
    pub path: PathBuf,
    lines: Vec<String>,
}

/// Keys whose values must never appear in the UI.
pub fn is_secret(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD"].iter().any(|s| k.contains(s)) && !k.ends_with("_FILE")
}

fn split(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let t = t.strip_prefix("export ").unwrap_or(t);
    let (k, v) = t.split_once('=')?;
    let k = k.trim();
    (!k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some((k, v))
}

/// Value as python-dotenv reads it: quotes removed, ` #comment` dropped for unquoted values.
fn clean_value(raw: &str) -> String {
    let v = raw.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|r| r.split_once(q)).map(|(inner, _)| inner) {
            return inner.to_string();
        }
    }
    v.split(" #").next().unwrap_or("").trim().to_string()
}

impl EnvFile {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { path: path.to_path_buf(), lines: text.lines().map(String::from).collect() })
    }

    /// Last assignment wins (like python-dotenv with override).
    pub fn get(&self, key: &str) -> Option<String> {
        self.lines.iter().rev().find_map(|l| split(l).filter(|(k, _)| *k == key).map(|(_, v)| clean_value(v)))
    }

    /// Display value: masked for secrets, "—" when unset.
    pub fn display(&self, key: &str) -> String {
        match self.get(key) {
            Some(v) if is_secret(key) && !v.is_empty() => "•••••• (gesetzt)".into(),
            Some(v) if !v.is_empty() => v,
            _ => "—".into(),
        }
    }

    /// Sets (or removes, for an empty value) a key; replaces the last assignment in place.
    pub fn set(&mut self, key: &str, value: &str) {
        let idx = self.lines.iter().rposition(|l| split(l).is_some_and(|(k, _)| k == key));
        let line = format!("{key}={value}");
        match (idx, value.is_empty()) {
            (Some(i), true) => {
                self.lines.remove(i);
            }
            (Some(i), false) => self.lines[i] = line,
            (None, false) => self.lines.push(line),
            (None, true) => {}
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("env.tmp");
        std::fs::write(&tmp, self.lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Comma-separated list value (repos, labels).
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key).unwrap_or_default().split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(text: &str) -> (tempfile::TempDir, EnvFile) {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(".env");
        std::fs::write(&p, text).unwrap();
        let e = EnvFile::load(&p).unwrap();
        (tmp, e)
    }

    #[test]
    fn reads_like_dotenv() {
        let (_t, e) = env("# c\nA=1\nB=\"two words\" # x\nC=plain # comment\nexport D='q'\nA=last\n");
        assert_eq!(e.get("A").as_deref(), Some("last"));
        assert_eq!(e.get("B").as_deref(), Some("two words"));
        assert_eq!(e.get("C").as_deref(), Some("plain"));
        assert_eq!(e.get("D").as_deref(), Some("q"));
        assert_eq!(e.get("NOPE"), None);
    }

    #[test]
    fn set_keeps_comments_and_order() {
        let (_t, mut e) = env("# header\nA=1\n\n# models\nB=2\n");
        e.set("B", "3");
        e.set("NEW", "x");
        e.set("A", "");
        e.save().unwrap();
        let text = std::fs::read_to_string(&e.path).unwrap();
        assert_eq!(text, "# header\n\n# models\nB=3\nNEW=x\n");
    }

    #[test]
    fn secrets_are_masked() {
        let (_t, e) = env("ANTHROPIC_API_KEY=sk-123\nAGENT_OPENROUTER_KEY_FILE=/x\nAGENT_CODER_MODEL=m\n");
        assert!(is_secret("GITHUB_TOKEN"));
        assert!(!e.display("ANTHROPIC_API_KEY").contains("sk-"));
        assert_eq!(e.display("AGENT_OPENROUTER_KEY_FILE"), "/x");
        assert_eq!(e.display("AGENT_CODER_MODEL"), "m");
        assert_eq!(e.display("MISSING"), "—");
    }

    #[test]
    fn list_values() {
        let (_t, e) = env("AGENT_REPOS=a/b, c/d ,\n");
        assert_eq!(e.list("AGENT_REPOS"), ["a/b", "c/d"]);
    }
}
