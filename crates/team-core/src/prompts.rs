//! Prompts: built-ins compiled in (identical texts to the Python agent), overridable by
//! `<data>/prompts/<name>.md`. Rendering follows Python's `str.format` rules for the
//! subset the templates use (`{name}` placeholders, `{{` / `}}` literal braces), so
//! prompts written for the Python agent keep working. A broken override falls back to
//! the built-in instead of failing the run.

use std::collections::HashMap;
use std::path::Path;

pub const BUILTIN: &[(&str, &str)] = &[
    ("coder-initial", include_str!("../prompts/coder-initial.md")),
    ("coder-continuation", include_str!("../prompts/coder-continuation.md")),
    ("coder-retry", include_str!("../prompts/coder-retry.md")),
    ("reviewer", include_str!("../prompts/reviewer.md")),
    ("qa-review", include_str!("../prompts/qa-review.md")),
    ("qa-fix", include_str!("../prompts/qa-fix.md")),
    ("pr-feedback", include_str!("../prompts/pr-feedback.md")),
    ("po-owner", include_str!("../prompts/po-owner.md")),
];

pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTIN.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

/// `str.format`-style rendering. Errors on unknown placeholders and unbalanced braces.
pub fn format(template: &str, values: &HashMap<&str, String>) -> Result<String, String> {
    let mut out = String::with_capacity(template.len() + 256);
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '}' => return Err("einzelne „}“".into()),
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => name.push(ch),
                        None => return Err("„{“ ohne schließende Klammer".into()),
                    }
                }
                let key = name.split([':', '!']).next().unwrap_or("");
                let value = values.get(key).ok_or_else(|| format!("unbekannter Platzhalter {{{name}}}"))?;
                out.push_str(value);
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// The override for `name` if present and renderable, else the built-in. The second
/// value names the problem with a broken override (for the log).
pub fn render(prompts_dir: &Path, name: &str, values: &HashMap<&str, String>) -> Result<(String, Option<String>), String> {
    let builtin = builtin(name).ok_or_else(|| format!("kein Prompt „{name}“"))?;
    let path = prompts_dir.join(format!("{name}.md"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        match format(&text.replace('\r', ""), values) {
            Ok(out) => return Ok((out, None)),
            Err(e) => {
                let warning = format!("Prompt-Override {} unbrauchbar ({e}) — eingebaute Fassung verwendet", path.display());
                return format(builtin, values).map(|out| (out, Some(warning)));
            }
        }
    }
    format(builtin, values).map(|out| (out, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(pairs: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn python_format_semantics() {
        let v = vals(&[("n", "7")]);
        assert_eq!(format("Issue #{n} {{\"x\": 1}}", &v).unwrap(), "Issue #7 {\"x\": 1}");
        assert!(format("{missing}", &v).is_err());
        assert!(format("a } b", &v).is_err());
        assert!(format("a { b", &v).is_err());
    }

    #[test]
    fn override_wins_and_broken_override_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let v: HashMap<&str, String> = HashMap::new();
        std::fs::write(tmp.path().join("po-owner.md"), "custom").unwrap();
        // po-owner uses the loop's {REPO}-style placeholders; an override without any renders as-is
        assert_eq!(render(tmp.path(), "po-owner", &v).unwrap().0, "custom");
        std::fs::write(tmp.path().join("qa-fix.md"), "broken {nope").unwrap();
        let err = render(tmp.path(), "qa-fix", &v);
        // the built-in needs values we didn't pass → error, but never the broken override's
        assert!(err.is_err() || err.unwrap().1.is_some());
    }

    #[test]
    fn every_builtin_is_present_and_balanced() {
        for (name, text) in BUILTIN {
            assert!(!text.trim().is_empty(), "{name}");
            // collect placeholder names, then render with dummies — must succeed
            let mut keys = Vec::new();
            let mut rest = *text;
            while let Some(i) = rest.find('{') {
                if rest[i + 1..].starts_with('{') {
                    rest = &rest[i + 2..];
                    continue;
                }
                let end = rest[i..].find('}').unwrap() + i;
                keys.push(rest[i + 1..end].split([':', '!']).next().unwrap().to_string());
                rest = &rest[end + 1..];
            }
            let leaked: Vec<&'static str> = keys.iter().map(|k| &*Box::leak(k.clone().into_boxed_str())).collect();
            let v: HashMap<&str, String> = leaked.iter().map(|k| (*k, "x".to_string())).collect();
            format(text, &v).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}
