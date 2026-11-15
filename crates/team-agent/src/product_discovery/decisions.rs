//! Decision Records and the maintainer's verdict on them.
//!
//! A DR is written as `proposed` on the week's branch. The maintainer accepts it by merging
//! the (draft) discovery PR, and can reject single DRs before merging with a PR comment
//! `/reject DR-0003 <reason>`. A merged DR without such a comment is accepted. The next run
//! writes that outcome into the file; until then `effective` computes it, so the Product
//! Owner sees accepted decisions right after the merge.

use super::model::{parse_decision, Decision, ROOT};
use super::run_record::RunRecord;
use crate::forge::Forge;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub struct Dr {
    /// path relative to the repository root
    pub file: String,
    pub text: String,
    pub d: Decision,
    /// status after applying the maintainer's merge / reject
    pub effective: String,
    /// "merge of PR #12" / "/reject in PR #12: reason"
    pub by: String,
}

/// All DR files of the checked-out branch, sorted by id.
pub fn load(repo: &Path) -> Vec<(String, String)> {
    let dir = repo.join(ROOT).join("decisions");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .map(|it| {
            it.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("DR-") && e.file_name().to_string_lossy().ends_with(".md"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok().map(|t| (format!("{ROOT}/decisions/{}", e.file_name().to_string_lossy()), t)))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// What the maintainer said in one discovery PR.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrVerdicts {
    pub merged: bool,
    /// DR id → reason
    pub rejected: BTreeMap<String, String>,
    /// sum of `review-minutes: N` lines
    pub review_minutes: u32,
}

pub fn pr_verdicts(forge: &dyn Forge, pr: u64) -> Result<PrVerdicts, String> {
    static REJECT: OnceLock<Regex> = OnceLock::new();
    static MINUTES: OnceLock<Regex> = OnceLock::new();
    let reject = REJECT.get_or_init(|| Regex::new(r"(?im)^\s*/reject\s+(DR-\d{4})\b[ :—-]*(.*)$").expect("static regex"));
    let minutes = MINUTES.get_or_init(|| Regex::new(r"(?im)^\s*review-minutes\s*:\s*(\d{1,4})\b").expect("static regex"));
    let p = forge.pr(pr)?;
    let mut v = PrVerdicts { merged: p.merged_at.is_some(), ..Default::default() };
    for c in forge.comments(pr)? {
        if c.body.contains("<!-- discovery:") {
            continue; // the agent's own comments
        }
        for m in reject.captures_iter(&c.body) {
            v.rejected.insert(m[1].to_uppercase(), m[2].trim().to_string());
        }
        for m in minutes.captures_iter(&c.body) {
            v.review_minutes += m[1].parse::<u32>().unwrap_or(0);
        }
    }
    Ok(v)
}

/// DRs with their effective status. PR lookups are cached per week; a GitHub error leaves
/// the file's own status.
pub fn effective(repo: &Path, forge: &dyn Forge) -> Vec<Dr> {
    let mut cache: BTreeMap<String, Option<(u64, PrVerdicts)>> = BTreeMap::new();
    load(repo)
        .into_iter()
        .map(|(file, text)| {
            let d = parse_decision(&text);
            let (mut effective, mut by) = (d.status.clone(), String::new());
            if d.status == "proposed" && !d.week.is_empty() {
                let entry = cache.entry(d.week.clone()).or_insert_with(|| {
                    let pr = RunRecord::load(repo, &d.week).pr?;
                    pr_verdicts(forge, pr).ok().map(|v| (pr, v))
                });
                if let Some((pr, v)) = entry {
                    if let Some(reason) = v.rejected.get(&d.id) {
                        effective = "rejected".into();
                        by = format!("/reject in PR #{pr}: {reason}");
                    } else if v.merged {
                        effective = "accepted".into();
                        by = format!("merge of PR #{pr}");
                    }
                }
            }
            Dr { file, text, d, effective, by }
        })
        .collect()
}

/// Writes effective accept/reject outcomes into the DR files; returns the changed ids.
pub fn normalize(repo: &Path, drs: &[Dr], today: &str) -> Vec<String> {
    let mut changed = Vec::new();
    for dr in drs.iter().filter(|d| d.effective != d.d.status) {
        let mut done = false;
        let text: String = dr
            .text
            .lines()
            .map(|l| {
                if !done && l.trim_start().trim_start_matches(['-', '*']).trim_start().to_lowercase().starts_with("status:") {
                    done = true;
                    format!("- status: {}\n- decided: {today} ({})", dr.effective, dr.by)
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if done && std::fs::write(repo.join(&dr.file), text + "\n").is_ok() {
            changed.push(dr.d.id.clone());
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{self, FakeForge};

    fn write(repo: &Path, rel: &str, text: &str) {
        let p = repo.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn dr(id: &str) -> String {
        format!("# {id}: q\n- week: 2026-w45\n- status: proposed\n- decision: experiment\n\n## Options\n### Do nothing\n### B\n\n## Delivery\n- none\n")
    }

    #[test]
    fn merge_accepts_and_reject_comments_reject() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        write(r, "discovery/decisions/DR-0001-a.md", &dr("DR-0001"));
        write(r, "discovery/decisions/DR-0002-b.md", &dr("DR-0002"));
        RunRecord { week: "2026-w45".into(), pr: Some(5), ..Default::default() }.save(r).unwrap();
        let forge = FakeForge::new("o/r").with_pr(fake::pr(5, "Discovery: week 2026-w45", "discovery/2026-w45", &["discovery"]));
        forge.add_comment(5, "/reject DR-0002 evidence is a single forum post\nreview-minutes: 12");
        forge.add_comment(5, "<!-- discovery:pr --> /reject DR-0001 (agent text is ignored)");
        let before = effective(r, &forge);
        assert_eq!(before.iter().map(|d| d.effective.as_str()).collect::<Vec<_>>(), vec!["proposed", "rejected"], "not merged yet: only the explicit rejection counts");
        forge.merge_pr(5).unwrap();
        let after = effective(r, &forge);
        assert_eq!(after[0].effective, "accepted");
        assert_eq!(after[0].by, "merge of PR #5");
        assert!(after[1].by.contains("single forum post"));
        assert_eq!(pr_verdicts(&forge, 5).unwrap().review_minutes, 12);
        assert_eq!(normalize(r, &after, "2026-11-10"), vec!["DR-0001", "DR-0002"]);
        let text = std::fs::read_to_string(r.join("discovery/decisions/DR-0001-a.md")).unwrap();
        assert!(text.contains("- status: accepted\n- decided: 2026-11-10 (merge of PR #5)"));
        assert!(normalize(r, &effective(r, &forge), "x").is_empty(), "idempotent");
    }
}
