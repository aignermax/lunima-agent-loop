//! One weekly run: clone on the week's branch, context, scout → strategist → critic, the
//! deterministic apply step (issues, draft PR, previews, private outreach) and the report.
//! Progress is committed and pushed after every step, so a run can stop anywhere and resume.

use super::actions::{self, plan_items, Critique, GateInput, LABEL_APPROVAL, LABEL_APPROVED, LABEL_DECLINED, LABEL_OPPORTUNITY, LABEL_PR};
use super::analytics;
use super::context::{self, Inputs};
use super::decisions;
use super::ledger;
use super::model::{self, parse_decision, parse_evidence, parse_experiment, ROOT};
use super::page;
use super::report::{self, ReportInput};
use super::run_record::RunRecord;
use super::stages::{self, Run, Stage};
use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::workspace;
use chrono::Local;
use std::path::{Path, PathBuf};
use team_core::git::{commit_and_push, run_git};
use team_core::state::{read_ledger, DiscoveryItem, DiscoveryState};

/// How a run ended (besides errors).
#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    Done(String),
    /// `budget` | `paused`: stopped cleanly between steps
    Stopped(&'static str, String),
}

const SKELETON: &[(&str, &str)] = &[
    ("README.md", include_str!("../../assets/discovery/README.md")),
    ("HYPOTHESES.md", include_str!("../../assets/discovery/HYPOTHESES.md")),
    ("evidence/README.md", include_str!("../../assets/discovery/evidence-README.md")),
    ("decisions/TEMPLATE.md", include_str!("../../assets/discovery/decision-TEMPLATE.md")),
    ("experiments/TEMPLATE.md", include_str!("../../assets/discovery/experiment-TEMPLATE.md")),
    ("interviews/TEMPLATE.md", include_str!("../../assets/discovery/interview-TEMPLATE.md")),
    ("campaigns/TEMPLATE.md", include_str!("../../assets/discovery/campaign-TEMPLATE.md")),
    ("pages/README.md", include_str!("../../assets/discovery/pages-README.md")),
    ("pilot/ONBOARDING.md", include_str!("../../assets/discovery/pilot-ONBOARDING.md")),
    ("pilot/FEEDBACK.md", include_str!("../../assets/discovery/pilot-FEEDBACK.md")),
];

/// Creates missing skeleton files (never overwrites).
pub fn bootstrap(repo: &Path) -> Result<usize, String> {
    let mut n = 0;
    for (rel, text) in SKELETON {
        let p = repo.join(ROOT).join(rel);
        if !p.exists() {
            std::fs::create_dir_all(p.parent().expect("has parent")).map_err(|e| e.to_string())?;
            std::fs::write(&p, text).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(n)
}

pub fn branch(week: &str) -> String {
    format!("discovery/{week}")
}

fn private_dir(ctx: &Ctx) -> PathBuf {
    let d = &ctx.cfg.product_discovery.private_dir;
    if d.trim().is_empty() { DiscoveryState::dir(&ctx.paths.state()).join("private") } else { PathBuf::from(d) }
}

pub fn previews_dir(ctx: &Ctx) -> PathBuf {
    DiscoveryState::dir(&ctx.paths.state()).join("previews")
}

/// The clone on `discovery/<week>` (resumed from origin if it exists) with `.discovery/`
/// excluded from git. Returns (clone dir, integration branch).
fn prepare(ctx: &Ctx, repo: &str, week: &str) -> Result<(PathBuf, String), String> {
    let setup = workspace::setup(ctx, "product-discovery", repo)?;
    let c = &setup.clone;
    c.fetch()?;
    let integ = setup.working_branch.clone();
    let _ = c.git(&["reset", "--hard", "-q"]);
    let _ = c.git(&["clean", "-fdq", "-e", context::WORK_DIR]);
    let b = branch(week);
    let from = if c.remote_branch_exists(&b) { format!("origin/{b}") } else { format!("origin/{integ}") };
    c.git(&["checkout", "-q", "-B", &b, &from])?;
    let exclude = c.dir.join(".git").join("info").join("exclude");
    let text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !text.lines().any(|l| l.trim() == format!("{}/", context::WORK_DIR)) {
        let _ = std::fs::create_dir_all(exclude.parent().expect("has parent"));
        std::fs::write(&exclude, format!("{text}\n{}/\n", context::WORK_DIR)).map_err(|e| e.to_string())?;
    }
    Ok((c.dir.clone(), integ))
}

fn commit(repo: &Path, week: &str, run: &RunRecord, msg: &str) -> Result<(), String> {
    run.save(repo)?;
    commit_and_push(repo, &branch(week), &format!("discovery({week}): {msg}")).map(|_| ())
}

/// The most recent earlier run record on the branch.
fn previous(repo: &Path, week: &str) -> Option<RunRecord> {
    let mut weeks: Vec<String> = std::fs::read_dir(repo.join(ROOT).join("runs")).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".json").map(String::from)).filter(|w| w.as_str() < week).collect();
    weeks.sort();
    weeks.pop().map(|w| RunRecord::load(repo, &w))
}

/// Published experiment pages (`docs/x/<slug>/`).
fn published(repo: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(repo.join("docs").join(page::PUBLISHED_DIR)).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

fn evidence_count(repo: &Path, week: &str) -> usize {
    parse_evidence(&std::fs::read_to_string(repo.join(ROOT).join("evidence").join(format!("{week}.md"))).unwrap_or_default()).len()
}

/// Ids the critic has to judge: this week's new files plus the strategist's actions.
pub fn review_items(repo: &Path, base: &str) -> Vec<String> {
    let mut items = Vec::new();
    for f in stages::new_this_week(repo, base, "decisions") {
        items.push(parse_decision(&std::fs::read_to_string(repo.join(&f)).unwrap_or_default()).id);
    }
    for f in stages::new_this_week(repo, base, "experiments") {
        items.push(parse_experiment(&std::fs::read_to_string(repo.join(&f)).unwrap_or_default()).id);
    }
    for f in stages::new_this_week(repo, base, "campaigns") {
        items.push(Path::new(&f).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    }
    for f in stages::new_this_week(repo, base, "pages") {
        if let Some(slug) = f.split('/').nth(2) {
            items.push(format!("P:{slug}"));
        }
    }
    if let Ok(a) = stages::load_actions(repo) {
        items.extend(a.opportunities.iter().filter(|o| o.action == "open").map(|o| format!("O:{}", o.key)));
        items.extend(a.interviews.iter().map(|i| format!("I:{}", i.key)));
        items.extend(a.approvals.iter().map(|p| format!("A:{}", p.key)));
        items.extend(a.spikes.iter().map(|s| format!("S:{}", s.key)));
        if !a.outreach.is_empty() {
            items.push("R:outreach".into());
        }
    }
    items.retain(|i| !i.is_empty());
    items
}

/// Writes outreach drafts outside the repository; returns the file (None = nothing to write).
fn write_outreach(ctx: &Ctx, week: &str, a: &actions::Actions) -> Option<PathBuf> {
    if a.outreach.is_empty() {
        return None;
    }
    let dir = private_dir(ctx);
    std::fs::create_dir_all(&dir).ok()?;
    let mut md = format!("# Outreach drafts — week {week}\n\nPRIVATE. Prepared by the discovery run; nothing was sent. Send only what you would write yourself, personally, one by one.\n\n");
    for o in &a.outreach {
        md.push_str(&format!(
            "## {} — {}\n- public source: {}\n- why relevant: {}\n- recent activity: {}\n- hypothesis to validate: {}\n- channel: {}\n- ask: {}\n\n> {}\n\n",
            o.name,
            o.affiliation,
            o.public_source,
            o.relevance,
            o.activity,
            o.hypothesis,
            o.channel,
            o.ask,
            o.draft.replace('\n', "\n> ")
        ));
    }
    let file = dir.join(format!("outreach-{week}.md"));
    std::fs::write(&file, md).ok()?;
    Some(file)
}

fn pr_body(week: &str, summary: &str, drs: &[(String, String, String)], report_url: &str) -> String {
    let list: String = if drs.is_empty() { "- none this week\n".into() } else { drs.iter().map(|(id, title, verdict)| format!("- **{id}** {title} — critic: {verdict}\n")).collect() };
    format!(
        "<!-- discovery:pr week={week} -->\nWeekly product discovery, week **{week}** (scout → strategist → critic).\n\n{summary}\n\n### Decision Records in this PR\n{list}\n### How to decide\n- **Accept:** mark *Ready for review* and merge (this PR is a draft so no automation can merge it).\n- **Reject a single DR before merging:** comment `/reject DR-NNNN <reason>`.\n- Optional: `review-minutes: N` records how long the review took.\n- Changes requested? Edit the files directly or comment; the next weekly run reads your comments.\n\nWeekly report: {report_url}\n\nNothing in this PR has been published, sent or paid for.\n"
    , summary = if summary.trim().is_empty() { "(no summary)" } else { summary.trim() })
}

/// The run itself. `st` is the role's state file; it is saved as the run progresses.
pub fn week(ctx: &Ctx, repo_name: &str, week: &str, st: &mut DiscoveryState) -> RoleResult<Ended> {
    let cfg = ctx.cfg.product_discovery.clone();
    let state_dir = ctx.paths.state();
    let save = |st: &DiscoveryState| {
        if let Err(e) = st.save(&state_dir) {
            crate::log::warn(&format!("Discovery-Status nicht gespeichert: {e}"));
        }
    };
    let forge = (ctx.forge)(repo_name);
    let (repo, integ) = prepare(ctx, repo_name, week)?;
    let base = format!("origin/{integ}");
    let today = Local::now().format("%Y-%m-%d").to_string();
    let mut run = RunRecord::load(&repo, week);
    st.stages_done = run.stages_done.clone();

    if !run.done("prepare") {
        let created = bootstrap(&repo)?;
        let drs = decisions::effective(&repo, forge.as_ref());
        let normalized = decisions::normalize(&repo, &drs, &today);
        if created > 0 {
            run.note(format!("created the discovery skeleton ({created} files)"));
        }
        if !normalized.is_empty() {
            run.note(format!("recorded the maintainer's verdicts: {}", normalized.join(", ")));
        }
        run.finish("prepare");
        commit(&repo, week, &run, "prepare")?;
    }

    let model = if cfg.model.trim().is_empty() { ctx.cfg.models.product_owner.clone() } else { cfg.model.clone() };
    let mut stage_run = Run { ctx, repo: &repo, repo_name, week, base: base.clone(), model, review_items: vec![] };

    for stage in Stage::ALL {
        if run.done(stage.name()) {
            continue;
        }
        if let Some(reason) = team_core::state::pause_reason(&state_dir, "product-discovery", Local::now()) {
            return Ok(Ended::Stopped("paused", format!("vor {}: {reason}", stage.name())));
        }
        if let Err(why) = ledger::check_budget(&state_dir, week, stage.name(), cfg.weekly_budget_usd, cfg.stage_estimate_usd) {
            run.note(format!("stopped before {}: {why}", stage.name()));
            commit(&repo, week, &run, &format!("stopped before {} (budget)", stage.name()))?;
            return Ok(Ended::Stopped("budget", why));
        }
        // fresh inputs for every stage (the maintainer may have answered in between)
        let drs = decisions::effective(&repo, forge.as_ref());
        let metrics = analytics::collect(&cfg, &published(&repo), &(Local::now() - chrono::Duration::days(90)).format("%Y-%m-%d").to_string(), &analytics::curl_fetch);
        let spent = ledger::week_cost(&read_ledger(&state_dir), week);
        let caps = format!("- open opportunities: at most {}\n- new experiments per week: at most {}\n- spikes per week: at most {}\n- new GitHub issues per run: at most {}", cfg.max_open_opportunities, cfg.max_new_experiments, cfg.max_spikes, cfg.max_new_issues);
        let prev = previous(&repo, week);
        context::write(&repo, forge.as_ref(), &Inputs { week, today: &today, repo_name, budget_left_usd: (cfg.weekly_budget_usd - spent).max(0.0), caps, drs: &drs, metrics: &metrics, previous: prev.as_ref(), extra: &cfg.context_files })?;
        if stage == Stage::Critic {
            stage_run.review_items = review_items(&repo, &base);
        }
        st.stage = stage.name().into();
        save(st);
        ctx.heartbeat.report("working", team_core::detail!["repo" => repo_name, "title" => format!("Discovery {week}: {}", stage.name())]);
        crate::log::info(&format!("Discovery {week}: {}", stage.name()));
        let rep = stages::run_stage(&stage_run, stage)?;
        for f in &rep.reverted {
            run.note(format!("{}: reverted a change outside its permissions: {f}", stage.name()));
        }
        for d in &rep.discarded {
            run.note(format!("{}: discarded {d}", stage.name()));
        }
        if stage == Stage::Scout {
            st.evidence_added = evidence_count(&repo, week);
            st.duplicates_dropped = rep.duplicates;
        }
        run.finish(stage.name());
        st.stages_done = run.stages_done.clone();
        save(st);
        commit(&repo, week, &run, stage.name())?;
    }

    // ---------------------------------------------------------------- apply
    st.stage = "apply".into();
    st.evidence_added = evidence_count(&repo, week);
    save(st);
    let acts = stages::load_actions(&repo).unwrap_or_default();
    let critique: Critique = stages::load_critique(&repo).unwrap_or_default();
    let mut experiments = Vec::new();
    for f in stages::new_this_week(&repo, &base, "experiments") {
        let text = std::fs::read_to_string(repo.join(&f)).unwrap_or_default();
        let x = parse_experiment(&text);
        if critique.verdict(&x.id).is_some_and(|v| v.verdict == "reject") && x.status == "proposed" {
            let _ = std::fs::write(repo.join(&f), text.replacen("- status: proposed", "- status: rejected-by-critic", 1));
        } else if x.status == "proposed" {
            experiments.push(x);
        }
    }
    let pages: Vec<(String, String)> = stages::new_this_week(&repo, &base, "pages").into_iter().filter_map(|f| f.split('/').nth(2).map(|s| (s.to_string(), f.clone()))).collect();
    let campaigns: Vec<(String, String)> = stages::new_this_week(&repo, &base, "campaigns")
        .into_iter()
        .map(|f| {
            let text = std::fs::read_to_string(repo.join(&f)).unwrap_or_default();
            let fl = model::fields(&text);
            let id = Path::new(&f).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            (id, format!("proposed {} USD, cap {} USD, {} days", fl.get("proposed_budget_usd").cloned().unwrap_or_default(), fl.get("max_spend_usd").cloned().unwrap_or_default(), fl.get("duration_days").cloned().unwrap_or_default()))
        })
        .collect();
    let outreach_ok = critique.verdict("R:outreach").is_some_and(|v| v.verdict != "reject");
    let outreach_file = if outreach_ok { write_outreach(ctx, week, &acts) } else { None };
    let open_opps = forge.open_issues(LABEL_OPPORTUNITY)?.len();
    let mut gate = plan_items(
        &acts,
        &critique,
        &GateInput {
            week,
            task_label: &ctx.cfg.labels.task,
            needs_human: &ctx.cfg.labels.needs_human,
            integration_branch: &integ,
            open_opportunities: open_opps,
            max_open_opportunities: cfg.max_open_opportunities,
            max_new_experiments: cfg.max_new_experiments,
            max_spikes: cfg.max_spikes,
            experiments: &experiments,
            pages: &pages,
            campaigns: &campaigns,
            outreach_count: if outreach_file.is_some() { acts.outreach.len() } else { 0 },
        },
    );
    let repo_for_persist = repo.clone();
    let filed = actions::file_items(forge.as_ref(), &mut gate, &mut run, cfg.max_new_issues, &mut |r| {
        let _ = r.save(&repo_for_persist);
    })?;
    st.issues_created = run.created.len();
    for b in &gate.blocked_privacy {
        run.note(format!("not filed (privacy): {b}"));
    }
    let previews = page::render_previews(&repo, &previews_dir(ctx));
    run.summary = acts.summary.clone();
    run.critic_summary = critique.summary.clone();
    commit(&repo, week, &run, "apply")?;

    let this_week_drs: Vec<(String, String, String)> = stages::new_this_week(&repo, &base, "decisions")
        .iter()
        .map(|f| {
            let d = parse_decision(&std::fs::read_to_string(repo.join(f)).unwrap_or_default());
            (d.id, d.title, d.critic_verdict.unwrap_or_else(|| "—".into()))
        })
        .collect();
    let pr_title = format!("Discovery: week {week}");
    let pr = match run.pr.and_then(|n| forge.pr(n).ok()).filter(|p| p.state == "open" || p.state.is_empty()) {
        Some(p) => p,
        None => match forge.pr_by_branch(&branch(week))? {
            Some(p) => p,
            None => forge.create_draft_pr(&pr_title, &pr_body(week, &acts.summary, &this_week_drs, &st.report_url), &branch(week), &integ)?,
        },
    };
    forge.add_labels(pr.number, &[LABEL_PR])?;
    run.pr = Some(pr.number);
    run.finish("apply");
    commit(&repo, week, &run, "run record")?;

    let drs = decisions::effective(&repo, forge.as_ref());
    let ledger_all = read_ledger(&state_dir);
    let week_cost = ledger::week_cost(&ledger_all, week);
    let total: f64 = ledger_all.iter().map(|r| r.cost_usd).sum();
    let mut review_minutes = 0;
    for w in std::fs::read_dir(repo.join(ROOT).join("runs")).into_iter().flatten().flatten() {
        if let Some(n) = w.file_name().to_string_lossy().strip_suffix(".json").map(|w| RunRecord::load(&repo, w)).and_then(|r| r.pr) {
            review_minutes += decisions::pr_verdicts(forge.as_ref(), n).map(|v| v.review_minutes).unwrap_or(0);
        }
    }
    let approvals_open: Vec<(u64, String)> = forge.open_issues(LABEL_APPROVAL)?.into_iter().filter(|i| !i.has_label(LABEL_APPROVED) && !i.has_label(LABEL_DECLINED)).map(|i| (i.number, i.title)).collect();
    let score = report::scoreboard(&repo, &drs);
    let notes: Vec<String> = run.notes.clone();
    let body = report::markdown(&ReportInput {
        week,
        result: "finished",
        summary: &acts.summary,
        recommendation: &acts.recommendation,
        critic_summary: &critique.summary,
        pr_url: &pr.html_url,
        filed: &filed,
        rejected: &gate.rejected,
        unreviewed: &gate.unreviewed,
        deferred: &gate.deferred,
        notes: &notes,
        evidence_added: st.evidence_added,
        duplicates: st.duplicates_dropped,
        week_cost,
        total_cost: total,
        week_tokens: ledger::week_tokens(&ledger_all, week),
        review_minutes,
        score: &score,
        pending_approvals: &approvals_open,
        previews: &previews,
        outreach: if outreach_file.is_some() { acts.outreach.len() } else { 0 },
    });
    let _ = std::fs::write(DiscoveryState::dir(&state_dir).join(team_core::state::DISCOVERY_REPORT_FILE), &body);
    match report::publish(forge.as_ref(), &body) {
        Ok((_, url)) => st.report_url = url,
        Err(e) => crate::log::warn(&format!("Discovery-Bericht nicht veröffentlicht: {e}")),
    }
    let _ = forge.edit_pr_body(pr.number, &pr_body(week, &acts.summary, &this_week_drs, &st.report_url));

    st.pr_url = pr.html_url.clone();
    st.week_cost_usd = week_cost;
    st.week_tokens = ledger::week_tokens(&ledger_all, week);
    st.opportunities = forge.open_issues(LABEL_OPPORTUNITY)?.into_iter().map(|i| DiscoveryItem { id: format!("#{}", i.number), title: i.title, status: "open".into(), url: i.html_url }).collect();
    st.approvals = approvals_open.iter().map(|(n, t)| DiscoveryItem { id: format!("#{n}"), title: t.clone(), status: "pending".into(), url: String::new() }).collect();
    st.decisions = drs.iter().map(|d| DiscoveryItem { id: d.d.id.clone(), title: d.d.title.clone(), status: format!("{} · {}", d.effective, d.d.decision), url: String::new() }).collect();
    st.stage.clear();
    save(st);
    let _ = run_git(Some(&repo), &["checkout", "-q", "--detach"], false);
    Ok(Ended::Done(format!("{} Evidenz, {} Issues, PR #{}", st.evidence_added, filed.len(), pr.number)))
}

/// Errors that end a run but are no failure of the week (the daemon handles them).
pub fn is_usage_limit(e: &RoleError) -> bool {
    matches!(e, RoleError::UsageLimit(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_creates_but_never_overwrites() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(bootstrap(t.path()).unwrap(), SKELETON.len());
        std::fs::write(t.path().join("discovery/HYPOTHESES.md"), "mine").unwrap();
        assert_eq!(bootstrap(t.path()).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(t.path().join("discovery/HYPOTHESES.md")).unwrap(), "mine");
        let h = model::parse_hypotheses(include_str!("../../assets/discovery/HYPOTHESES.md"));
        assert_eq!(h.len(), 9);
        assert!(h.iter().all(|(_, s)| s == "open"));
    }
}
