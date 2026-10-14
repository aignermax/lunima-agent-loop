//! The reviewer's result block and the coder's test gate.
//!
//! ```text
//! === REVIEW RESULT ===
//! VERDICT: OK | BLOCKING
//! SUMMARY: one line
//! === FINDINGS ===
//! - [MAJOR] text
//! === END ===
//! ```

use crate::agent_toml::ProjectConfig;
use crate::shell::{run_shell, tail};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub verdict: String,
    pub summary: String,
    /// (SEVERITY, text)
    pub findings: Vec<(String, String)>,
}

/// Verdicts that let a PR pass. Anything else blocks — the Python agent let every verdict
/// except the literal `BLOCKING` pass, so a garbled `VERDICT: FAIL` approved the PR.
const PASSING: &[&str] = &["OK", "PASS", "PASSED", "APPROVE", "APPROVED"];

impl Verdict {
    pub fn blocking(summary: &str) -> Self {
        Self { verdict: "BLOCKING".into(), summary: summary.into(), findings: vec![] }
    }
    pub fn is_blocking(&self) -> bool {
        !PASSING.contains(&self.verdict.as_str())
    }
    pub fn findings_text(&self) -> String {
        self.findings.iter().map(|(s, t)| format!("- [{s}] {t}")).collect::<Vec<_>>().join("\n")
    }
    /// `- **[SEV]** text` lines for GitHub comments.
    pub fn findings_markdown(&self) -> String {
        self.findings.iter().map(|(s, t)| format!("- **[{s}]** {t}")).collect::<Vec<_>>().join("\n")
    }
}

fn block_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s)=== REVIEW RESULT ===\s*\nVERDICT:\s*(\S+)\s*\nSUMMARY:\s*([^\n]+)\n=== FINDINGS ===\s*\n(.*?)=== END ===").expect("regex"))
}

fn finding_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?m)^-\s*\[(\w+)\]\s*(.+)$").expect("regex"))
}

/// Parses the first result block; none → BLOCKING.
pub fn parse(output: &str) -> Verdict {
    let text = output.replace('\r', "");
    let Some(c) = block_re().captures(&text) else {
        return Verdict::blocking("Could not parse reviewer output — treating as BLOCKING.");
    };
    let findings = finding_re().captures_iter(&c[3]).map(|f| (f[1].to_uppercase(), f[2].trim().to_string())).collect();
    Verdict { verdict: c[1].trim().to_uppercase(), summary: c[2].trim().to_string(), findings }
}

/// `## Automated Review — verdict: **V**` comment for the PR.
pub fn review_comment(v: &Verdict, model: &str) -> String {
    let mut s = format!("## Automated Review — verdict: **{}**\n_Model: `{model}`_\n\n{}", v.verdict, v.summary);
    if !v.findings.is_empty() {
        s.push_str(&format!("\n\n### Findings\n{}", v.findings_markdown()));
    }
    s
}

/// The coder's test gate: the repo's `test_cmd` from `.agent.toml` in the worktree.
/// `None` = no command configured (gate skipped).
pub fn test_gate(worktree: &Path, project: &ProjectConfig) -> Option<Verdict> {
    let cmd = project.test_cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    let timeout = project.command_timeout_sec;
    Some(match run_shell(cmd, worktree, Duration::from_secs(timeout)) {
        Err(e) => Verdict::blocking(&format!("Test command could not be launched: {e}")),
        Ok(r) if r.timed_out => Verdict {
            verdict: "BLOCKING".into(),
            summary: format!("Test command timed out after {timeout}s."),
            findings: vec![("BLOCKING".into(), format!("Command `{cmd}` did not finish within {timeout}s."))],
        },
        Ok(r) if r.ok() => Verdict { verdict: "OK".into(), summary: "Tests passed.".into(), findings: vec![] },
        Ok(r) => {
            let code = r.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
            Verdict {
                verdict: "BLOCKING".into(),
                summary: format!("Test command failed (exit {code})."),
                findings: vec![("BLOCKING".into(), format!("Test command exited {code}. Output tail:\n{}", tail(&format!("{}{}", r.stdout, r.stderr), 1500)))],
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ok_and_blocking() {
        let ok = parse("blah\n=== REVIEW RESULT ===\nVERDICT: ok\nSUMMARY: Looks fine\n=== FINDINGS ===\n- [minor] nit\n=== END ===\n");
        assert_eq!(ok.verdict, "OK");
        assert!(!ok.is_blocking());
        assert_eq!(ok.findings, vec![("MINOR".to_string(), "nit".to_string())]);
        let bad = parse("=== REVIEW RESULT ===\r\nVERDICT: BLOCKING\r\nSUMMARY: Broken\r\n=== FINDINGS ===\r\n- [MAJOR] a\r\n- [MINOR] b\r\n=== END ===");
        assert!(bad.is_blocking());
        assert_eq!(bad.findings.len(), 2);
        assert!(bad.findings_text().contains("- [MAJOR] a"));
    }

    #[test]
    fn missing_or_unknown_verdict_blocks() {
        assert!(parse("no block").is_blocking());
        assert!(parse("=== REVIEW RESULT ===\nVERDICT: FAIL\nSUMMARY: x\n=== FINDINGS ===\n=== END ===").is_blocking());
    }

    #[test]
    fn comment_lists_findings() {
        let v = parse("=== REVIEW RESULT ===\nVERDICT: BLOCKING\nSUMMARY: s\n=== FINDINGS ===\n- [MAJOR] a\n=== END ===");
        let c = review_comment(&v, "m");
        assert!(c.starts_with("## Automated Review — verdict: **BLOCKING**"));
        assert!(c.contains("- **[MAJOR]** a"));
    }

    #[test]
    fn test_gate_runs_test_cmd() {
        let t = tempfile::tempdir().unwrap();
        let mut p = ProjectConfig::default();
        assert!(test_gate(t.path(), &p).is_none());
        p.test_cmd = "exit 0".into();
        assert!(!test_gate(t.path(), &p).unwrap().is_blocking());
        p.test_cmd = "exit 4".into();
        let v = test_gate(t.path(), &p).unwrap();
        assert!(v.is_blocking() && v.summary.contains("exit 4"));
    }
}
