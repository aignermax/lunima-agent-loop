//! Product discovery end to end: scripted Claude stages, simulated GitHub, real git against a
//! local origin, two machines sharing both.

use super::*;
use crate::fake::{self, FakeForge, FakeLlm};
use crate::forge::Forge;
use crate::llm::{Job, LlmError};
use chrono::TimeZone;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use team_core::claude::Outcome;

const WEEK: &str = "2026-w45";
const EARLIER: &str = "# Evidence 2026-w44\n\n### E-2026-w44-01\n- class: external\n- level: E1\n- source: https://klayout.de/forum/discussion/77\n- quote: \"Converting flat GDS back to cells is manual work\"\n- supports: H3\n";

struct World {
    tmp: tempfile::TempDir,
    origin: PathBuf,
    forge: Arc<FakeForge>,
    llm: Arc<FakeLlm>,
}

/// Monday of week 45, after the 06:00 window opened.
fn monday() -> chrono::DateTime<Local> {
    Local.with_ymd_and_hms(2026, 11, 2, 7, 0, 0).unwrap()
}

fn world(llm: FakeLlm) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let origin = fake::origin(tmp.path(), &[("docs/AGENT_BRIDGE.md", "MCP bridge"), ("discovery/evidence/2026-w44.md", EARLIER)]);
    World { tmp, origin, forge: Arc::new(FakeForge::new("o/r")), llm: Arc::new(llm) }
}

impl World {
    fn ctx(&self, machine: &str) -> Ctx {
        let dir = self.tmp.path().join(machine);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = fake::ctx(&dir, &self.origin, self.forge.clone(), self.llm.clone());
        ctx.machine = machine.into();
        ctx.cfg.product_owner.repo = "o/r".into();
        ctx.cfg.roles.product_discovery = true;
        ctx
    }
    fn role(&self, at: chrono::DateTime<Local>) -> ProductDiscovery {
        ProductDiscovery::with_clock(Box::new(move || at))
    }
    fn pushed(&self, file: &str) -> Option<String> {
        fake::pushed_file(&self.origin, &format!("discovery/{WEEK}"), file)
    }
    fn issues_titled(&self, prefix: &str) -> Vec<team_core::gh::Issue> {
        self.forge.s.lock().unwrap().issues.values().filter(|i| i.title.starts_with(prefix)).cloned().collect()
    }
}

fn put(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn done(cost: f64) -> Result<Outcome, LlmError> {
    Ok(Outcome { text: "done".into(), subtype: "success".into(), num_turns: 12, cost_usd: cost, ..Default::default() })
}

const EVIDENCE: &str = "# Evidence 2026-w45\n\nSearched gdsfactory issues and the KLayout forum.\n\n### E-2026-w45-01\n- class: external\n- level: E1\n- source: https://github.com/gdsfactory/gdsfactory/issues/9\n- observed: 2026-10-20\n- retrieved: 2026-11-02\n- who: layout engineer\n- quote: \"Importing a foreign GDS loses all ports\"\n- supports: H3\n- contradicts: none\n\n### E-2026-w45-02\n- class: external\n- level: E1\n- source: https://www.klayout.de/forum/discussion/77/?utm_source=x\n- quote: \"Converting flat GDS back to cells is manual work\"\n- supports: H3\n";

fn scout(cost: f64) -> impl Fn(&Job) -> Result<Outcome, LlmError> + Send + Sync {
    move |job: &Job| {
        // one bad entry (a web page is not user feedback) and a write outside its permissions
        put(&job.cwd, "discovery/evidence/2026-w45.md", &format!("{EVIDENCE}\n### E-2026-w45-03\n- class: user-feedback\n- level: E2\n- source: https://reddit.com/r/photonics/x\n- quote: \"I would pay for this\"\n- supports: H2\n"));
        put(&job.cwd, "docs/x/evil/index.html", "<h1>published?</h1>");
        done(cost)
    }
}

fn scout_valid(job: &Job) -> Result<Outcome, LlmError> {
    put(&job.cwd, "discovery/evidence/2026-w45.md", EVIDENCE);
    done(1.0)
}

fn scout_repair(job: &Job) -> Result<Outcome, LlmError> {
    assert!(job.prompt.contains("REPAIR") && job.prompt.contains("E-2026-w45-03"), "the repair session gets the problems");
    put(&job.cwd, "discovery/evidence/2026-w45.md", &format!("{EVIDENCE}\n### E-2026-w45-03\n- class: external\n- level: E1\n- source: https://reddit.com/r/photonics/x\n- quote: \"I would pay for this\"\n- supports: H2\n"));
    done(0.5)
}

const DR: &str = "# DR-0001: Should we test demand for foreign-GDS reconstruction?\n- week: 2026-w45\n- status: proposed\n- decision: experiment\n- hypotheses: H3\n- evidence: E-2026-w45-01\n\n## Options\n### A: Do nothing\nKeep the roadmap.\n### B: Landing-page test\nMeasure pilot interest.\n\n## Evidence\nOne public issue (E1).\n\n## Decision\nRun X-2026-w45-gds-roundtrip.\n\n## Delivery\n- [ ] Spike: round-trip three public layouts — acceptance: pin recall reported\n";

fn strategist(job: &Job) -> Result<Outcome, LlmError> {
    assert!(job.prompt.contains("DR-0001"), "next free DR id is in the prompt");
    let h = std::fs::read_to_string(job.cwd.join("discovery/HYPOTHESES.md")).unwrap();
    put(&job.cwd, "discovery/HYPOTHESES.md", &h.replacen("## H3: Semantic GDS reconstruction\n- status: open", "## H3: Semantic GDS reconstruction\n- status: testing", 1));
    put(&job.cwd, "discovery/decisions/DR-0001-gds-demand.md", DR);
    put(&job.cwd, "discovery/experiments/X-2026-w45-gds-roundtrip.md", "# X-2026-w45-gds-roundtrip: Pilot interest in GDS reconstruction\n- status: proposed\n- method: landing-page\n- autonomy: needs-approval\n- assumption: designers want foreign layouts back as editable circuits\n\n## Pre-registration\n- success: ≥ 3 pilot requests from 100 visits\n- failure: < 1 pilot request from 100 visits\n- measurement: CTA clicks and mails\n- timebox: 2026-12-15\n\n## Results\n");
    put(
        &job.cwd,
        "discovery/pages/gds-reconstruction/page.json",
        r#"{"experiment":"X-2026-w45-gds-roundtrip","slug":"gds-reconstruction","title":"Foreign GDS back to editable circuits","tagline":"Import a layout and get components, pins and routes","problem":["Layouts from partners arrive as flat polygons."],"capabilities":[{"text":"MCP bridge for agents","status":"available","source":"docs/AGENT_BRIDGE.md"},{"text":"Pin recovery on foreign layouts","status":"planned"}],"cta":[{"label":"Join the pilot","kind":"pilot"}],"honesty_note":"Pin recovery on foreign layouts does not exist yet; this page measures interest first.","status":"draft","approved_by":"","approved_on":""}"#,
    );
    let actions = r#"{
      "summary": "One public issue and one forum thread describe lost ports after GDS import. Thin evidence; no user has spoken to us.",
      "recommendation": "Ask two designers before building anything.",
      "opportunities": [
        {"key":"gds-import","title":"Foreign GDS loses ports","problem":"ports vanish on import","persona":"layout engineer","evidence":["E-2026-w45-01"],"hypotheses":["H3"],"next_test":"interview","action":"open"},
        {"key":"chiplet-sim","title":"Chiplet simulation","problem":"multi-die loss","persona":"system architect","evidence":["E-2026-w45-01"],"hypotheses":["H5"],"next_test":"desk research","action":"open"}
      ],
      "interviews": [{"key":"gds-designers","title":"Two designers who import partner GDS","target_profile":"PIC layout engineer receiving foreign GDS","hypotheses":["H3"],"questions":["When did you last import a layout from a partner?","What did you do with it?","What did it cost you?"],"why":"Only E1 evidence so far"}],
      "outreach": [{"name":"Dr. Ada Example","affiliation":"Example University photonics group","public_source":"https://github.com/example/pic-tools","relevance":"maintains a GDS cell extractor","activity":"commits in October","hypothesis":"H3","channel":"GitHub discussions of example/pic-tools","draft":"Hi Ada, ...","ask":"20-minute call"}],
      "spikes": [{"key":"gds-roundtrip","title":"Round-trip three public layouts","question":"How many pins survive import?","acceptance":"pin recall per layout"}]
    }"#;
    put(&job.cwd, ".discovery/out/actions.json", actions);
    done(2.0)
}

fn critic(job: &Job) -> Result<Outcome, LlmError> {
    for item in ["DR-0001", "X-2026-w45-gds-roundtrip", "P:gds-reconstruction", "O:gds-import", "O:chiplet-sim", "I:gds-designers", "S:gds-roundtrip", "R:outreach"] {
        assert!(job.prompt.contains(item), "critic must review {item}");
    }
    let p = job.cwd.join("discovery/decisions/DR-0001-gds-demand.md");
    let dr = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("{dr}\n## Critic\n- verdict: weaken\n- strongest counter-argument: KLayout already extracts cells\n- what would change my mind: two designers describing the pain\n")).unwrap();
    let verdicts = r#"{"summary":"Evidence is a single issue; chiplet simulation has no evidence at all.","verdicts":[
      {"item":"DR-0001","verdict":"weaken","reason":"single source"},
      {"item":"X-2026-w45-gds-roundtrip","verdict":"stands","reason":"cheap, falsifiable"},
      {"item":"P:gds-reconstruction","verdict":"stands","reason":"honest draft"},
      {"item":"O:gds-import","verdict":"stands","reason":"two sources"},
      {"item":"O:chiplet-sim","verdict":"reject","reason":"evidence is about GDS, not chiplets"},
      {"item":"I:gds-designers","verdict":"stands","reason":"the cheapest real evidence"},
      {"item":"S:gds-roundtrip","verdict":"stands","reason":"answers feasibility"},
      {"item":"R:outreach","verdict":"stands","reason":"one relevant person"}]}"#;
    put(&job.cwd, ".discovery/out/critic.json", verdicts);
    done(1.5)
}

#[test]
fn a_complete_week_files_only_what_survives_the_critic() {
    let w = world(FakeLlm::new().then(scout(1.25)).then(scout_repair).then(strategist).then(critic));
    let ctx = w.ctx("A");
    assert_eq!(w.role(monday()).cycle(&ctx), Ok(true));
    assert_eq!(w.llm.calls(), 4, "scout, one scout repair, strategist, critic");

    // sandbox: no shell, no GitHub credentials
    for job in w.llm.jobs.lock().unwrap().iter() {
        let tools = job.tools.as_ref().expect("sandboxed");
        assert!(!tools.iter().any(|t| t == "Bash"));
        assert!(job.env.set.contains_key("GH_CONFIG_DIR") && job.env.remove.iter().any(|v| v == "GH_TOKEN"));
        assert!(job.max_duration.is_some());
    }

    // what reached the week's branch
    let ev = w.pushed("discovery/evidence/2026-w45.md").unwrap();
    assert!(ev.contains("E-2026-w45-01") && ev.contains("E-2026-w45-03"));
    assert!(!ev.contains("E-2026-w45-02"), "already collected in week 44: dropped");
    assert!(w.pushed("docs/x/evil/index.html").is_none(), "writes outside the stage's permissions are reverted");
    assert!(w.pushed("discovery/decisions/DR-0001-gds-demand.md").unwrap().contains("## Critic\n- verdict: weaken"));
    assert!(w.pushed("discovery/HYPOTHESES.md").unwrap().contains("- status: testing"));
    assert!(w.pushed("discovery/interviews/TEMPLATE.md").is_some(), "skeleton bootstrapped");
    assert!(w.pushed(".discovery/out/actions.json").is_none(), "work files are never committed");
    let record: run_record::RunRecord = serde_json::from_str(&w.pushed("discovery/runs/2026-w45.json").unwrap()).unwrap();
    assert_eq!(record.stages_done, vec!["prepare", "scout", "strategist", "critic", "apply"]);

    // the PR: a draft nobody but the maintainer can merge
    let pr = w.forge.pr(record.pr.unwrap()).unwrap();
    assert!(pr.draft && pr.title == "Discovery: week 2026-w45" && pr.base.name == "dev");
    let labels = w.forge.labels(pr.number);
    assert!(labels.contains(&"discovery".to_string()) && !labels.contains(&"agent-pr".to_string()));
    assert!(w.forge.merge_pr(pr.number).is_err());

    // issues: what the critic let through, within the caps
    let x = &w.issues_titled("Experiment:")[0];
    for l in ["discovery-experiment", "discovery-approval", "needs-human"] {
        assert!(w.forge.labels(x.number).contains(&l.to_string()), "{l}");
    }
    assert_eq!(w.issues_titled("Opportunity:").len(), 1, "chiplet-sim was rejected");
    assert!(w.issues_titled("Interview:")[0].body.as_deref().unwrap().contains("past behaviour"));
    let spike = &w.issues_titled("Spike:")[0];
    assert!(w.forge.labels(spike.number) == vec!["agent-task"] && spike.body.as_deref().unwrap().contains("Team branch: spike/gds-roundtrip"));
    assert_eq!(w.issues_titled("Approval:").len(), 2, "publish page + outreach");

    // outreach stays private
    let private = ctx.paths.state().join("discovery/private/outreach-2026-w45.md");
    assert!(std::fs::read_to_string(&private).unwrap().contains("Ada Example"));
    let public: String = w.forge.s.lock().unwrap().issues.values().filter_map(|i| i.body.clone()).collect();
    assert!(!public.contains("Ada"), "no names in public issues");

    // preview, report, ledger, state
    assert!(std::fs::read_to_string(ctx.paths.state().join("discovery/previews/gds-reconstruction/index.html")).unwrap().contains("PREVIEW"));
    let report = w.forge.open_issues("discovery-report").unwrap().remove(0);
    let body = report.body.unwrap();
    assert!(body.contains("O:chiplet-sim — evidence is about GDS") && body.contains("Ask two designers"));
    let ledger = team_core::state::read_ledger(&ctx.paths.state());
    assert_eq!(ledger.iter().map(|r| r.stage.as_str()).collect::<Vec<_>>(), vec!["scout", "scout-repair", "strategist", "critic"]);
    assert!(ledger.iter().all(|r| r.billing == "subscription-notional"));
    let st = DiscoveryState::load(&ctx.paths.state()).unwrap();
    assert_eq!((st.last_result.as_str(), st.attempts, st.week_cost_usd), ("ok", 1, 5.25));
    assert_eq!(st.evidence_added, 2);
    assert!(st.approvals.len() == 3 && st.decisions[0].id == "DR-0001");
    assert!(w.forge.lock_info("discovery-2026-w45").unwrap().is_some(), "a finished week keeps its lock");

    // once a week
    assert_eq!(w.role(monday() + chrono::Duration::hours(3)).cycle(&ctx), Ok(false));
    assert_eq!(w.llm.calls(), 4);
}

#[test]
fn an_interrupted_run_resumes_on_another_machine_without_repeating_work() {
    let w = world(FakeLlm::new().then(scout_valid).then(|_| Err(LlmError::Failed("Claude beendet ohne Ergebnis".into()))).then(strategist).then(critic));
    let a = w.ctx("A");
    assert!(matches!(w.role(monday()).cycle(&a), Err(RoleError::Failed(_))));
    let st = DiscoveryState::load(&a.paths.state()).unwrap();
    assert_eq!((st.last_result.as_str(), st.attempts), ("error", 1));
    assert!(w.forge.lock_info("discovery-2026-w45").unwrap().is_none(), "a failed run releases the week");
    let record: run_record::RunRecord = serde_json::from_str(&w.pushed("discovery/runs/2026-w45.json").unwrap()).unwrap();
    assert_eq!(record.stages_done, vec!["prepare", "scout"], "progress is on the branch");

    let b = w.ctx("B");
    assert_eq!(w.role(monday() + chrono::Duration::minutes(40)).cycle(&b), Ok(true));
    assert_eq!(w.llm.calls(), 4, "scout ran once; B resumed at the strategist");
    assert!(w.llm.prompt(2).contains("strategist"));
    assert_eq!(w.issues_titled("Opportunity:").len(), 1);
    // a stray third run of the same week files nothing twice
    let issues_before = w.forge.s.lock().unwrap().issues.len();
    w.forge.unlock("discovery-2026-w45").unwrap();
    let mut st = DiscoveryState::load(&a.paths.state()).unwrap();
    st.last_attempt = Some((monday() - chrono::Duration::hours(1)).to_rfc3339());
    st.save(&a.paths.state()).unwrap();
    assert_eq!(w.role(monday() + chrono::Duration::hours(2)).cycle(&a), Ok(true));
    assert_eq!(w.llm.calls(), 4, "all stages are recorded as done");
    assert_eq!(w.forge.s.lock().unwrap().issues.len(), issues_before, "idempotent: no duplicate issues or reports");
}

#[test]
fn only_one_machine_runs_a_week_and_stale_locks_are_broken() {
    let w = world(FakeLlm::new().then(scout_valid).then(strategist).then(critic));
    w.forge.try_lock("discovery-2026-w45", "OTHER").unwrap();
    let a = w.ctx("A");
    assert_eq!(w.role(monday()).cycle(&a), Ok(false));
    assert_eq!(DiscoveryState::load(&a.paths.state()).unwrap().last_result, "skipped");
    assert_eq!(w.llm.calls(), 0);
    // OTHER crashed long ago
    w.forge.age_lock("discovery-2026-w45", std::time::Duration::from_secs(4 * 3600));
    assert_eq!(w.role(monday() + chrono::Duration::minutes(31)).cycle(&a), Ok(true));
    assert_eq!(w.llm.calls(), 3);
}

#[test]
fn the_budget_stops_before_a_stage_that_would_cross_it() {
    let w = world(FakeLlm::new().then(|job: &Job| {
        put(&job.cwd, "discovery/evidence/2026-w45.md", EVIDENCE);
        done(4.5)
    }));
    let mut ctx = w.ctx("A");
    ctx.cfg.product_discovery.weekly_budget_usd = 6.0;
    assert_eq!(w.role(monday()).cycle(&ctx), Ok(false));
    assert_eq!(w.llm.calls(), 1, "strategist (estimate 4 USD) would cross 6 USD after 4.50");
    let st = DiscoveryState::load(&ctx.paths.state()).unwrap();
    assert_eq!(st.last_result, "budget");
    assert!(st.last_note.contains("4.50"));
    assert!(w.pushed("discovery/evidence/2026-w45.md").is_some(), "the scout's work is kept");
    assert!(w.pushed("discovery/runs/2026-w45.json").unwrap().contains("budget"));
    assert_eq!(w.role(monday() + chrono::Duration::hours(5)).cycle(&ctx), Ok(false), "the week is over for discovery");
    assert_eq!(w.llm.calls(), 1);
    assert!(w.forge.open_prs().unwrap().is_empty(), "nothing filed without the critic");
}

#[test]
fn pause_takes_effect_between_stages_and_is_not_a_failed_attempt() {
    let w = world(FakeLlm::new());
    let ctx = w.ctx("A");
    let control = ctx.paths.state().join("control.json");
    let w = World {
        llm: Arc::new(FakeLlm::new().then(move |job: &Job| {
            put(&job.cwd, "discovery/evidence/2026-w45.md", EVIDENCE);
            std::fs::write(&control, r#"{"paused":{"product-discovery":{"until":null,"reason":"Notstopp"}}}"#).unwrap();
            done(1.0)
        })),
        ..w
    };
    let ctx = w.ctx("A");
    assert_eq!(w.role(monday()).cycle(&ctx), Ok(false));
    assert_eq!(w.llm.calls(), 1);
    let st = DiscoveryState::load(&ctx.paths.state()).unwrap();
    assert_eq!((st.last_result.as_str(), st.attempts), ("paused", 0));
    assert!(st.last_note.contains("Notstopp"));
    assert!(w.forge.lock_info("discovery-2026-w45").unwrap().is_none(), "another machine may resume after the pause");
}

#[test]
fn usage_limit_gives_the_week_back() {
    let until = Local::now() + chrono::Duration::hours(2);
    let w = world(FakeLlm::new().then(move |_| Err(LlmError::UsageLimit(until))));
    let ctx = w.ctx("A");
    assert_eq!(w.role(monday()).cycle(&ctx), Err(RoleError::UsageLimit(until)));
    let st = DiscoveryState::load(&ctx.paths.state()).unwrap();
    assert_eq!((st.last_result.as_str(), st.attempts), ("usage-limit", 0));
    assert!(w.forge.lock_info("discovery-2026-w45").unwrap().is_none());
    assert_eq!(team_core::state::read_ledger(&ctx.paths.state())[0].result, "usage-limit");
}

#[test]
fn without_a_critic_review_nothing_is_filed_and_the_dr_is_withdrawn() {
    let silent = |job: &Job| {
        put(&job.cwd, ".discovery/out/critic.json", r#"{"summary":"","verdicts":[]}"#);
        done(1.0)
    };
    let w = world(FakeLlm::new().then(scout_valid).then(strategist).then(silent).then(silent));
    let ctx = w.ctx("A");
    assert_eq!(w.role(monday()).cycle(&ctx), Ok(true));
    assert_eq!(w.llm.calls(), 4, "critic + one repair");
    assert!(w.pushed("discovery/decisions/DR-0001-gds-demand.md").is_none(), "a DR without criticism is withdrawn");
    let titles: Vec<String> = w.forge.s.lock().unwrap().issues.values().map(|i| i.title.clone()).collect();
    assert_eq!(titles, vec!["Discovery — weekly report"], "unreviewed items are not filed");
    let report = w.forge.open_issues("discovery-report").unwrap().remove(0).body.unwrap();
    assert!(report.contains("O:gds-import") && report.contains("withdrawn"));
}

#[test]
fn the_product_owner_gets_accepted_decisions_and_cannot_merge_discovery_prs() {
    let w = world(FakeLlm::new());
    // dev already holds a merged discovery week: DR-0001 accepted by merging PR #1
    let origin = fake::origin(&w.tmp.path().join("po"), &[("discovery/decisions/DR-0001-gds-demand.md", DR), ("discovery/runs/2026-w45.json", r#"{"week":"2026-w45","pr":1}"#)]);
    let mut merged = fake::pr(1, "Discovery: week 2026-w45", "discovery/2026-w45", &["discovery"]);
    merged.state = "closed".into();
    merged.merged_at = Some("2026-11-03T10:00:00Z".into());
    let mut open_draft = fake::pr(2, "Discovery: week 2026-w46", "discovery/2026-w46", &["discovery"]);
    open_draft.draft = true;
    let forge = Arc::new(FakeForge::new("o/r").with_pr(merged).with_pr(open_draft).with_pr(fake::pr(3, "Agent: x", "agent/issue-1-1", &["agent-pr"])));
    // a rogue PO pass that un-drafts and merges the open discovery PR
    let rogue = {
        let forge = forge.clone();
        move |_: &Job| {
            forge.mark_ready(2);
            forge.merge_pr(2).unwrap();
            done(0.0)
        }
    };
    let llm = Arc::new(FakeLlm::new().then(rogue));
    let dir = w.tmp.path().join("po-home");
    std::fs::create_dir_all(&dir).unwrap();
    let mut ctx = fake::ctx(&dir, &origin, forge.clone(), llm.clone());
    ctx.cfg.product_owner.repo = "o/r".into();
    ctx.cfg.roles.product_discovery = true;
    let mut po = crate::po::ProductOwner::default();
    assert_eq!(po.cycle(&ctx), Ok(true));
    let prompt = llm.prompt(0);
    assert!(prompt.contains("### DR-0001 (experiment)") && prompt.contains("not yet handed over"));
    assert!(prompt.contains("Spike: round-trip three public layouts"));
    assert!(prompt.contains("Discovery PRs are the maintainer's"));
    assert!(prompt.contains("do not run your own product-market-fit research"));
    let alerts = forge.open_issues("needs-human").unwrap();
    assert!(alerts.iter().any(|i| i.title.contains("Discovery PR #2 was merged by automation")), "the guard reports it");

    // once a delivery issue names the DR, it counts as handed over
    let clone = ctx.paths.clone_dir("product-owner", "o/r");
    let section = report::po_section(&ctx, &clone, "o/r");
    assert!(section.contains("not yet handed over"));
    forge.create_issue("Spike: round-trip", "Decision: DR-0001").and_then(|i| forge.add_labels(i.number, &["agent-task"])).unwrap();
    assert!(report::po_section(&ctx, &clone, "o/r").contains("delivery issue exists"));
}
