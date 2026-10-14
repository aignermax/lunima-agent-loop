//! The optional Claude review and the verdict comment (`[qa-agent] **PASSED|FAILED**` — the
//! coder's QA fix keys on that prefix).

use super::steps::StepResult;
use crate::ctx::{Ctx, RoleError};
use crate::forge::QA_COMMENT_PREFIX;
use crate::llm::{Job, LlmError};
use crate::review::{parse, Verdict};
use std::collections::HashMap;
use std::path::Path;
use team_core::gh::PullRequest;
use team_core::provider::ChildEnv;

/// Runs the read-only Claude review of a PR. A usage limit is passed up (no verdict);
/// every other problem becomes a BLOCKING verdict.
pub fn review(ctx: &Ctx, repo: &str, pr: &PullRequest, dir: &Path) -> Result<(Verdict, String), RoleError> {
    let model = if pr.has_label(&ctx.cfg.labels.critical) { ctx.cfg.models.reviewer_critical.clone() } else { ctx.cfg.models.reviewer.clone() };
    let mut values: HashMap<&str, String> = HashMap::new();
    values.insert("pr_number", pr.number.to_string());
    values.insert("pr_title", pr.title.clone());
    values.insert("branch", pr.head.name.clone());
    values.insert("base_branch", pr.base.name.clone());
    values.insert("screenshots_dir", "docs/pr-media".into());
    ctx.tools.insert(&mut values);
    let prompt = match ctx.render("qa-review", &values) {
        Ok(p) => p,
        Err(e) => return Ok((Verdict::blocking(&format!("QA review crashed: {e}")), model)),
    };
    let job = Job {
        prompt,
        cwd: dir.to_path_buf(),
        model: model.clone(),
        max_turns: ctx.cfg.budgets.reviewer_max_turns,
        env: ChildEnv::subscription(),
        label: format!("qa_{}_pr-{}", repo.replace('/', "_"), pr.number),
    };
    match ctx.llm.run(&job) {
        Ok(out) => {
            crate::log::info(&format!("QA-Review PR #{}: {} Tokens", pr.number, out.usage.total()));
            Ok((parse(&out.text), model))
        }
        Err(LlmError::UsageLimit(t)) => Err(RoleError::UsageLimit(t)),
        Err(LlmError::Failed(e)) => Ok((Verdict::blocking(&format!("QA review could not run: {e}")), model)),
    }
}

/// The verdict comment, lines joined with `\n`.
pub fn comment(passed: bool, steps: &[StepResult], note: Option<&str>, review: Option<&Verdict>) -> String {
    let mut lines = vec![format!("{QA_COMMENT_PREFIX} **{}**", if passed { "PASSED" } else { "FAILED" }), String::new()];
    if let Some(n) = note {
        lines.push(n.to_string());
        lines.push(String::new());
    }
    for s in steps {
        lines.push(format!("- `{}`: {}", s.name, s.status()));
        if s.ran() && !s.passed && !s.tail.trim().is_empty() {
            lines.extend([String::new(), "```".into(), s.tail.trim_end().to_string(), "```".into()]);
        }
    }
    if let Some(v) = review {
        lines.push(String::new());
        lines.push(format!("### Claude review: **{}**", v.verdict));
        if !v.summary.is_empty() {
            lines.push(v.summary.clone());
        }
        if !v.findings.is_empty() {
            lines.push(String::new());
            lines.push(v.findings_markdown());
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_format() {
        let ok = StepResult { name: "build".into(), skipped: None, exit_code: Some(0), passed: true, tail: String::new() };
        let bad = StepResult { name: "test".into(), skipped: None, exit_code: Some(2), passed: false, tail: "boom".into() };
        let c = comment(false, &[ok, bad], None, Some(&Verdict { verdict: "BLOCKING".into(), summary: "s".into(), findings: vec![("MAJOR".into(), "x".into())] }));
        assert!(c.starts_with("[qa-agent] **FAILED**\n\n- `build`: ok\n- `test`: failed (exit 2)\n\n```\nboom\n```"));
        assert!(c.contains("### Claude review: **BLOCKING**\ns\n\n- **[MAJOR]** x"));
    }
}
