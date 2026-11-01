//! The three LLM stages and the checks around them.
//!
//! Each stage runs as a sandboxed Claude session (file tools + web search only, edits inside
//! the clone). Afterwards the code decides what survives: files outside the stage's allowed
//! paths are reverted, the output is validated, one repair session gets the list of
//! problems, and whatever is still invalid is discarded or salvaged — never committed as is.

use super::actions::{check_actions, Actions, Critique};
use super::context::{context_dir, out_dir};
use super::model::{self, check_campaign, check_decision, check_evidence, check_experiment, check_hypotheses, check_page, dedup_key, parse_decision, parse_evidence, parse_experiment, privacy_problems, rewrite_evidence, without_section, ROOT};
use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::llm::{Job, LlmError};
use chrono::Local;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;
use team_core::git::run_git;
use team_core::provider::ChildEnv;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Scout,
    Strategist,
    Critic,
}

impl Stage {
    pub const ALL: [Stage; 3] = [Stage::Scout, Stage::Strategist, Stage::Critic];
    pub fn name(self) -> &'static str {
        match self {
            Stage::Scout => "scout",
            Stage::Strategist => "strategist",
            Stage::Critic => "critic",
        }
    }
    fn prompt(self) -> &'static str {
        match self {
            Stage::Scout => "discovery-scout",
            Stage::Strategist => "discovery-strategist",
            Stage::Critic => "discovery-critic",
        }
    }
    /// May this stage write `path` (relative, `/`-separated)?
    pub fn allows(self, path: &str, week: &str) -> bool {
        let p = path.trim_start_matches("./");
        let in_dir = |dir: &str, prefix: &str, ext: &str| p.strip_prefix(&format!("{ROOT}/{dir}/")).is_some_and(|f| !f.contains('/') && f.starts_with(prefix) && f.ends_with(ext));
        match self {
            Stage::Scout => p == format!("{ROOT}/evidence/{week}.md"),
            Stage::Strategist => p == format!("{ROOT}/HYPOTHESES.md") || in_dir("decisions", "DR-", ".md") || in_dir("experiments", "X-", ".md") || in_dir("campaigns", "C-", ".md") || p.strip_prefix(&format!("{ROOT}/pages/")).is_some_and(|r| r.split('/').count() == 2 && r.ends_with("/page.json")),
            Stage::Critic => in_dir("decisions", "DR-", ".md"),
        }
    }
}

/// Built-in tools of a stage session: no shell, so no gh, git or network beyond web search.
pub const TOOLS: &[&str] = &["Read", "Glob", "Grep", "Write", "Edit", "WebSearch", "WebFetch"];

/// Everything a stage needs about the run.
pub struct Run<'a> {
    pub ctx: &'a Ctx,
    pub repo: &'a Path,
    pub repo_name: &'a str,
    pub week: &'a str,
    /// `origin/<integration branch>`: the accepted state experiments and pages are compared with
    pub base: String,
    pub model: String,
    /// item ids the critic must judge (filled after the strategist)
    pub review_items: Vec<String>,
}

/// Paths changed in the work tree (tracked or new), relative.
pub fn changed(repo: &Path) -> Vec<String> {
    run_git(Some(repo), &["status", "--porcelain=v1", "--untracked-files=all"], false)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let p = &l[3..];
            p.rsplit(" -> ").next().unwrap_or(p).trim_matches('"').to_string()
        })
        .collect()
}

/// Back to the committed version (or gone, if the stage created it).
pub fn restore(repo: &Path, path: &str) {
    if run_git(Some(repo), &["cat-file", "-e", &format!("HEAD:{path}")], false).is_ok() {
        let _ = run_git(Some(repo), &["checkout", "HEAD", "--", path], false);
    } else {
        let _ = std::fs::remove_file(repo.join(path));
    }
}

/// `path` as committed before this stage (`HEAD`) or on the integration branch (`base`).
pub fn show(repo: &Path, rev: &str, path: &str) -> Option<String> {
    run_git(Some(repo), &["show", &format!("{rev}:{path}")], false).ok()
}

fn read(repo: &Path, rel: &str) -> String {
    std::fs::read_to_string(repo.join(rel)).unwrap_or_default()
}

/// Evidence id → level, over all evidence files of the clone.
pub fn evidence_levels(repo: &Path) -> BTreeMap<String, u8> {
    let mut out = BTreeMap::new();
    for f in std::fs::read_dir(repo.join(ROOT).join("evidence")).into_iter().flatten().flatten() {
        if model::is_evidence_file(&f.file_name().to_string_lossy()) {
            for e in parse_evidence(&std::fs::read_to_string(f.path()).unwrap_or_default()) {
                if let Some(l) = e.level {
                    out.insert(e.id, l);
                }
            }
        }
    }
    out
}

/// Next free DR number.
pub fn next_dr(repo: &Path) -> String {
    let max = super::decisions::load(repo).iter().filter_map(|(f, _)| f.split("DR-").nth(1).and_then(|r| r.get(..4)).and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0);
    format!("DR-{:04}", max + 1)
}

/// A problem with stage output; `path` = the file to discard if a repair doesn't fix it.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub path: Option<String>,
    pub msg: String,
}

fn p(path: &str, msg: impl Into<String>) -> Problem {
    Problem { path: Some(path.to_string()), msg: msg.into() }
}

/// Removes entries already collected in earlier weeks or twice this week. Returns the
/// number dropped.
pub fn drop_duplicates(repo: &Path, week: &str) -> usize {
    let rel = format!("{ROOT}/evidence/{week}.md");
    let text = read(repo, &rel);
    if text.is_empty() {
        return 0;
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for f in std::fs::read_dir(repo.join(ROOT).join("evidence")).into_iter().flatten().flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        if name != format!("{week}.md") && model::is_evidence_file(&name) {
            for e in parse_evidence(&std::fs::read_to_string(f.path()).unwrap_or_default()) {
                if !e.source.is_empty() {
                    seen.insert(dedup_key(&e));
                }
            }
        }
    }
    let entries = parse_evidence(&text);
    let keep: Vec<&model::Evidence> = entries.iter().filter(|e| e.source.is_empty() || seen.insert(dedup_key(e))).collect();
    let dropped = entries.len() - keep.len();
    if dropped > 0 {
        let _ = std::fs::write(repo.join(&rel), rewrite_evidence(&text, &keep));
    }
    dropped
}

pub fn validate(run: &Run, stage: Stage) -> Vec<Problem> {
    let repo = run.repo;
    let mut out = Vec::new();
    let changed_files: Vec<String> = changed(repo).into_iter().filter(|f| stage.allows(f, run.week)).collect();
    for f in &changed_files {
        let problems = privacy_problems(&read(repo, f));
        if !problems.is_empty() {
            out.push(p(f, format!("{f}: must not contain {} (public repository)", problems.join("; "))));
        }
    }
    match stage {
        Stage::Scout => {
            let rel = format!("{ROOT}/evidence/{}.md", run.week);
            for e in parse_evidence(&read(repo, &rel)) {
                out.extend(check_evidence(&e, run.week).into_iter().map(|m| p(&rel, m)));
            }
        }
        Stage::Strategist => {
            let levels = evidence_levels(repo);
            for f in &changed_files {
                let text = read(repo, f);
                let before_stage = show(repo, "HEAD", f);
                let accepted = show(repo, &run.base, f);
                if f.ends_with("HYPOTHESES.md") {
                    out.extend(check_hypotheses(before_stage.as_deref().unwrap_or(""), &text).into_iter().map(|m| p(f, m)));
                } else if f.contains("/decisions/") {
                    out.extend(check_decision(f, &text, &levels, false).into_iter().map(|m| p(f, m)));
                    let d = parse_decision(&text);
                    match before_stage.as_deref().map(parse_decision) {
                        None if d.status != "proposed" => out.push(p(f, format!("{}: a new DR must have status proposed — only the maintainer accepts or rejects", d.id))),
                        Some(old) if old.status != d.status && d.status != "superseded" => out.push(p(f, format!("{}: an existing DR may only be marked superseded", d.id))),
                        Some(old) if old.decision != d.decision => out.push(p(f, format!("{}: the decision of an existing DR cannot change — write a new DR that supersedes it", d.id))),
                        _ => {}
                    }
                } else if f.contains("/experiments/") {
                    out.extend(check_experiment(f, &text, accepted.as_deref().or(before_stage.as_deref())).into_iter().map(|m| p(f, m)));
                    let x = parse_experiment(&text);
                    if before_stage.is_none() && (x.status != "proposed" || !x.id.contains(run.week)) {
                        out.push(p(f, format!("{}: a new experiment must be status proposed and carry this week ({}) in its id", x.id, run.week)));
                    }
                } else if f.contains("/campaigns/") {
                    out.extend(check_campaign(f, &text, accepted.as_deref()).into_iter().map(|m| p(f, m)));
                } else if f.contains("/pages/") {
                    out.extend(check_page(f, &text, repo, accepted.as_deref()).into_iter().map(|m| p(f, m)));
                }
            }
            match load_actions(repo) {
                Err(e) => out.push(Problem { path: None, msg: e }),
                Ok(a) => {
                    out.extend(check_actions(&a).into_iter().map(|m| Problem { path: None, msg: m }));
                    for e in a.opportunities.iter().flat_map(|o| o.evidence.iter()) {
                        if !levels.contains_key(e) {
                            out.push(Problem { path: None, msg: format!("actions.json cites unknown evidence {e}") });
                        }
                    }
                }
            }
        }
        Stage::Critic => {
            let levels = evidence_levels(repo);
            for f in new_this_week(repo, &run.base, "decisions") {
                let text = read(repo, &f);
                out.extend(check_decision(&f, &text, &levels, true).into_iter().map(|m| p(&f, m)));
                if let Some(before) = show(repo, "HEAD", &f) {
                    if without_section(&before, "Critic").trim() != without_section(&text, "Critic").trim() {
                        out.push(p(&f, format!("{f}: the critic may only write the `## Critic` section")));
                    }
                }
            }
            match load_critique(repo) {
                Err(e) => out.push(Problem { path: None, msg: e }),
                Ok(c) => {
                    for item in &run.review_items {
                        match c.verdict(item) {
                            None => out.push(Problem { path: None, msg: format!("critic.json: no verdict for {item}") }),
                            Some(v) if !model::VERDICTS.contains(&v.verdict.as_str()) => out.push(Problem { path: None, msg: format!("critic.json: verdict for {item} must be stands | weaken | reject") }),
                            Some(v) if v.reason.trim().is_empty() => out.push(Problem { path: None, msg: format!("critic.json: {item} needs a reason") }),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    out
}

pub fn load_actions(repo: &Path) -> Result<Actions, String> {
    let f = out_dir(repo).join("actions.json");
    let text = std::fs::read_to_string(&f).map_err(|_| "actions.json missing in .discovery/out/".to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("actions.json is not valid: {e}"))
}

pub fn load_critique(repo: &Path) -> Result<Critique, String> {
    let f = out_dir(repo).join("critic.json");
    let text = std::fs::read_to_string(&f).map_err(|_| "critic.json missing in .discovery/out/".to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("critic.json is not valid: {e}"))
}

/// Files of `discovery/<dir>/` that are not on the integration branch `base` (new this week).
pub fn new_this_week(repo: &Path, base: &str, dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(repo.join(ROOT).join(dir)).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let rel = if e.path().is_dir() { format!("{ROOT}/{dir}/{name}/page.json") } else { format!("{ROOT}/{dir}/{name}") };
        if repo.join(&rel).is_file() && !name.starts_with("TEMPLATE") && !name.starts_with("README") && show(repo, base, &rel).is_none() {
            out.push(rel);
        }
    }
    out.sort();
    out
}

/// Template values shared by the three prompts.
fn values(run: &Run, repair: &[Problem]) -> std::collections::HashMap<&'static str, String> {
    let cfg = &run.ctx.cfg.product_discovery;
    let mut v = std::collections::HashMap::new();
    v.insert("WEEK", run.week.to_string());
    v.insert("DATE", Local::now().format("%Y-%m-%d").to_string());
    v.insert("REPO", run.repo_name.to_string());
    v.insert("CONTEXT_DIR", format!("{}/context", super::context::WORK_DIR));
    v.insert("OUT_DIR", format!("{}/out", super::context::WORK_DIR));
    v.insert("EVIDENCE_FILE", format!("{ROOT}/evidence/{}.md", run.week));
    v.insert("NEXT_DR", next_dr(run.repo));
    v.insert("MAX_OPEN_OPPORTUNITIES", cfg.max_open_opportunities.to_string());
    v.insert("MAX_NEW_EXPERIMENTS", cfg.max_new_experiments.to_string());
    v.insert("MAX_SPIKES", cfg.max_spikes.to_string());
    v.insert("ITEMS", if run.review_items.is_empty() { "(none)".into() } else { run.review_items.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n") });
    let fix = if repair.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## REPAIR — your previous output was rejected\n\nFix exactly these problems in place (keep everything that was valid, do not start over):\n{}\n",
            repair.iter().map(|p| format!("- {}", p.msg)).collect::<Vec<_>>().join("\n")
        )
    };
    v.insert("REPAIR", fix);
    v
}

/// The sandbox: subscription login, no GitHub credentials even if a tool slipped through.
pub fn sandbox_env(ctx: &Ctx) -> ChildEnv {
    let mut env = ChildEnv::subscription();
    let empty_gh = ctx.paths.state().join("discovery").join("no-gh");
    let _ = std::fs::create_dir_all(&empty_gh);
    env.set.insert("GH_CONFIG_DIR".into(), empty_gh.display().to_string());
    env.set.insert("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".into(), "1".into());
    env.remove.extend(["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN"].iter().map(|s| s.to_string()));
    env
}

fn session(run: &Run, stage: Stage, repair: &[Problem]) -> RoleResult<()> {
    let ctx = run.ctx;
    let cfg = &ctx.cfg.product_discovery;
    let prompt = ctx.render(stage.prompt(), &values(run, repair))?;
    let label = format!("discovery_{}_{}{}", run.week, stage.name(), if repair.is_empty() { "" } else { "-repair" });
    let job = Job {
        prompt,
        cwd: run.repo.to_path_buf(),
        model: run.model.clone(),
        max_turns: cfg.max_turns,
        env: sandbox_env(ctx),
        label,
        max_duration: Some(Duration::from_secs(cfg.timeout_minutes.max(1) * 60)),
        tools: Some(TOOLS.iter().map(|t| t.to_string()).collect()),
    };
    let started = Local::now();
    let ledger_stage = format!("{}{}", stage.name(), if repair.is_empty() { "" } else { "-repair" });
    let result = ctx.llm.run(&job);
    let (outcome, tag) = match &result {
        Ok(o) if o.is_error => (Some(o), "error"),
        Ok(o) => (Some(o), "ok"),
        Err(LlmError::UsageLimit(_)) => (None, "usage-limit"),
        Err(LlmError::Failed(e)) if e.contains(team_core::claude::TIMED_OUT_TEXT) => (None, "timeout"),
        Err(_) => (None, "error"),
    };
    super::ledger::append(&ctx.paths.state(), &super::ledger::record(run.week, &ledger_stage, &ctx.machine, &run.model, started, outcome, tag));
    match result {
        Ok(o) if o.is_error && !o.reached_max_turns => Err(RoleError::Failed(format!("{}: Claude meldet Fehler ({})", stage.name(), o.subtype))),
        // out of turns: whatever was written is validated like any other output
        Ok(_) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// What a finished stage left behind.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageReport {
    pub reverted: Vec<String>,
    pub discarded: Vec<String>,
    pub duplicates: usize,
    pub repaired: bool,
}

fn revert_disallowed(run: &Run, stage: Stage, rep: &mut StageReport) {
    for f in changed(run.repo) {
        if !stage.allows(&f, run.week) {
            restore(run.repo, &f);
            rep.reverted.push(f);
        }
    }
}

/// Runs a stage with one repair round and salvages what stays invalid.
pub fn run_stage(run: &Run, stage: Stage) -> RoleResult<StageReport> {
    let mut rep = StageReport::default();
    session(run, stage, &[])?;
    revert_disallowed(run, stage, &mut rep);
    if stage == Stage::Scout {
        rep.duplicates += drop_duplicates(run.repo, run.week);
    }
    let mut problems = validate(run, stage);
    if !problems.is_empty() {
        rep.repaired = true;
        session(run, stage, &problems)?;
        revert_disallowed(run, stage, &mut rep);
        if stage == Stage::Scout {
            rep.duplicates += drop_duplicates(run.repo, run.week);
        }
        problems = validate(run, stage);
    }
    if problems.is_empty() {
        return Ok(rep);
    }
    salvage(run, stage, &problems, &mut rep);
    Ok(rep)
}

/// Keeps what is valid after a failed repair.
fn salvage(run: &Run, stage: Stage, problems: &[Problem], rep: &mut StageReport) {
    let repo = run.repo;
    if stage == Stage::Scout {
        // keep the valid entries, drop the invalid ones
        let rel = format!("{ROOT}/evidence/{}.md", run.week);
        let text = read(repo, &rel);
        let entries = parse_evidence(&text);
        let keep: Vec<&model::Evidence> = entries.iter().filter(|e| check_evidence(e, run.week).is_empty() && privacy_problems(&e.raw).is_empty()).collect();
        rep.discarded.extend(entries.iter().filter(|e| !keep.iter().any(|k| k.id == e.id)).map(|e| format!("{} (invalid evidence entry)", e.id)));
        let _ = std::fs::write(repo.join(&rel), rewrite_evidence(&text, &keep));
        return;
    }
    let files: BTreeSet<&str> = problems.iter().filter_map(|p| p.path.as_deref()).collect();
    for f in files {
        restore(repo, f);
        rep.discarded.push(format!("{f} ({})", problems.iter().filter(|p| p.path.as_deref() == Some(f)).map(|p| p.msg.as_str()).collect::<Vec<_>>().join("; ")));
    }
    let global: Vec<&Problem> = problems.iter().filter(|p| p.path.is_none()).collect();
    if !global.is_empty() {
        let name = if stage == Stage::Strategist { "actions.json" } else { "critic.json" };
        let reason = global.iter().map(|p| p.msg.as_str()).collect::<Vec<_>>().join("; ");
        if stage == Stage::Strategist {
            // unusable actions: nothing is filed this week
            let fallback = Actions { summary: format!("Strategist output discarded: {reason}"), ..Default::default() };
            let _ = std::fs::write(out_dir(repo).join("actions.json"), serde_json::to_string_pretty(&fallback).unwrap_or_default());
        } else if let Ok(mut c) = load_critique(repo) {
            // keep the valid verdicts; items without one stay unreviewed (and are not filed)
            c.verdicts.retain(|v| model::VERDICTS.contains(&v.verdict.as_str()) && !v.reason.trim().is_empty());
            let _ = std::fs::write(out_dir(repo).join("critic.json"), serde_json::to_string_pretty(&c).unwrap_or_default());
        } else {
            let _ = std::fs::write(out_dir(repo).join("critic.json"), serde_json::to_string_pretty(&Critique { summary: format!("Critic output unusable: {reason}"), verdicts: vec![] }).unwrap_or_default());
        }
        rep.discarded.push(format!("{name} ({reason})"));
    }
    if stage == Stage::Critic {
        // a DR the critic did not review is withdrawn: criticism must be part of every DR
        let levels = evidence_levels(repo);
        for f in new_this_week(repo, &run.base, "decisions") {
            if parse_decision(&read(repo, &f)).critic_verdict.is_none() || !check_decision(&f, &read(repo, &f), &levels, true).is_empty() {
                let _ = std::fs::remove_file(repo.join(&f));
                rep.discarded.push(format!("{f} (withdrawn: no valid critic review)"));
            }
        }
    }
}

/// Context folder sanity: the stages read from it, nothing there is committed.
pub fn context_ready(repo: &Path) -> bool {
    context_dir(repo).join("brief.md").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_write_permissions() {
        let w = "2026-w45";
        assert!(Stage::Scout.allows("discovery/evidence/2026-w45.md", w));
        assert!(!Stage::Scout.allows("discovery/evidence/2026-w44.md", w), "earlier weeks are history");
        assert!(!Stage::Scout.allows("discovery/HYPOTHESES.md", w));
        assert!(Stage::Strategist.allows("discovery/HYPOTHESES.md", w));
        assert!(Stage::Strategist.allows("discovery/decisions/DR-0004-x.md", w));
        assert!(Stage::Strategist.allows("discovery/pages/gds/page.json", w));
        assert!(!Stage::Strategist.allows("discovery/pages/gds/index.html", w));
        assert!(!Stage::Strategist.allows("docs/x/gds/index.html", w), "never the published site");
        assert!(!Stage::Strategist.allows("discovery/evidence/2026-w45.md", w), "evidence is the scout's");
        assert!(!Stage::Strategist.allows(".github/workflows/x.yml", w));
        assert!(Stage::Critic.allows("discovery/decisions/DR-0004-x.md", w));
        assert!(!Stage::Critic.allows("discovery/experiments/X-2026-w45-a.md", w));
    }
}
