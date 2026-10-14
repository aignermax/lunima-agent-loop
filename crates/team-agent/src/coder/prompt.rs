//! Coder prompts: initial / continuation (+ notes) and the review retry.

use crate::ctx::Ctx;
use crate::pr_text::{org_note, ux_note, PR_SUMMARY_INSTRUCTION};
use crate::review::Verdict;
use crate::session::SessionState;
use std::collections::HashMap;
use team_core::gh::Issue;

pub fn is_complex(ctx: &Ctx, issue: &Issue) -> bool {
    issue.has_label(&ctx.cfg.labels.complex)
}

fn body(issue: &Issue) -> String {
    issue.body.clone().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "No description provided.".into())
}

/// Local clones of the Akhetonics dependency repos, if this machine has them.
fn workspace_note(ctx: &Ctx, repo: &str) -> String {
    if !repo.to_lowercase().contains("akhetonics-desktop") {
        return String::new();
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from);
    let Some(ws) = home.map(|h| h.join("akhetonics-workspace")).filter(|p| p.is_dir()) else { return String::new() };
    let ws = ws.display().to_string();
    let t = &ctx.tools;
    format!("\n\n## 📦 IMPORTANT: Dependency Workspace Available\n**You have access to ALL dependency repositories at:**\n`{ws}`\n\nIt holds akhetonics-desktop, SAPPHIRE-Compiler, raycore-isa, raycore-assembler, phridge-blades-simulator, Phridge-Dispatcher, raycore-vulkan-icd, raycore-vulkan-layer and Lunima.\n\n**When you need NuGet-consumed source code, or need to understand a type/function that lives in one of these packages instead of in this repo:**\n1. Use `{} {}/semantic_search.py --path {ws} --query \"your search\"`\n2. `{} {}/find_symbol.py SymbolName` also accepts `--path`\n3. **DO NOT waste tokens guessing** — the source code is available locally!\n", t.python, t.dir, t.python, t.dir)
}

/// The prompt for the next coder session on `issue`.
pub fn session(ctx: &Ctx, repo: &str, issue: &Issue, state: &SessionState) -> Result<String, String> {
    let mut v: HashMap<&str, String> = HashMap::new();
    ctx.tools.insert(&mut v);
    v.insert("issue_number", issue.number.to_string());
    v.insert("issue_title", issue.title.clone());
    let branch_note = if state.session_count > 0 && !state.branch_name.starts_with("agent/issue-") {
        format!("\n**IMPORTANT:** You are working on existing branch: `{}`\nDo NOT create a new branch. All work must be on this branch.", state.branch_name)
    } else {
        String::new()
    };
    v.insert("branch_note", branch_note);
    let text = if state.session_count > 0 {
        let notes = state.notes.iter().rev().take(5).rev().cloned().collect::<Vec<_>>().join("\n");
        v.insert("session_number", (state.session_count + 1).to_string());
        v.insert("total_turns", state.total_turns_used.to_string());
        v.insert("branch_name", state.branch_name.clone());
        v.insert("recent_notes", if notes.is_empty() { "No notes yet.".into() } else { notes });
        ctx.render("coder-continuation", &v)?
    } else {
        v.insert("issue_body", body(issue));
        ctx.render("coder-initial", &v)?
    };
    let ux = if is_complex(ctx, issue) { ux_note(issue.number) } else { String::new() };
    Ok(format!("{text}{}{}{ux}{PR_SUMMARY_INSTRUCTION}", workspace_note(ctx, repo), org_note(&ctx.cfg.discovery.org)))
}

/// Retry after a BLOCKING test gate / review.
pub fn retry(ctx: &Ctx, issue: &Issue, branch: &str, blocking: &Verdict) -> Result<String, String> {
    let mut v: HashMap<&str, String> = HashMap::new();
    ctx.tools.insert(&mut v);
    v.insert("issue_number", issue.number.to_string());
    v.insert("issue_title", issue.title.clone());
    v.insert("issue_body", body(issue));
    v.insert("review_summary", blocking.summary.clone());
    let findings = blocking.findings_text();
    v.insert("findings_text", if findings.is_empty() { "(no specific findings; verdict was BLOCKING — see summary)".into() } else { findings });
    v.insert("branch", branch.to_string());
    ctx.render("coder-retry", &v)
}

/// The reviewer's prompt (diff against the PR's real base).
pub fn reviewer(ctx: &Ctx, issue: &Issue, pr: u64, branch: &str, base: &str) -> Result<String, String> {
    let mut v: HashMap<&str, String> = HashMap::new();
    ctx.tools.insert(&mut v);
    v.insert("pr_number", pr.to_string());
    v.insert("issue_number", issue.number.to_string());
    v.insert("issue_title", issue.title.clone());
    v.insert("issue_body", body(issue));
    v.insert("branch", branch.to_string());
    v.insert("base_branch", base.to_string());
    ctx.render("reviewer", &v)
}
