//! The discovery data model as it lives in the repository (`discovery/`): parsers and
//! validators for evidence entries, the hypothesis register, Decision Records, experiment
//! pre-registrations, campaign plans and landing-page specs. Everything an LLM stage writes
//! passes through these checks before it is committed; see docs/DISCOVERY.md.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

pub const ROOT: &str = "discovery";

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).expect("static regex"))
}

/// `- key: value` lines of a block (first occurrence wins, keys lower-cased).
pub fn fields(block: &str) -> BTreeMap<String, String> {
    static LINE: OnceLock<Regex> = OnceLock::new();
    let mut out = BTreeMap::new();
    for c in re(&LINE, r"(?m)^[ \t]*[-*][ \t]*([A-Za-z_ -]+?)[ \t]*:[ \t]*(.*)$").captures_iter(block) {
        out.entry(c[1].trim().to_lowercase().replace(' ', "_")).or_insert_with(|| c[2].trim().to_string());
    }
    out
}

/// Comma-separated ids (`H1, H2`), empty for "none"/"-".
pub fn id_list(raw: &str) -> Vec<String> {
    raw.split([',', ';', ' ']).map(|s| s.trim().trim_matches(['[', ']', '`'])).filter(|s| !s.is_empty() && !matches!(s.to_lowercase().as_str(), "none" | "-" | "n/a")).map(String::from).collect()
}

// ---------------------------------------------------------------- evidence

/// Where a piece of evidence comes from. The class caps the demand-evidence level: internal
/// tests and simulated personas say something about feasibility or usability, never about
/// external demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    AgentHypothesis,
    InternalTest,
    Simulated,
    External,
    UserFeedback,
    UserCommitment,
}

impl Class {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "agent-hypothesis" => Self::AgentHypothesis,
            "internal-test" => Self::InternalTest,
            "simulated" => Self::Simulated,
            "external" => Self::External,
            "user-feedback" => Self::UserFeedback,
            "user-commitment" => Self::UserCommitment,
            _ => return None,
        })
    }
    pub fn max_level(self) -> u8 {
        match self {
            Self::AgentHypothesis | Self::InternalTest | Self::Simulated => 0,
            Self::External => 1,
            Self::UserFeedback => 2,
            Self::UserCommitment => 3,
        }
    }
    pub const NAMES: &'static str = "agent-hypothesis | internal-test | simulated | external | user-feedback | user-commitment";
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub id: String,
    pub class: Option<Class>,
    pub level: Option<u8>,
    pub source: String,
    pub quote: String,
    pub supports: Vec<String>,
    pub contradicts: Vec<String>,
    /// the whole `### E-…` block, verbatim
    pub raw: String,
}

/// `2026-w45.md` — evidence files; README and other notes in the folder are not evidence.
pub fn is_evidence_file(name: &str) -> bool {
    static NAME: OnceLock<Regex> = OnceLock::new();
    re(&NAME, r"^\d{4}-w\d{2}\.md$").is_match(name)
}

pub fn parse_level(s: &str) -> Option<u8> {
    let s = s.trim().to_uppercase();
    s.strip_prefix('E').and_then(|n| n.parse::<u8>().ok()).filter(|n| *n <= 3)
}

/// The `### E-…` entries of an evidence file, in order. Text before the first entry is the
/// file's header and is kept by `rewrite_evidence`.
pub fn parse_evidence(text: &str) -> Vec<Evidence> {
    let mut out = Vec::new();
    let starts: Vec<usize> = text.match_indices("\n### E-").map(|(i, _)| i + 1).chain(text.starts_with("### E-").then_some(0)).collect::<BTreeSet<_>>().into_iter().collect();
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(text.len());
        let raw = text[start..end].trim_end().to_string();
        let id = raw.lines().next().unwrap_or("").trim_start_matches('#').split_whitespace().next().unwrap_or("").to_string();
        let f = fields(&raw);
        let get = |k: &str| f.get(k).cloned().unwrap_or_default();
        out.push(Evidence {
            id,
            class: Class::parse(&get("class")),
            level: parse_level(&get("level")),
            source: get("source"),
            quote: get("quote").trim_matches(['"', '“', '”', '>', ' ']).to_string(),
            supports: id_list(&get("supports")),
            contradicts: id_list(&get("contradicts")),
            raw,
        });
    }
    out
}

/// The header of an evidence file plus `keep` entries.
pub fn rewrite_evidence(text: &str, keep: &[&Evidence]) -> String {
    let header = text.find("### E-").map(|i| &text[..i]).unwrap_or(text).trim_end();
    let mut out = header.to_string();
    for e in keep {
        out.push_str("\n\n");
        out.push_str(&e.raw);
    }
    out.push('\n');
    out
}

/// Lower-cased URL without scheme, `www.`, fragment, tracking parameters and trailing `/`.
pub fn normalize_url(url: &str) -> String {
    let u = url.trim().trim_matches(['<', '>', '(', ')']).to_lowercase();
    let u = u.split('#').next().unwrap_or("");
    let u = u.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    let (path, query) = u.split_once('?').unwrap_or((u, ""));
    let kept: Vec<&str> = query.split('&').filter(|p| !p.is_empty() && !p.starts_with("utm_") && !p.starts_with("ref=")).collect();
    let path = path.trim_end_matches('/');
    if kept.is_empty() { path.to_string() } else { format!("{path}?{}", kept.join("&")) }
}

fn norm_words(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Same source and (nearly) the same quote = the same finding.
pub fn dedup_key(e: &Evidence) -> String {
    let q: String = norm_words(&e.quote).chars().take(80).collect();
    format!("{}|{q}", normalize_url(&e.source))
}

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// Problems of one entry (empty = valid). `week` is the file's week ("2026-w44").
pub fn check_evidence(e: &Evidence, week: &str) -> Vec<String> {
    let mut errs = Vec::new();
    let id_ok = e.id.strip_prefix(&format!("E-{week}-")).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if !id_ok {
        errs.push(format!("{}: id must be E-{week}-NN", e.id));
    }
    let Some(class) = e.class else {
        errs.push(format!("{}: class must be one of {}", e.id, Class::NAMES));
        return errs;
    };
    match e.level {
        None => errs.push(format!("{}: level must be E0–E3", e.id)),
        Some(l) if l > class.max_level() => errs.push(format!("{}: level E{l} is above what class {:?} can support (max E{}); internal tests and simulated users are never evidence of external demand", e.id, class, class.max_level())),
        _ => {}
    }
    match class {
        Class::AgentHypothesis => {}
        Class::External | Class::InternalTest | Class::Simulated => {
            if !is_url(&e.source) && !e.source.starts_with('#') && !e.source.contains('/') {
                errs.push(format!("{}: source must be a URL, an issue (#123) or a repository path", e.id));
            }
        }
        Class::UserFeedback | Class::UserCommitment => {
            // direct user evidence only enters through artefacts a human produced: an
            // interview summary, a user's own issue, or recorded metrics
            static OK: OnceLock<Regex> = OnceLock::new();
            if !re(&OK, r"^(I-\d{4}|#\d+|metrics:[a-z0-9-]+|https://github\.com/[^/]+/[^/]+/(issues|discussions)/\d+)").is_match(&e.source) {
                errs.push(format!("{}: user evidence needs an interview (I-0001), a user's issue (#123 or its URL) or metrics:<page> as source", e.id));
            }
        }
    }
    if class != Class::AgentHypothesis && e.quote.is_empty() {
        errs.push(format!("{}: quote is required (verbatim excerpt)", e.id));
    }
    if e.quote.chars().count() > 600 {
        errs.push(format!("{}: quote longer than 600 characters", e.id));
    }
    errs
}

// ---------------------------------------------------------------- hypotheses

pub const HYPOTHESIS_STATUS: &[&str] = &["open", "testing", "supported", "weakened", "dropped", "superseded"];

/// `## H3: …` sections → (id, status).
pub fn parse_hypotheses(text: &str) -> Vec<(String, String)> {
    static HEAD: OnceLock<Regex> = OnceLock::new();
    let heads: Vec<(usize, String)> = re(&HEAD, r"(?m)^##\s+(H\d+)\b").captures_iter(text).map(|c| (c.get(0).unwrap().start(), c[1].to_string())).collect();
    heads
        .iter()
        .enumerate()
        .map(|(k, (start, id))| {
            let end = heads.get(k + 1).map(|h| h.0).unwrap_or(text.len());
            (id.clone(), fields(&text[*start..end]).get("status").cloned().unwrap_or_default().to_lowercase())
        })
        .collect()
}

/// Hypotheses may be added and re-rated, never deleted (drop them with a reason instead).
pub fn check_hypotheses(before: &str, after: &str) -> Vec<String> {
    let now = parse_hypotheses(after);
    let mut errs: Vec<String> = parse_hypotheses(before).into_iter().filter(|(id, _)| !now.iter().any(|(n, _)| n == id)).map(|(id, _)| format!("HYPOTHESES.md: {id} was deleted — set `status: dropped` with a reason instead")).collect();
    for (id, status) in &now {
        if !HYPOTHESIS_STATUS.contains(&status.as_str()) {
            errs.push(format!("HYPOTHESES.md: {id} status „{status}“ must be one of {}", HYPOTHESIS_STATUS.join(" | ")));
        }
    }
    if now.is_empty() {
        errs.push("HYPOTHESES.md: no `## H<n>` sections".into());
    }
    errs
}

// ---------------------------------------------------------------- sections

/// Body of the `## <name>` section (up to the next `## `), if present.
pub fn section<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("## {}", name.to_lowercase());
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let l = line.trim().to_lowercase();
        if l == needle || l.starts_with(&format!("{needle} ")) || l.starts_with(&format!("{needle}:")) {
            let start = offset + line.len();
            let rest = &text[start..];
            let end = rest.match_indices("\n## ").next().map(|(i, _)| i + 1).unwrap_or(rest.len());
            return Some(&rest[..end]);
        }
        offset += line.len();
    }
    None
}

/// Text with the `## <name>` section (heading and body) removed.
pub fn without_section(text: &str, name: &str) -> String {
    let Some(body) = section(text, name) else { return text.to_string() };
    let body_start = body.as_ptr() as usize - text.as_ptr() as usize;
    let head_start = text[..body_start].trim_end_matches('\n').rfind('\n').map(|i| i + 1).unwrap_or(0);
    format!("{}{}", &text[..head_start], &text[body_start + body.len()..])
}

// ---------------------------------------------------------------- decision records

pub const DR_STATUS: &[&str] = &["proposed", "accepted", "rejected", "superseded"];
pub const DR_DECISION: &[&str] = &["pursue", "experiment", "park", "kill", "do-nothing", "need-more-evidence"];
pub const VERDICTS: &[&str] = &["stands", "weaken", "reject"];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Decision {
    pub id: String,
    pub title: String,
    pub week: String,
    pub status: String,
    pub decision: String,
    pub evidence: Vec<String>,
    pub critic_verdict: Option<String>,
    /// `- [ ] …` / `- …` lines of `## Delivery` (empty = no delivery consequence)
    pub delivery: Vec<String>,
}

pub fn parse_decision(text: &str) -> Decision {
    static HEAD: OnceLock<Regex> = OnceLock::new();
    let (id, title) = re(&HEAD, r"(?m)^#\s+(DR-\d{4})\s*:\s*(.+)$").captures(text).map(|c| (c[1].to_string(), c[2].trim().to_string())).unwrap_or_default();
    let head_end = text.find("\n## ").unwrap_or(text.len());
    let f = fields(&text[..head_end]);
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let delivery = section(text, "Delivery")
        .map(|b| b.lines().map(str::trim).filter(|l| l.starts_with('-') || l.starts_with('*')).map(|l| l.trim_start_matches(['-', '*', ' ']).trim_start_matches("[ ]").trim_start_matches("[x]").trim().to_string()).filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("none")).collect())
        .unwrap_or_default();
    Decision {
        id,
        title,
        week: get("week"),
        status: get("status").to_lowercase(),
        decision: get("decision").to_lowercase(),
        evidence: id_list(&get("evidence")),
        critic_verdict: section(text, "Critic").and_then(|b| fields(b).get("verdict").map(|v| v.to_lowercase())),
        delivery,
    }
}

/// Problems of a Decision Record. `levels` maps known evidence ids to their level;
/// `need_critic` once the critic stage has run.
pub fn check_decision(file: &str, text: &str, levels: &BTreeMap<String, u8>, need_critic: bool) -> Vec<String> {
    let d = parse_decision(text);
    let mut errs = Vec::new();
    let tag = if d.id.is_empty() { file.to_string() } else { d.id.clone() };
    if d.id.is_empty() {
        errs.push(format!("{file}: first line must be `# DR-NNNN: <question>`"));
    } else if !file.contains(&d.id) {
        errs.push(format!("{file}: file name must contain {}", d.id));
    }
    if !DR_STATUS.contains(&d.status.as_str()) {
        errs.push(format!("{tag}: status must be one of {}", DR_STATUS.join(" | ")));
    }
    if !DR_DECISION.contains(&d.decision.as_str()) {
        errs.push(format!("{tag}: decision must be one of {}", DR_DECISION.join(" | ")));
    }
    let options = section(text, "Options").unwrap_or("");
    let n_options = options.lines().filter(|l| l.trim_start().starts_with("### ")).count();
    if n_options < 2 {
        errs.push(format!("{tag}: `## Options` needs at least two `### …` options"));
    }
    if !norm_words(options).contains("do nothing") && !norm_words(options).contains("status quo") {
        errs.push(format!("{tag}: one option must be „do nothing“ / status quo"));
    }
    for e in &d.evidence {
        if !levels.contains_key(e) {
            errs.push(format!("{tag}: cites unknown evidence {e}"));
        }
    }
    if d.decision == "pursue" && !d.evidence.iter().any(|e| levels.get(e).is_some_and(|l| *l >= 2)) {
        errs.push(format!("{tag}: `pursue` needs at least one cited evidence entry of level E2 or higher (direct user evidence); use `experiment` or `need-more-evidence`"));
    }
    if section(text, "Delivery").is_none() {
        errs.push(format!("{tag}: `## Delivery` section missing (write `- none` if there is no delivery consequence)"));
    }
    if need_critic {
        match &d.critic_verdict {
            None => errs.push(format!("{tag}: `## Critic` section with `- verdict: stands | weaken | reject` missing")),
            Some(v) if !VERDICTS.contains(&v.as_str()) => errs.push(format!("{tag}: critic verdict must be one of {}", VERDICTS.join(" | "))),
            _ => {}
        }
    }
    errs
}

// ---------------------------------------------------------------- experiments

pub const METHODS: &[&str] = &["desk-research", "spike", "benchmark", "persona-journey", "interview", "landing-page", "campaign", "pilot", "outreach"];
/// methods that reach real people or the public: never autonomous
pub const EXTERNAL_METHODS: &[&str] = &["interview", "landing-page", "campaign", "pilot", "outreach"];
pub const EXPERIMENT_STATUS: &[&str] = &["proposed", "approved", "running", "done", "abandoned", "rejected-by-critic"];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Experiment {
    pub id: String,
    pub title: String,
    pub status: String,
    pub method: String,
    pub autonomy: String,
}

pub fn parse_experiment(text: &str) -> Experiment {
    static HEAD: OnceLock<Regex> = OnceLock::new();
    let (id, title) = re(&HEAD, r"(?m)^#\s+(X-\d{4}-w\d{2}-[a-z0-9-]+)\s*:\s*(.+)$").captures(text).map(|c| (c[1].to_string(), c[2].trim().to_string())).unwrap_or_default();
    let head_end = text.find("\n## ").unwrap_or(text.len());
    let f = fields(&text[..head_end]);
    let get = |k: &str| f.get(k).cloned().unwrap_or_default().to_lowercase();
    Experiment { id, title, status: get("status"), method: get("method"), autonomy: get("autonomy") }
}

/// Problems of an experiment file. `before` = the version already on the integration
/// branch: its pre-registration must not change (results are appended, criteria stay).
pub fn check_experiment(file: &str, text: &str, before: Option<&str>) -> Vec<String> {
    let x = parse_experiment(text);
    let tag = if x.id.is_empty() { file.to_string() } else { x.id.clone() };
    let mut errs = Vec::new();
    if x.id.is_empty() {
        errs.push(format!("{file}: first line must be `# X-<yyyy>-w<ww>-<slug>: <title>`"));
    } else if !file.contains(&x.id) {
        errs.push(format!("{file}: file name must contain {}", x.id));
    }
    if !METHODS.contains(&x.method.as_str()) {
        errs.push(format!("{tag}: method must be one of {}", METHODS.join(" | ")));
    }
    if !EXPERIMENT_STATUS.contains(&x.status.as_str()) {
        errs.push(format!("{tag}: status must be one of {}", EXPERIMENT_STATUS.join(" | ")));
    }
    let external = EXTERNAL_METHODS.contains(&x.method.as_str());
    if external && x.autonomy != "needs-approval" {
        errs.push(format!("{tag}: method {} reaches real people — autonomy must be needs-approval", x.method));
    }
    if !external && !["autonomous", "needs-approval"].contains(&x.autonomy.as_str()) {
        errs.push(format!("{tag}: autonomy must be autonomous | needs-approval"));
    }
    match section(text, "Pre-registration") {
        None => errs.push(format!("{tag}: `## Pre-registration` with success, failure and measurement missing")),
        Some(pre) => {
            let f = fields(pre);
            for k in ["success", "failure", "measurement", "timebox"] {
                if f.get(k).is_none_or(|v| v.trim().is_empty()) {
                    errs.push(format!("{tag}: pre-registration needs `- {k}: …`"));
                }
            }
            if let Some(b) = before.and_then(|b| section(b, "Pre-registration")) {
                if norm_words(b) != norm_words(pre) {
                    errs.push(format!("{tag}: the pre-registration changed after registration — append results under `## Results` instead and propose a new experiment for new criteria"));
                }
            }
        }
    }
    errs
}

// ---------------------------------------------------------------- campaigns

/// Problems of a campaign plan. The authorised budget is set by the maintainer alone.
pub fn check_campaign(file: &str, text: &str, before: Option<&str>) -> Vec<String> {
    let head_end = text.find("\n## ").unwrap_or(text.len());
    let f = fields(&text[..head_end]);
    let mut errs = Vec::new();
    for k in ["audience", "hypothesis", "channel", "landing_page", "proposed_budget_usd", "max_spend_usd", "duration_days", "stop_conditions", "measurement", "authorized_budget_usd"] {
        if f.get(k).is_none_or(|v| v.trim().is_empty()) {
            errs.push(format!("{file}: campaign plan needs `- {}: …`", k.replace('_', " ")));
        }
    }
    let authorised = |t: &str| {
        let he = t.find("\n## ").unwrap_or(t.len());
        fields(&t[..he]).get("authorized_budget_usd").map(|v| v.trim().trim_start_matches('$').parse::<f64>().unwrap_or(f64::NAN)).unwrap_or(0.0)
    };
    let was = before.map(authorised).unwrap_or(0.0);
    let now = authorised(text);
    if now != was && !(now == 0.0 && was.is_nan()) {
        errs.push(format!("{file}: authorized budget may only be changed by the maintainer (keep {was})"));
    }
    if section(text, "Compliance").is_none() {
        errs.push(format!("{file}: `## Compliance` checklist (platform policy, consent, privacy, no personal data) missing"));
    }
    errs
}

// ---------------------------------------------------------------- landing pages

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Capability {
    pub text: String,
    /// available | planned | research
    pub status: String,
    /// repository path proving an `available` claim (docs, code, test)
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Figure {
    /// repository path of an existing image (e.g. docs/media/…png)
    pub src: String,
    pub caption: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Cta {
    pub label: String,
    /// pilot | waitlist | feedback
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PageSpec {
    /// experiment id this page belongs to
    pub experiment: String,
    pub slug: String,
    pub title: String,
    pub tagline: String,
    pub problem: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub figures: Vec<Figure>,
    pub cta: Vec<Cta>,
    pub honesty_note: String,
    /// draft | approved — only the maintainer approves
    pub status: String,
    pub approved_by: String,
    pub approved_on: String,
}

/// Problems of a landing-page spec (`discovery/pages/<slug>/page.json`). `repo` resolves
/// cited sources and figures; `before` = the spec on the integration branch.
pub fn check_page(file: &str, text: &str, repo: &Path, before: Option<&str>) -> Vec<String> {
    let spec: PageSpec = match serde_json::from_str(text) {
        Ok(s) => s,
        Err(e) => return vec![format!("{file}: not valid page JSON ({e})")],
    };
    let mut errs = Vec::new();
    static SLUG: OnceLock<Regex> = OnceLock::new();
    if !re(&SLUG, r"^[a-z0-9][a-z0-9-]{1,40}$").is_match(&spec.slug) || !file.contains(&format!("/{}/", spec.slug)) {
        errs.push(format!("{file}: slug must be lower-case-with-dashes and match the folder name"));
    }
    if spec.title.trim().is_empty() || spec.tagline.trim().is_empty() || spec.problem.is_empty() {
        errs.push(format!("{file}: title, tagline and problem are required"));
    }
    if spec.honesty_note.trim().len() < 40 {
        errs.push(format!("{file}: honesty_note must say plainly what is not available yet and that the page tests interest"));
    }
    if spec.cta.is_empty() || spec.cta.iter().any(|c| !["pilot", "waitlist", "feedback"].contains(&c.kind.as_str()) || c.label.trim().is_empty()) {
        errs.push(format!("{file}: cta needs label and kind pilot | waitlist | feedback"));
    }
    if spec.capabilities.iter().all(|c| c.status != "available") && spec.capabilities.iter().all(|c| c.status != "planned") {
        errs.push(format!("{file}: list at least one capability"));
    }
    static CLAIM: OnceLock<Regex> = OnceLock::new();
    let claim = re(&CLAIM, r"(?i)\b\d+(\.\d+)?\s*(x|×|%|times)\b|\b(faster|fastest|best|leading|first|guaranteed|production[- ]ready)\b");
    let mut texts: Vec<&str> = vec![&spec.title, &spec.tagline];
    texts.extend(spec.problem.iter().map(String::as_str));
    for c in &spec.capabilities {
        if !["available", "planned", "research"].contains(&c.status.as_str()) {
            errs.push(format!("{file}: capability status must be available | planned | research"));
        }
        if c.status == "available" && (c.source.trim().is_empty() || !repo.join(&c.source).exists()) {
            errs.push(format!("{file}: available capability „{}“ must cite an existing repository path as source", c.text));
        }
        texts.push(&c.text);
    }
    for t in texts {
        if let Some(m) = claim.find(t) {
            errs.push(format!("{file}: unsupported performance or superlative claim „{}“ — state verifiable facts only", m.as_str()));
        }
    }
    for f in &spec.figures {
        if f.src.contains("://") || !repo.join(&f.src).is_file() {
            errs.push(format!("{file}: figure {} must be an existing image in the repository", f.src));
        }
    }
    let approved_before = before.and_then(|b| serde_json::from_str::<PageSpec>(b).ok()).is_some_and(|b| !b.approved_by.is_empty());
    if !approved_before && (spec.status != "draft" || !spec.approved_by.is_empty() || !spec.approved_on.is_empty()) {
        errs.push(format!("{file}: status must stay draft with empty approved_by/approved_on — only the maintainer approves a page"));
    }
    errs
}

// ---------------------------------------------------------------- privacy

/// Things that must never reach the public repository or a public issue: e-mail
/// addresses, phone numbers, private paths, secrets, confidentiality markers.
pub fn privacy_problems(text: &str) -> Vec<String> {
    static EMAIL: OnceLock<Regex> = OnceLock::new();
    static PHONE: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    static SECRET: OnceLock<Regex> = OnceLock::new();
    static CONF: OnceLock<Regex> = OnceLock::new();
    let mut out = Vec::new();
    for m in re(&EMAIL, r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").find_iter(text) {
        let addr = m.as_str().to_lowercase();
        if !(addr.ends_with("@example.com") || addr.ends_with(".invalid") || addr.starts_with("noreply@")) {
            out.push(format!("e-mail address {}", m.as_str()));
        }
    }
    if let Some(m) = re(&PHONE, r"\+\d{1,3}[\s-]?\(?\d{2,4}\)?[\s-]?\d{3,4}[\s-]?\d{3,5}").find(text) {
        out.push(format!("phone number {}", m.as_str()));
    }
    if let Some(m) = re(&PATH, r#"(?i)\b[a-z]:\\users\\[^\s"']+|/home/[a-z0-9_-]+/[^\s"')]*|/Users/[A-Za-z0-9_-]+/[^\s"')]*"#).find(text) {
        out.push(format!("private path {}", m.as_str()));
    }
    if let Some(m) = re(&SECRET, r"\b(ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-ant-[A-Za-z0-9-]{20,}|sk-[A-Za-z0-9]{32,}|AKIA[0-9A-Z]{16})").find(text) {
        out.push(format!("secret-like token {}…", &m.as_str()[..6]));
    }
    if let Some(m) = re(&CONF, r"(?i)\b(confidential|proprietary|internal only|under nda|akhetonics[- ]internal)\b").find(text) {
        out.push(format!("confidentiality marker „{}“", m.as_str()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EV: &str = "# Evidence 2026-w44\n\nheader text\n\n### E-2026-w44-01\n- class: external\n- level: E1\n- source: https://github.com/gdsfactory/gdsfactory/issues/1?utm_source=x\n- quote: \"Routing between two arrays takes hours\"\n- supports: H1, H2\n- contradicts: none\n\n### E-2026-w44-02\n- class: agent-hypothesis\n- level: E0\n- note: users might want X\n";

    #[test]
    fn evidence_parses_and_rewrites() {
        let list = parse_evidence(EV);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "E-2026-w44-01");
        assert_eq!(list[0].class, Some(Class::External));
        assert_eq!(list[0].supports, vec!["H1", "H2"]);
        assert!(list[0].contradicts.is_empty());
        assert_eq!(list[0].quote, "Routing between two arrays takes hours");
        assert!(check_evidence(&list[0], "2026-w44").is_empty(), "{:?}", check_evidence(&list[0], "2026-w44"));
        assert!(check_evidence(&list[1], "2026-w44").is_empty());
        let only_second = rewrite_evidence(EV, &[&list[1]]);
        assert!(only_second.starts_with("# Evidence 2026-w44\n\nheader text\n\n### E-2026-w44-02"));
        assert!(!only_second.contains("E-2026-w44-01"));
    }

    #[test]
    fn evidence_levels_are_capped_by_class() {
        let mk = |class: &str, level: &str, source: &str| parse_evidence(&format!("### E-2026-w44-07\n- class: {class}\n- level: {level}\n- source: {source}\n- quote: \"x\"\n")).remove(0);
        assert!(check_evidence(&mk("simulated", "E2", "https://x.org"), "2026-w44").iter().any(|e| e.contains("never evidence of external demand")));
        assert!(check_evidence(&mk("internal-test", "E1", "https://x.org"), "2026-w44").iter().any(|e| e.contains("above")));
        assert!(check_evidence(&mk("external", "E3", "https://x.org"), "2026-w44").iter().any(|e| e.contains("above")));
        // a web page is not a user talking to us
        assert!(check_evidence(&mk("user-feedback", "E2", "https://reddit.com/r/photonics/x"), "2026-w44").iter().any(|e| e.contains("interview")));
        assert!(check_evidence(&mk("user-feedback", "E2", "I-0003"), "2026-w44").is_empty());
        assert!(check_evidence(&mk("user-commitment", "E3", "metrics:gds-reconstruction"), "2026-w44").is_empty());
        assert!(check_evidence(&mk("external", "E1", "my opinion"), "2026-w44").iter().any(|e| e.contains("source")));
        assert!(check_evidence(&mk("bogus", "E1", "https://x"), "2026-w44").iter().any(|e| e.contains("class")));
        assert!(check_evidence(&mk("external", "E1", "https://x"), "2026-w45").iter().any(|e| e.contains("id must be")));
    }

    #[test]
    fn only_week_files_are_evidence() {
        assert!(is_evidence_file("2026-w45.md"));
        assert!(!is_evidence_file("README.md"), "the format example in the README is not evidence");
        assert!(!is_evidence_file("2026-w45.md.bak"));
    }

    #[test]
    fn duplicates_ignore_tracking_and_formatting() {
        let a = parse_evidence("### E-2026-w44-01\n- source: https://www.Example.org/a/?utm_source=x#frag\n- quote: Routing takes HOURS!\n").remove(0);
        let b = parse_evidence("### E-2026-w45-03\n- source: http://example.org/a\n- quote: \"routing takes hours\"\n").remove(0);
        let c = parse_evidence("### E-2026-w45-04\n- source: http://example.org/a\n- quote: a different finding on the same page\n").remove(0);
        assert_eq!(dedup_key(&a), dedup_key(&b));
        assert_ne!(dedup_key(&a), dedup_key(&c));
    }

    #[test]
    fn hypotheses_cannot_be_deleted() {
        let before = "## H0: status quo\n- status: open\n\n## H1: agent-first\n- status: open\n";
        assert!(check_hypotheses(before, "## H0: x\n- status: open\n## H1: y\n- status: weakened\n## H5: new\n- status: open\n").is_empty());
        assert!(check_hypotheses(before, "## H0: x\n- status: open\n")[0].contains("H1 was deleted"));
        assert!(check_hypotheses(before, "## H0: x\n- status: maybe\n## H1: y\n- status: open\n")[0].contains("maybe"));
    }

    fn dr(decision: &str, evidence: &str, critic: bool) -> String {
        format!(
            "# DR-0001: Should Lunima import foreign GDS?\n- week: 2026-w44\n- status: proposed\n- decision: {decision}\n- evidence: {evidence}\n\n## Options\n### A: Do nothing\nkeep\n### B: Build importer\nwork\n\n## Delivery\n- [ ] Spike: round-trip 3 public layouts — acceptance: report\n{}",
            if critic { "\n## Critic\n- verdict: weaken\n- strongest counter-argument: KLayout does this\n" } else { "" }
        )
    }

    #[test]
    fn decision_records_are_validated() {
        let levels: BTreeMap<String, u8> = [("E-2026-w44-01".to_string(), 1u8), ("I-0001-a".to_string(), 2u8)].into();
        let ok = dr("experiment", "E-2026-w44-01", true);
        assert!(check_decision("discovery/decisions/DR-0001-gds.md", &ok, &levels, true).is_empty(), "{:?}", check_decision("discovery/decisions/DR-0001-gds.md", &ok, &levels, true));
        let d = parse_decision(&ok);
        assert_eq!((d.id.as_str(), d.decision.as_str(), d.critic_verdict.as_deref()), ("DR-0001", "experiment", Some("weaken")));
        assert_eq!(d.delivery, vec!["Spike: round-trip 3 public layouts — acceptance: report"]);
        let errs = check_decision("discovery/decisions/DR-0001-gds.md", &dr("pursue", "E-2026-w44-01", true), &levels, true);
        assert!(errs.iter().any(|e| e.contains("E2 or higher")), "pursue on E1 only");
        assert!(check_decision("discovery/decisions/DR-0001-gds.md", &dr("pursue", "I-0001-a", true), &levels, true).is_empty());
        assert!(check_decision("discovery/decisions/DR-0001-gds.md", &dr("experiment", "E-9", true), &levels, true).iter().any(|e| e.contains("unknown evidence")));
        assert!(check_decision("discovery/decisions/DR-0001-gds.md", &dr("experiment", "", false), &levels, true).iter().any(|e| e.contains("Critic")));
        assert!(check_decision("discovery/decisions/DR-0001-gds.md", &dr("experiment", "", false), &levels, false).is_empty());
        let no_null = ok.replace("### A: Do nothing", "### A: Build exporter");
        assert!(check_decision("discovery/decisions/DR-0001-gds.md", &no_null, &levels, true).iter().any(|e| e.contains("do nothing")));
        assert!(check_decision("discovery/decisions/DR-0002-x.md", &ok, &levels, true).iter().any(|e| e.contains("file name")));
    }

    fn exp(method: &str, autonomy: &str, success: &str) -> String {
        format!("# X-2026-w44-gds-roundtrip: Round-trip public layouts\n- status: proposed\n- method: {method}\n- autonomy: {autonomy}\n\n## Pre-registration\n- success: {success}\n- failure: fewer than 2 of 3 layouts\n- measurement: pins detected / pins expected\n- timebox: 2026-11-20\n\n## Results\n")
    }

    #[test]
    fn experiments_are_preregistered_and_immutable() {
        let f = "discovery/experiments/X-2026-w44-gds-roundtrip.md";
        assert!(check_experiment(f, &exp("spike", "autonomous", "3 of 3"), None).is_empty());
        assert!(check_experiment(f, &exp("landing-page", "autonomous", "3 of 3"), None).iter().any(|e| e.contains("needs-approval")));
        assert!(check_experiment(f, &exp("spike", "autonomous", ""), None).iter().any(|e| e.contains("success")));
        let before = exp("spike", "autonomous", "3 of 3");
        let with_results = format!("{before}- 2026-11-18: 2 of 3 layouts reconstructed\n");
        assert!(check_experiment(f, &with_results, Some(&before)).is_empty(), "appending results is fine");
        assert!(check_experiment(f, &exp("spike", "autonomous", "2 of 3"), Some(&before)).iter().any(|e| e.contains("changed after registration")));
    }

    #[test]
    fn campaign_budget_belongs_to_the_maintainer() {
        let plan = |auth: &str| format!("# C-2026-w44-li: LinkedIn test\n- audience: PIC designers\n- hypothesis: H3\n- channel: LinkedIn\n- landing page: gds-reconstruction\n- proposed budget usd: 150\n- max spend usd: 150\n- duration days: 10\n- stop conditions: CPC > 5 USD\n- measurement: pilot requests\n- authorized budget usd: {auth}\n\n## Compliance\n- [ ] no personal data\n");
        assert!(check_campaign("c.md", &plan("0"), None).is_empty());
        assert!(check_campaign("c.md", &plan("150"), None).iter().any(|e| e.contains("only be changed by the maintainer")));
        assert!(check_campaign("c.md", &plan("150"), Some(&plan("150"))).is_empty(), "a budget the maintainer set stays");
    }

    #[test]
    fn pages_are_honest_drafts() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("docs")).unwrap();
        std::fs::write(t.path().join("docs/AGENT_BRIDGE.md"), "x").unwrap();
        let mut spec = PageSpec {
            experiment: "X-2026-w44-gds".into(),
            slug: "gds-reconstruction".into(),
            title: "Turn foreign GDS into editable circuits".into(),
            tagline: "Import a layout and get components and routes back".into(),
            problem: vec!["Layouts arrive as flat polygons.".into()],
            capabilities: vec![Capability { text: "MCP tools for routing repair".into(), status: "available".into(), source: "docs/AGENT_BRIDGE.md".into() }, Capability { text: "Pin recovery on foreign layouts".into(), status: "planned".into(), source: String::new() }],
            figures: vec![],
            cta: vec![Cta { label: "Join the pilot".into(), kind: "pilot".into() }],
            honesty_note: "Pin recovery on foreign layouts is not available yet; this page measures interest before we build it.".into(),
            status: "draft".into(),
            ..Default::default()
        };
        let f = "discovery/pages/gds-reconstruction/page.json";
        let check = |s: &PageSpec| check_page(f, &serde_json::to_string(s).unwrap(), t.path(), None);
        assert!(check(&spec).is_empty(), "{:?}", check(&spec));
        spec.capabilities[0].source = "docs/MISSING.md".into();
        assert!(check(&spec).iter().any(|e| e.contains("existing repository path")));
        spec.capabilities[0].source = "docs/AGENT_BRIDGE.md".into();
        spec.tagline = "10x faster routing".into();
        assert!(check(&spec).iter().any(|e| e.contains("claim")));
        spec.tagline = "Import a layout".into();
        spec.approved_by = "agent".into();
        assert!(check(&spec).iter().any(|e| e.contains("only the maintainer")));
    }

    #[test]
    fn privacy_scanner_catches_personal_and_confidential_data() {
        assert!(privacy_problems("Contact jane.doe@uni.edu for details").iter().any(|p| p.contains("e-mail")));
        assert!(privacy_problems("pilot@lunima.invalid and noreply@github.com are fine").is_empty());
        assert!(privacy_problems("call +49 89 1234 5678").iter().any(|p| p.contains("phone")));
        assert!(privacy_problems("see /home/max/secret/design.gds").iter().any(|p| p.contains("path")));
        assert!(privacy_problems("token ghp_abcdefghijklmnopqrstuvwxyz123").iter().any(|p| p.contains("secret")));
        assert!(privacy_problems("This chip is Akhetonics-internal").iter().any(|p| p.contains("confidential")));
        assert!(privacy_problems("Routing takes hours in KLayout, see https://github.com/x/y/issues/3").is_empty());
    }

    #[test]
    fn sections_are_found_and_removed() {
        let t = "# T\n\n## Options\nA\n\n## Critic\n- verdict: stands\n\n## Tail\nz\n";
        assert_eq!(section(t, "critic").unwrap().trim(), "- verdict: stands");
        assert_eq!(without_section(t, "Critic"), "# T\n\n## Options\nA\n\n## Tail\nz\n");
        assert!(section(t, "Missing").is_none());
    }
}
