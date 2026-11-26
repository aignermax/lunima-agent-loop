//! `lunima-team-agent hook bash-guard`: a Claude Code PreToolUse hook for agent sessions.
//! Sessions with a CodeGraph index still searched code with `grep -r` / `rg` / `find -name`
//! (every call re-reads the whole context). The first recursive code search of a session in
//! an indexed worktree is refused once with a pointer to `codegraph_explore`; running it again
//! is allowed, since grep is right for literal text (log messages, strings).

use regex::Regex;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const REASON: &str = "This worktree has a CodeGraph index: find code with `codegraph_explore` (a question or the symbol/file names you know; it returns their source with callers and blast radius in one call). Use grep only for literal text such as log messages or strings: if that is what you need, run this command again; it is refused only once per session.";

/// Reads the hook input from stdin and prints a decision; always exits successfully, so a
/// broken input never blocks a session.
pub fn run() -> i32 {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Ok(v) = serde_json::from_str::<Value>(&input) else { return 0 };
    let command = v.pointer("/tool_input/command").and_then(Value::as_str).unwrap_or("");
    let cwd = v.get("cwd").and_then(Value::as_str).map(PathBuf::from).unwrap_or_default();
    let session = v.get("session_id").and_then(Value::as_str).unwrap_or("");
    if deny(command, &cwd, session, &std::env::temp_dir().join("lunima-bash-guard")) {
        println!("{}", json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": REASON}}));
    }
    0
}

/// True when this command is the session's first recursive code search in an indexed tree;
/// marks the session so the next one passes.
pub fn deny(command: &str, cwd: &Path, session: &str, state_dir: &Path) -> bool {
    if session.is_empty() || !is_code_search(command) || !indexed(cwd) {
        return false;
    }
    let mark = state_dir.join(session.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>());
    if mark.exists() {
        return false;
    }
    let _ = std::fs::create_dir_all(state_dir);
    std::fs::write(&mark, command).is_ok()
}

/// A recursive search through source: `grep -r`/`-R`, `rg`, `git grep`, `ag`, `find … -name`.
/// A search limited to docs or config files (`--include=*.md`, `-g '*.json'` …) is not one.
pub fn is_code_search(command: &str) -> bool {
    static SEARCH: OnceLock<Regex> = OnceLock::new();
    static DOCS: OnceLock<Regex> = OnceLock::new();
    let search = SEARCH.get_or_init(|| {
        Regex::new(r#"(?:^|[;&|(]\s*|\bxargs\s+)(?:grep\s+(?:[^|;&]*\s)?-[A-Za-z]*[rR]|rg\b|git\s+grep\b|ag\b|find\s+[^|;&]*-i?name\b)"#).expect("static regex")
    });
    let docs = DOCS.get_or_init(|| Regex::new(r#"(?:--include[= ]|-g\s+|--glob[= ])['"]?\*\.(?:md|txt|json|toml|ya?ml|ini|cfg|csv|lock)\b"#).expect("static regex"));
    search.is_match(command) && !docs.is_match(command)
}

/// The working directory or one of its parents up to the repository root holds `.codegraph/`.
fn indexed(cwd: &Path) -> bool {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".codegraph").is_dir() {
            return true;
        }
        if d.join(".git").exists() {
            return false;
        }
        dir = d.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_code_searches_are_recognised_and_literal_or_doc_searches_are_not() {
        for yes in ["grep -rn \"def run\" host/", "cd host && grep -R Plan .", "rg -n Plan host", "git grep -n stop_all", "find . -name \"*.py\" | head", "ls; grep -rln lab_mode tests | head"] {
            assert!(is_code_search(yes), "{yes}");
        }
        for no in ["grep -n Plan host/ls/lab_mode.py", "sed -n 1,80p host/ls/lab_mode.py", "git diff --stat", "grep -rn TODO --include=*.md docs", "rg -g '*.json' ports", "cat AGENTS.md", "pytest -q tests/ls"] {
            assert!(!is_code_search(no), "{no}");
        }
    }

    #[test]
    fn only_the_first_search_in_an_indexed_tree_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("wt");
        std::fs::create_dir_all(repo.join(".codegraph")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("host")).unwrap();
        let state = t.path().join("state");
        assert!(deny("grep -rn Plan .", &repo.join("host"), "s-1", &state), "first search, from a subfolder");
        assert!(!deny("grep -rn Plan .", &repo, "s-1", &state), "the repeat passes");
        assert!(deny("rg Plan", &repo, "s-2", &state), "per session");
        assert!(!deny("sed -n 1,9p x.py", &repo, "s-3", &state), "reading a known file is fine");
        let plain = t.path().join("plain");
        std::fs::create_dir_all(plain.join(".git")).unwrap();
        assert!(!deny("grep -rn Plan .", &plain, "s-4", &state), "no index, no nudge");
        assert!(!deny("grep -rn Plan .", &repo, "", &state), "no session id, no state");
    }
}
