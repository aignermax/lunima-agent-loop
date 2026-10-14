//! Every prompt the team runs on: built into the team agent (team_core::prompts::BUILTIN),
//! overridable by `<data>/prompts/<name>.md`. An override file always wins; deleting it
//! restores the built-in. All prompts use Python `str.format` placeholders.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PromptDoc {
    /// built-in name, e.g. `coder-initial`
    pub id: String,
    pub role: &'static str,
    pub title: &'static str,
    pub builtin: &'static str,
    pub override_path: PathBuf,
}

impl PromptDoc {
    pub fn builtin_text(&self) -> String {
        self.builtin.to_string()
    }

    pub fn override_text(&self) -> Option<String> {
        std::fs::read_to_string(&self.override_path).ok()
    }

    /// An override is in effect that differs from the built-in.
    pub fn is_customised(&self) -> bool {
        self.override_text().is_some_and(|o| normalise(&o) != normalise(self.builtin))
    }
}

/// Line endings and trailing whitespace don't make a prompt "different".
fn normalise(text: &str) -> String {
    text.replace('\r', "").trim_end().to_string()
}

/// (prompt name, role, title) in display order.
const TITLES: &[(&str, &str, &str)] = &[
    ("po-owner", "Product Owner", "PO-Lauf"),
    ("coder-initial", "Coder", "Neues Issue umsetzen"),
    ("coder-continuation", "Coder", "Arbeit fortsetzen"),
    ("coder-retry", "Coder", "Review-Befunde einarbeiten"),
    ("reviewer", "Reviewer", "PR prüfen"),
    ("qa-review", "QA-Tester", "QA-Review"),
    ("qa-fix", "QA-Tester", "QA-Fehler beheben"),
    ("pr-feedback", "PR-Feedback", "Kommentar umsetzen"),
];

/// All editable prompts: the known ones in display order, then any newer built-in.
pub fn all(prompts_dir: &Path) -> Vec<PromptDoc> {
    let doc = |name: &str, role: &'static str, title: &'static str, text: &'static str| PromptDoc {
        id: name.to_string(),
        role,
        title,
        builtin: text,
        override_path: prompts_dir.join(format!("{name}.md")),
    };
    let mut docs: Vec<PromptDoc> = TITLES.iter().filter_map(|(n, r, t)| team_core::prompts::builtin(n).map(|text| doc(n, r, t, text))).collect();
    for (name, text) in team_core::prompts::BUILTIN {
        if !TITLES.iter().any(|(n, _, _)| n == name) {
            docs.push(doc(name, "Weitere", name, text));
        }
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
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '}').map(|p| i + 1 + p) {
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

/// What Python's `str.format` would reject: unbalanced braces and fields that aren't known
/// placeholders (a single-brace `{"json": 1}` is such a field). The agent falls back to its
/// built-in prompt in that case, silently ignoring the edit.
pub fn format_problems(text: &str, known: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        let (c, after) = (rest.as_bytes()[i], &rest[i + 1..]);
        if after.starts_with(c as char) {
            rest = &after[1..];
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
    fn every_builtin_is_editable_and_po_comes_first() {
        let docs = all(Path::new("P"));
        assert_eq!(docs.len(), team_core::prompts::BUILTIN.len());
        assert_eq!(docs[0].id, "po-owner");
        let reviewer = docs.iter().find(|d| d.id == "reviewer").unwrap();
        assert_eq!(reviewer.override_path, Path::new("P").join("reviewer.md"));
        for d in &docs {
            assert!(format_problems(d.builtin, &placeholders(d.builtin)).is_empty(), "built-in {} must format cleanly", d.id);
        }
    }

    #[test]
    fn identical_override_is_not_custom() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = all(tmp.path()).into_iter().find(|d| d.id == "qa-fix").unwrap();
        assert!(!doc.is_customised());
        std::fs::write(&doc.override_path, doc.builtin.replace('\n', "\r\n")).unwrap();
        assert!(!doc.is_customised(), "same text with CRLF is still the built-in");
        std::fs::write(&doc.override_path, "changed").unwrap();
        assert!(doc.is_customised());
    }
}
