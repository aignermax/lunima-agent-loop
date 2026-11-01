//! What the strategist wants done (`.discovery/out/actions.json`), what the critic thinks of
//! it (`.discovery/out/critic.json`) and the deterministic code that turns the surviving
//! items into GitHub issues. The LLM stages never touch GitHub themselves.
//!
//! Every created issue carries a marker with a stable key and is recorded in the week's run
//! record, so an interrupted run that is resumed never files the same item twice.

use super::model::{privacy_problems, Experiment};
use super::run_record::RunRecord;
use crate::forge::Forge;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const LABEL_PR: &str = "discovery";
pub const LABEL_OPPORTUNITY: &str = "discovery-opportunity";
pub const LABEL_EXPERIMENT: &str = "discovery-experiment";
pub const LABEL_INTERVIEW: &str = "discovery-interview";
pub const LABEL_APPROVAL: &str = "discovery-approval";
pub const LABEL_REPORT: &str = "discovery-report";
pub const LABEL_PARKED: &str = "discovery-parked";
/// set by the maintainer on an approval issue (the agent never sets these)
pub const LABEL_APPROVED: &str = "approved";
pub const LABEL_DECLINED: &str = "declined";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Opportunity {
    pub key: String,
    pub title: String,
    pub problem: String,
    pub persona: String,
    pub evidence: Vec<String>,
    pub hypotheses: Vec<String>,
    pub next_test: String,
    /// open | park | close
    pub action: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interview {
    pub key: String,
    pub title: String,
    pub target_profile: String,
    pub hypotheses: Vec<String>,
    pub questions: Vec<String>,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Approval {
    pub key: String,
    /// publish-page | external-contact | campaign | spend | architecture | dependency | api-break | confidential-data | other
    pub kind: String,
    pub title: String,
    pub action: String,
    pub rationale: String,
    pub evidence: Vec<String>,
    pub benefit: String,
    pub risk: String,
    pub cost: String,
    pub rollback: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Outreach {
    pub name: String,
    pub affiliation: String,
    pub public_source: String,
    pub relevance: String,
    pub activity: String,
    pub hypothesis: String,
    /// public channel (profile page, project discussion board) — never a scraped address
    pub channel: String,
    pub draft: String,
    pub ask: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spike {
    pub key: String,
    pub title: String,
    pub question: String,
    pub acceptance: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Actions {
    pub summary: String,
    pub recommendation: String,
    pub opportunities: Vec<Opportunity>,
    pub interviews: Vec<Interview>,
    pub approvals: Vec<Approval>,
    pub outreach: Vec<Outreach>,
    pub spikes: Vec<Spike>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Verdict {
    /// DR-0003 | X-… | C-… | O:<key> | I:<key> | A:<key> | S:<key> | P:<slug> | R:outreach
    pub item: String,
    /// stands | weaken | reject
    pub verdict: String,
    pub reason: String,
    pub strongest_counterargument: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Critique {
    pub summary: String,
    pub verdicts: Vec<Verdict>,
}

impl Critique {
    pub fn verdict(&self, item: &str) -> Option<&Verdict> {
        self.verdicts.iter().find(|v| v.item.eq_ignore_ascii_case(item))
    }
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(60).collect()
}

/// Problems with the strategist's actions file (empty = valid).
pub fn check_actions(a: &Actions) -> Vec<String> {
    let mut errs = Vec::new();
    if a.summary.trim().is_empty() {
        errs.push("actions.json: summary is required".into());
    }
    for o in &a.opportunities {
        if slug(&o.key) != o.key || o.key.is_empty() {
            errs.push(format!("opportunity „{}“: key must be lower-case-with-dashes", o.key));
        }
        if !["open", "park", "close"].contains(&o.action.as_str()) {
            errs.push(format!("opportunity {}: action must be open | park | close", o.key));
        }
        if o.action == "open" && (o.problem.trim().is_empty() || o.evidence.is_empty()) {
            errs.push(format!("opportunity {}: an open opportunity needs the problem and at least one evidence id", o.key));
        }
    }
    for i in &a.interviews {
        if i.key.is_empty() || i.questions.len() < 3 || i.target_profile.trim().is_empty() {
            errs.push(format!("interview „{}“: key, target_profile and at least three questions are required", i.key));
        }
    }
    for p in &a.approvals {
        for (name, v) in [("action", &p.action), ("rationale", &p.rationale), ("benefit", &p.benefit), ("risk", &p.risk), ("cost", &p.cost), ("rollback", &p.rollback)] {
            if v.trim().is_empty() {
                errs.push(format!("approval {}: {name} is required", p.key));
            }
        }
    }
    for o in &a.outreach {
        if !o.public_source.starts_with("https://") || o.draft.trim().is_empty() || o.hypothesis.trim().is_empty() {
            errs.push(format!("outreach {}: public_source (https URL), hypothesis and draft are required", o.name));
        }
        if o.channel.contains('@') || o.draft.contains('@') && privacy_problems(&o.draft).iter().any(|p| p.starts_with("e-mail")) {
            errs.push(format!("outreach {}: no e-mail addresses — name a public channel (profile or project page) instead", o.name));
        }
    }
    for s in &a.spikes {
        if s.key.is_empty() || s.acceptance.trim().is_empty() || s.question.trim().is_empty() {
            errs.push(format!("spike „{}“: key, question and acceptance are required", s.key));
        }
    }
    errs
}

/// One thing to file on GitHub, already gated.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// run-record key, e.g. "O:gds-import"
    pub key: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// What `plan_items` decided, for the report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gate {
    pub items: Vec<Item>,
    /// "O:x — reason" for items the critic rejected
    pub rejected: Vec<String>,
    /// items without a critic verdict (fail-safe: not filed)
    pub unreviewed: Vec<String>,
    /// items over a cap
    pub deferred: Vec<String>,
    /// opportunities to park or close: (key, reason)
    pub retire: Vec<(String, String)>,
    pub blocked_privacy: Vec<String>,
}

pub fn marker(key: &str) -> String {
    format!("<!-- discovery:item key={key} -->")
}

fn critic_note(v: &Verdict) -> String {
    format!("\n\n**Critic ({})**: {}{}", v.verdict, v.reason.trim(), if v.strongest_counterargument.trim().is_empty() { String::new() } else { format!("\n\n> Strongest counter-argument: {}", v.strongest_counterargument.trim()) })
}

fn list(items: &[String]) -> String {
    if items.is_empty() { "none".into() } else { items.join(", ") }
}

/// Context the gate needs besides the actions.
pub struct GateInput<'a> {
    pub week: &'a str,
    pub task_label: &'a str,
    pub needs_human: &'a str,
    pub integration_branch: &'a str,
    pub open_opportunities: usize,
    pub max_open_opportunities: usize,
    pub max_new_experiments: usize,
    pub max_spikes: usize,
    /// new experiment files of this week
    pub experiments: &'a [Experiment],
    /// (slug, spec path) of new draft pages of this week
    pub pages: &'a [(String, String)],
    /// (id, proposed budget line) of new campaign plans of this week
    pub campaigns: &'a [(String, String)],
    pub outreach_count: usize,
}

/// Applies the critic and the caps; nothing touches GitHub here.
pub fn plan_items(a: &Actions, c: &Critique, g: &GateInput) -> Gate {
    let mut gate = Gate::default();
    let footer = format!("\n\n---\n_Filed by the weekly product discovery ({}). Evidence: `discovery/evidence/`. The maintainer decides; the agent never acts on this outside the repository._", g.week);
    let consider = |key: String, title: String, body: String, labels: Vec<String>, gate: &mut Gate| match c.verdict(&key) {
        None => gate.unreviewed.push(key),
        Some(v) if v.verdict == "reject" => gate.rejected.push(format!("{key} — {}", v.reason.trim())),
        Some(v) => {
            let body = format!("{}\n\n{body}{}{footer}", marker(&key), critic_note(v));
            gate.items.push(Item { key, title, body, labels });
        }
    };
    for x in g.experiments.iter().take(g.max_new_experiments) {
        let mut labels = vec![LABEL_EXPERIMENT.to_string()];
        if x.autonomy == "needs-approval" {
            labels.extend([LABEL_APPROVAL.to_string(), g.needs_human.to_string()]);
        }
        let body = format!("Pre-registered experiment `{}` (method: {}, autonomy: {}).\n\nCriteria, timebox and budget are fixed in `discovery/experiments/{}.md` and are not edited after registration; results are appended there.{}", x.id, x.method, x.autonomy, x.id, if x.autonomy == "needs-approval" { "\n\n**Needs approval:** add the label `approved` to run it, `declined` to stop it. No answer = not approved." } else { "" });
        consider(x.id.clone(), format!("Experiment: {}", x.title), body, labels, &mut gate);
    }
    gate.deferred.extend(g.experiments.iter().skip(g.max_new_experiments).map(|x| format!("{} (max {} new experiments per week)", x.id, g.max_new_experiments)));
    for i in &a.interviews {
        let qs: String = i.questions.iter().map(|q| format!("1. {q}\n")).collect();
        let body = format!(
            "**Interview request for the maintainer** (the agent never contacts anyone).\n\n**Why:** {}\n\n**Who:** {}\n\n**Hypotheses probed:** {}\n\n**Questions** (ask about past behaviour, not opinions; don't pitch):\n{qs}\n**Afterwards:** keep raw notes private. Commit an anonymised summary (role and company type only, no names) as `discovery/interviews/I-NNNN.md` using `discovery/interviews/TEMPLATE.md`, then close this issue.",
            i.why, i.target_profile, list(&i.hypotheses)
        );
        consider(format!("I:{}", i.key), format!("Interview: {}", i.title), body, vec![LABEL_INTERVIEW.into(), g.needs_human.into()], &mut gate);
    }
    let mut opening = 0;
    for o in &a.opportunities {
        if o.action != "open" {
            gate.retire.push((o.key.clone(), o.reason.clone()));
            continue;
        }
        let key = format!("O:{}", o.key);
        if g.open_opportunities + opening >= g.max_open_opportunities {
            if c.verdict(&key).is_some_and(|v| v.verdict != "reject") {
                gate.deferred.push(format!("{key} (max {} open opportunities — park one first)", g.max_open_opportunities));
            }
            continue;
        }
        let body = format!("**Problem:** {}\n\n**Who:** {}\n\n**Evidence:** {}\n\n**Hypotheses:** {}\n\n**Cheapest next test:** {}", o.problem, o.persona, list(&o.evidence), list(&o.hypotheses), o.next_test);
        let before = gate.items.len();
        consider(key, format!("Opportunity: {}", o.title), body, vec![LABEL_OPPORTUNITY.into()], &mut gate);
        opening += gate.items.len() - before;
    }
    for (slug, spec) in g.pages {
        let body = format!("**Approval needed: publish the experiment page `{slug}`** on lunima.org.\n\nDraft spec: `{spec}` (in the discovery PR). A local preview was rendered by the agent; nothing is public.\n\nTo publish after review: `lunima-team-agent experiment publish <Lunima clone> {slug} --approved-by <you> --contact <address> [--goatcounter <code>]`, commit `docs/x/{slug}/` and the stamped spec, and merge to `main` yourself. Analytics stay off unless you pass `--goatcounter`.\n\n**Rollback:** delete `docs/x/{slug}/` and merge.");
        consider(format!("P:{slug}"), format!("Approval: publish experiment page {slug}"), body, vec![LABEL_APPROVAL.into(), g.needs_human.into()], &mut gate);
    }
    for (id, budget) in g.campaigns {
        let body = format!("**Approval needed: paid campaign `{id}`**. Plan: `discovery/campaigns/{id}.md` ({budget}).\n\nNothing is booked, no ad account is connected and the authorised budget is 0 until you set `authorized budget usd` in the plan yourself and launch it from your own account.");
        consider(id.clone(), format!("Approval: campaign {id}"), body, vec![LABEL_APPROVAL.into(), g.needs_human.into()], &mut gate);
    }
    for p in &a.approvals {
        let body = format!("**Approval needed ({})**\n\n**Action:** {}\n\n**Rationale:** {}\n\n**Evidence:** {}\n\n**Expected benefit:** {}\n\n**Risk:** {}\n\n**Cost:** {}\n\n**Rollback:** {}\n\nAdd the label `approved` or `declined`. No answer = not approved.", p.kind, p.action, p.rationale, list(&p.evidence), p.benefit, p.risk, p.cost, p.rollback);
        consider(format!("A:{}", p.key), format!("Approval: {}", p.title), body, vec![LABEL_APPROVAL.into(), g.needs_human.into()], &mut gate);
    }
    for (n, s) in a.spikes.iter().enumerate() {
        let key = format!("S:{}", s.key);
        if n >= g.max_spikes {
            gate.deferred.push(format!("{key} (max {} spikes per week)", g.max_spikes));
            continue;
        }
        let body = format!("**Throwaway feasibility spike** from product discovery — answers a question, is not merged into `{}`.\n\nTeam branch: spike/{}\n\n**Question:** {}\n\n**Acceptance:** {}\n\nReport the answer (numbers, what worked, what didn't) in the PR description.", g.integration_branch, s.key, s.question, s.acceptance);
        consider(key, format!("Spike: {}", s.title), body, vec![g.task_label.into()], &mut gate);
    }
    if g.outreach_count > 0 {
        let body = format!("**Approval needed: {} outreach drafts** to researchers and engineers with a public, relevant track record.\n\nThe drafts (person, public source, relevance, hypothesis, suggested message) are stored **privately on the agent machine** (Control Center → Discovery), never in this public repository. Nothing has been sent. Send the ones you like personally; record replies as interview summaries.", g.outreach_count);
        consider("R:outreach".into(), format!("Approval: outreach drafts ({})", g.week), body, vec![LABEL_APPROVAL.into(), g.needs_human.into()], &mut gate);
    }
    // nothing personal or confidential goes into a public issue
    gate.items.retain(|it| {
        let problems = privacy_problems(&format!("{}\n{}", it.title, it.body));
        if problems.is_empty() {
            return true;
        }
        gate.blocked_privacy.push(format!("{} — {}", it.key, problems.join("; ")));
        false
    });
    gate
}

/// Files the gated items (at most `max_new` new issues), parks/closes retired opportunities.
/// Returns (key, issue number) of every item that exists afterwards.
pub fn file_items(forge: &dyn Forge, gate: &mut Gate, run: &mut RunRecord, max_new: usize, persist: &mut dyn FnMut(&RunRecord)) -> Result<Vec<(String, u64)>, String> {
    let mut existing: BTreeMap<String, u64> = BTreeMap::new();
    for label in [LABEL_OPPORTUNITY, LABEL_EXPERIMENT, LABEL_INTERVIEW, LABEL_APPROVAL] {
        for i in forge.issues(label, "all")? {
            if let Some(key) = i.body.as_deref().and_then(item_key) {
                existing.entry(key).or_insert(i.number);
            }
        }
    }
    let mut out = Vec::new();
    for it in std::mem::take(&mut gate.items) {
        if let Some(n) = run.applied.get(&it.key).copied().or_else(|| existing.get(&it.key).copied()) {
            run.applied.insert(it.key.clone(), n);
            out.push((it.key, n));
            continue;
        }
        if run.created.len() >= max_new {
            gate.deferred.push(format!("{} (max {max_new} new issues per run)", it.key));
            continue;
        }
        let issue = forge.create_issue(&it.title, &it.body)?;
        // recorded before labelling: a crash in between must not file it again
        run.applied.insert(it.key.clone(), issue.number);
        run.created.push(it.key.clone());
        persist(run);
        let labels: Vec<&str> = it.labels.iter().map(String::as_str).collect();
        forge.add_labels(issue.number, &labels)?;
        out.push((it.key, issue.number));
    }
    for (key, reason) in std::mem::take(&mut gate.retire) {
        if let Some(n) = existing.get(&format!("O:{key}")).copied() {
            let i = forge.issue(n)?;
            if i.state == "open" {
                forge.comment(n, &format!("Parked by the weekly product discovery: {}", if reason.trim().is_empty() { "no reason given" } else { reason.trim() }))?;
                forge.add_labels(n, &[LABEL_PARKED])?;
                forge.close_issue(n)?;
            }
        }
    }
    Ok(out)
}

/// The key of a discovery-filed issue body.
pub fn item_key(body: &str) -> Option<String> {
    let start = body.find("<!-- discovery:item key=")? + "<!-- discovery:item key=".len();
    let end = body[start..].find(" -->")?;
    Some(body[start..start + end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeForge;

    fn verdict(item: &str, v: &str) -> Verdict {
        Verdict { item: item.into(), verdict: v.into(), reason: format!("{v} because"), strongest_counterargument: String::new() }
    }

    fn input<'a>(experiments: &'a [Experiment]) -> GateInput<'a> {
        GateInput { week: "2026-w45", task_label: "agent-task", needs_human: "needs-human", integration_branch: "dev", open_opportunities: 2, max_open_opportunities: 3, max_new_experiments: 1, max_spikes: 1, experiments, pages: &[], campaigns: &[], outreach_count: 0 }
    }

    fn opp(key: &str) -> Opportunity {
        Opportunity { key: key.into(), title: key.into(), problem: "p".into(), evidence: vec!["E-2026-w45-01".into()], action: "open".into(), ..Default::default() }
    }

    #[test]
    fn critic_and_caps_gate_every_item() {
        let actions = Actions { summary: "s".into(), opportunities: vec![opp("a"), opp("b"), opp("c"), Opportunity { key: "old".into(), action: "park".into(), reason: "no evidence".into(), ..Default::default() }], spikes: vec![Spike { key: "s1".into(), title: "t".into(), question: "q".into(), acceptance: "a".into() }, Spike { key: "s2".into(), title: "t".into(), question: "q".into(), acceptance: "a".into() }], ..Default::default() };
        let xs = vec![Experiment { id: "X-2026-w45-one".into(), title: "One".into(), method: "landing-page".into(), autonomy: "needs-approval".into(), status: "proposed".into() }, Experiment { id: "X-2026-w45-two".into(), method: "spike".into(), autonomy: "autonomous".into(), ..Default::default() }];
        let critique = Critique { verdicts: vec![verdict("O:a", "reject"), verdict("O:b", "stands"), verdict("O:c", "weaken"), verdict("X-2026-w45-one", "stands"), verdict("S:s1", "stands"), verdict("S:s2", "stands")], ..Default::default() };
        let g = plan_items(&actions, &critique, &input(&xs));
        let keys: Vec<&str> = g.items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["X-2026-w45-one", "O:b", "S:s1"]);
        assert!(g.rejected[0].starts_with("O:a"));
        assert!(g.deferred.iter().any(|d| d.starts_with("X-2026-w45-two")), "experiment cap");
        assert!(g.deferred.iter().any(|d| d.starts_with("O:c")), "opportunity cap: 2 open + b = 3");
        assert!(g.deferred.iter().any(|d| d.starts_with("S:s2")), "spike cap");
        assert_eq!(g.retire, vec![("old".to_string(), "no evidence".to_string())]);
        let x = &g.items[0];
        assert!(x.labels.contains(&"discovery-approval".to_string()) && x.labels.contains(&"needs-human".to_string()), "external method needs approval");
        assert!(x.body.contains("<!-- discovery:item key=X-2026-w45-one -->") && x.body.contains("Critic (stands)"));
        let spike = &g.items[2];
        assert!(spike.labels == vec!["agent-task"] && spike.body.contains("Team branch: spike/s1"), "spikes run through the coder on a throwaway branch");
    }

    #[test]
    fn missing_verdicts_are_not_filed_and_private_data_is_blocked() {
        let actions = Actions { summary: "s".into(), interviews: vec![Interview { key: "k".into(), title: "t".into(), target_profile: "PIC designer".into(), questions: vec!["a".into(), "b".into(), "c".into()], why: "Ask jane@uni.edu".into(), ..Default::default() }], opportunities: vec![opp("z")], ..Default::default() };
        let g = plan_items(&actions, &Critique { verdicts: vec![verdict("I:k", "stands")], ..Default::default() }, &input(&[]));
        assert_eq!(g.unreviewed, vec!["O:z"]);
        assert!(g.items.is_empty());
        assert!(g.blocked_privacy[0].starts_with("I:k"));
    }

    #[test]
    fn filing_is_idempotent_and_capped() {
        let forge = FakeForge::new("o/r");
        let mut run = RunRecord { week: "2026-w45".into(), ..Default::default() };
        let item = |k: &str| Item { key: k.into(), title: k.into(), body: format!("{}\nbody", marker(k)), labels: vec![LABEL_OPPORTUNITY.into()] };
        let mut gate = Gate { items: vec![item("O:a"), item("O:b"), item("O:c")], ..Default::default() };
        let mut saved = 0;
        let out = file_items(&forge, &mut gate, &mut run, 2, &mut |_| saved += 1).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(saved, 2, "each creation is persisted before labelling");
        assert!(gate.deferred[0].starts_with("O:c"));
        // resumed run (fresh record, e.g. other machine): existing issues are found by marker
        let mut again = Gate { items: vec![item("O:a"), item("O:b")], ..Default::default() };
        let mut fresh = RunRecord::default();
        let out2 = file_items(&forge, &mut again, &mut fresh, 2, &mut |_| {}).unwrap();
        assert_eq!(out, out2);
        assert_eq!(forge.log().iter().filter(|l| l.starts_with("issue ")).count(), 2, "no duplicates");
    }

    #[test]
    fn retiring_parks_the_existing_opportunity() {
        let forge = FakeForge::new("o/r").with_issue(crate::fake::issue(7, "Opportunity: x", &format!("{}\n", marker("O:x")), &[LABEL_OPPORTUNITY]));
        let mut gate = Gate { retire: vec![("x".into(), "contradicted by E-2026-w45-02".into())], ..Default::default() };
        file_items(&forge, &mut gate, &mut RunRecord::default(), 5, &mut |_| {}).unwrap();
        assert_eq!(forge.issue(7).unwrap().state, "closed");
        assert!(forge.labels(7).contains(&LABEL_PARKED.to_string()));
        assert!(forge.comment_bodies(7)[0].contains("contradicted"));
    }

    #[test]
    fn actions_are_validated() {
        let bad = Actions { summary: String::new(), opportunities: vec![Opportunity { key: "Bad Key".into(), action: "maybe".into(), ..Default::default() }], outreach: vec![Outreach { name: "R".into(), public_source: "https://uni.edu/r".into(), hypothesis: "h".into(), draft: "d".into(), channel: "r@uni.edu".into(), ..Default::default() }], ..Default::default() };
        let errs = check_actions(&bad);
        for needle in ["summary", "lower-case", "open | park | close", "no e-mail"] {
            assert!(errs.iter().any(|e| e.contains(needle)), "{needle}: {errs:?}");
        }
    }
}
