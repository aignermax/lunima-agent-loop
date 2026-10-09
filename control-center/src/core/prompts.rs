//! Every prompt the team runs on: the PO loop's (built into the CLI, override in
//! `<loop>/prompts/`) and the issue agent's (built-in `<agent>/src/prompts/`, override in
//! `<agent>/prompts/`). An override file always wins; deleting it restores the built-in.

use std::path::{Path, PathBuf};

const LOOP_OWNER: &str = include_str!("../../../prompts/owner.md");
const LOOP_WORKER: &str = include_str!("../../../prompts/worker.md");

/// Where the built-in text comes from.
#[derive(Debug, Clone)]
pub enum Builtin {
    /// compiled into this app (the loop's prompts)
    Embedded(&'static str),
    /// a file in the issue agent's checkout
    File(PathBuf),
}

#[derive(Debug, Clone)]
pub struct PromptDoc {
    pub id: String,
    pub role: &'static str,
    pub title: &'static str,
    pub builtin: Builtin,
    pub override_path: PathBuf,
    /// The override file may be deleted. False in a checkout of the loop repo, where
    /// prompts/*.md are the tracked sources of the built-in prompts themselves.
    pub deletable: bool,
}

impl PromptDoc {
    pub fn builtin_text(&self) -> String {
        match &self.builtin {
            Builtin::Embedded(t) => (*t).to_string(),
            Builtin::File(p) => std::fs::read_to_string(p).unwrap_or_default(),
        }
    }

    pub fn override_text(&self) -> Option<String> {
        std::fs::read_to_string(&self.override_path).ok()
    }

    /// An override is in effect that differs from the built-in.
    pub fn is_customised(&self) -> bool {
        self.override_text().is_some_and(|o| normalise(&o) != normalise(&self.builtin_text()))
    }
}

/// Line endings and trailing whitespace don't make a prompt "different" (git may check
/// the sources out with CRLF on Windows).
fn normalise(text: &str) -> String {
    text.replace('\r', "").trim_end().to_string()
}

/// (role, title, issue-agent prompt name)
const AGENT_PROMPTS: &[(&str, &str, &str)] = &[
    ("Coder", "Neues Issue umsetzen", "coder-initial"),
    ("Coder", "Arbeit fortsetzen", "coder-continuation"),
    ("Coder", "Review-Befunde einarbeiten", "coder-retry"),
    ("Reviewer", "PR prüfen", "reviewer"),
    ("QA-Tester", "QA-Review", "qa-review"),
    ("QA-Tester", "QA-Fehler beheben", "qa-fix"),
    ("PR-Feedback", "Kommentar umsetzen", "pr-feedback"),
];

/// All editable prompts; the issue agent's only when it is connected.
pub fn all(loop_root: &Path, agent_dir: Option<&Path>) -> Vec<PromptDoc> {
    let loop_checkout = loop_root.join("lunima-agent-loop.csproj").is_file();
    let mut docs = vec![
        PromptDoc { id: "loop/owner".into(), role: "Product Owner", title: "PO-Lauf", builtin: Builtin::Embedded(LOOP_OWNER), override_path: loop_root.join("prompts").join("owner.md"), deletable: !loop_checkout },
        PromptDoc { id: "loop/worker".into(), role: "Loop-Worker", title: "Issue umsetzen (Kimi)", builtin: Builtin::Embedded(LOOP_WORKER), override_path: loop_root.join("prompts").join("worker.md"), deletable: !loop_checkout },
    ];
    if let Some(dir) = agent_dir {
        docs.extend(AGENT_PROMPTS.iter().map(|(role, title, name)| PromptDoc {
            id: format!("agent/{name}"),
            role,
            title,
            builtin: Builtin::File(dir.join("src").join("prompts").join(format!("{name}.md"))),
            override_path: dir.join("prompts").join(format!("{name}.md")),
            deletable: true,
        }));
    }
    docs
}

/// Placeholders a prompt uses: `{NAME}` / `{name}`, skipping `{{escaped}}` braces.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' && chars.get(i + 1) == Some(&'{') {
            i += 2;
            continue;
        }
        if chars[i] == '{' {
            let end = chars[i + 1..].iter().position(|c| *c == '}').map(|p| i + 1 + p);
            if let Some(end) = end {
                let name: String = chars[i + 1..end].iter().collect();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !out.contains(&name) {
                    out.push(name);
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// What Python's `str.format` would reject in an issue-agent prompt: unbalanced braces and
/// fields that aren't known placeholders (a single-brace `{"json": 1}` is such a field).
/// The agent falls back to its built-in prompt in that case, silently ignoring the edit.
pub fn format_problems(text: &str, known: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        let (c, after) = (rest.as_bytes()[i], &rest[i + 1..]);
        if after.starts_with(c as char) {
            rest = &after[1..]; // `{{` / `}}` are literal braces
            continue;
        }
        if c == b'}' {
            problems.push("einzelne „}“ (für eine echte Klammer „}}“ schreiben)".to_string());
            rest = after;
            continue;
        }
        let Some(end) = after.find('}') else {
            problems.push("„{“ ohne schließende Klammer".to_string());
            break;
        };
        let field = &after[..end];
        let name = field.split(['!', ':', '.', '[']).next().unwrap_or("");
        if !known.iter().any(|k| k == name) {
            problems.push(format!("unbekannter Platzhalter „{{{field}}}“"));
        }
        rest = &after[end + 1..];
    }
    problems.dedup();
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_problems_match_python_format() {
        let known = vec!["pr_number".to_string()];
        assert!(format_problems("PR #{pr_number} {{\"literal\": 1}}", &known).is_empty());
        assert_eq!(format_problems(r#"Antworte mit {"verdict": "OK"}"#, &known).len(), 1);
        assert!(!format_problems("a } b", &known).is_empty());
        assert!(!format_problems("a { b", &known).is_empty());
        assert!(format_problems("{pr_number:>5}", &known).is_empty());
    }

    #[test]
    fn placeholders_skip_escaped_braces_and_json() {
        let t = "Issue #{issue_number}: {issue_title}\n{{\"verdict\": \"OK\"}}\n{REPO} {issue_number}";
        assert_eq!(placeholders(t), ["issue_number", "issue_title", "REPO"]);
    }

    #[test]
    fn agent_prompts_only_when_connected() {
        let root = Path::new("loop");
        assert_eq!(all(root, None).len(), 2);
        let docs = all(root, Some(Path::new("agent")));
        assert_eq!(docs.len(), 2 + AGENT_PROMPTS.len());
        let reviewer = docs.iter().find(|d| d.id == "agent/reviewer").unwrap();
        assert_eq!(reviewer.override_path, Path::new("agent").join("prompts").join("reviewer.md"));
    }

    #[test]
    fn loop_checkout_sources_are_never_deleted_and_identical_copy_is_not_custom() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("lunima-agent-loop.csproj"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("prompts")).unwrap();
        std::fs::write(tmp.path().join("prompts/owner.md"), LOOP_OWNER.replace('\n', "\r\n")).unwrap();
        let owner = all(tmp.path(), None).into_iter().next().unwrap();
        assert!(!owner.deletable);
        assert!(!owner.is_customised(), "same text with CRLF is still the built-in");
        std::fs::write(tmp.path().join("prompts/owner.md"), "changed").unwrap();
        assert!(owner.is_customised());
    }

    #[test]
    fn override_beats_builtin_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src/prompts")).unwrap();
        std::fs::write(tmp.path().join("src/prompts/qa-fix.md"), "built-in {pr_number}").unwrap();
        let doc = all(Path::new("loop"), Some(tmp.path())).into_iter().find(|d| d.id == "agent/qa-fix").unwrap();
        assert_eq!(doc.builtin_text(), "built-in {pr_number}");
        assert_eq!(doc.override_text(), None);
    }
}
