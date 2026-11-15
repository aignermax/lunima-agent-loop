//! The input folder of a run (`.discovery/context/` inside the clone, never committed):
//! everything the stages may know beyond the repository and the public web, written as
//! plain Markdown so a sandboxed session (file tools only) can read it.

use super::actions::{item_key, LABEL_APPROVAL, LABEL_APPROVED, LABEL_DECLINED, LABEL_EXPERIMENT, LABEL_INTERVIEW, LABEL_OPPORTUNITY};
use super::analytics::{self, Metrics};
use super::decisions::Dr;
use super::model::{parse_evidence, ROOT};
use super::run_record::RunRecord;
use crate::forge::Forge;
use std::path::{Path, PathBuf};
use team_core::gh::Issue;

pub const WORK_DIR: &str = ".discovery";

pub fn context_dir(repo: &Path) -> PathBuf {
    repo.join(WORK_DIR).join("context")
}

pub fn out_dir(repo: &Path) -> PathBuf {
    repo.join(WORK_DIR).join("out")
}

/// Labels of Lunima issues worth the scout's attention (user-reported problems first).
const SIGNAL_LABELS: &[&str] = &["bug", "ux-finding", "enhancement", "question"];

fn short(s: &str, n: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n { flat } else { format!("{}…", flat.chars().take(n).collect::<String>()) }
}

fn issue_line(i: &Issue) -> String {
    let labels: Vec<String> = i.labels.iter().map(|l| l.name.clone()).collect();
    format!("- #{} [{}] {} ({}) — {}\n  {}\n", i.number, i.state, i.title, labels.join(", "), i.html_url, short(i.body.as_deref().unwrap_or(""), 300))
}

/// Human comments on a discovery issue (the maintainer's answers), newest last.
fn human_comments(forge: &dyn Forge, n: u64) -> Vec<String> {
    forge.comments(n).unwrap_or_default().into_iter().filter(|c| !c.body.contains("<!-- discovery:") && !c.body.starts_with("Parked by the weekly product discovery")).map(|c| short(&c.body, 400)).collect()
}

pub struct Inputs<'a> {
    pub week: &'a str,
    pub today: &'a str,
    pub repo_name: &'a str,
    pub budget_left_usd: f64,
    pub caps: String,
    pub drs: &'a [Dr],
    pub metrics: &'a Metrics,
    /// last week's run record (critic summary, notes)
    pub previous: Option<&'a RunRecord>,
    /// local notes from `product_discovery.context_files`
    pub extra: &'a [String],
}

/// Writes the context folder; returns the file names written.
pub fn write(repo: &Path, forge: &dyn Forge, inp: &Inputs) -> Result<Vec<String>, String> {
    let dir = context_dir(repo);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out_dir(repo)).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut put = |name: &str, text: String| -> Result<(), String> {
        std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))?;
        files.push(name.to_string());
        Ok(())
    };

    put(
        "brief.md",
        format!(
            "# Discovery brief — week {week} ({today})\n\nRepository: {repo}. Remaining notional budget this week: {left:.2} USD.\n\n## Outcome for Q1 2027\nIdentify and externally validate at least one genuinely valuable photonic engineering workflow for Lunima.\n- 3–5 conversations with real PIC designers (only the maintainer talks to people)\n- at least two different workflows compared\n- at least one practical external validation of an actual Lunima workflow\n- clear evidence supporting or weakening the leading strategic hypotheses\nThese are targets, never reasons to invent evidence. \"Do nothing\" and \"not enough evidence\" are valid results.\n\n## Caps\n{caps}\n",
            week = inp.week,
            today = inp.today,
            repo = inp.repo_name,
            left = inp.budget_left_usd,
            caps = inp.caps
        ),
    )?;

    let mut signals = String::from("# Lunima issues (user-reported problems and requests)\n\nExternal reporters are evidence of class `user-feedback` (source: the issue URL) only when the author is not the maintainer or an agent; agent-filed findings (`ux-finding`, simulated users) are class `simulated`.\n\n");
    for label in SIGNAL_LABELS {
        let list = forge.issues(label, "all").unwrap_or_default();
        signals.push_str(&format!("## {label} ({} total, newest 25)\n", list.len()));
        for i in list.iter().take(25) {
            signals.push_str(&issue_line(i));
        }
        signals.push('\n');
    }
    put("lunima-issues.md", signals)?;

    let mut disc = String::from("# Discovery issues and the maintainer's answers\n\nApproval labels: `approved` / `declined` are set only by the maintainer. No label = no decision = not approved.\n\n");
    for label in [LABEL_OPPORTUNITY, LABEL_EXPERIMENT, LABEL_INTERVIEW, LABEL_APPROVAL] {
        disc.push_str(&format!("## {label}\n"));
        for i in forge.issues(label, "all").unwrap_or_default().iter().take(40) {
            let decision = if i.has_label(LABEL_APPROVED) {
                " **APPROVED by maintainer**"
            } else if i.has_label(LABEL_DECLINED) {
                " **DECLINED by maintainer**"
            } else {
                ""
            };
            disc.push_str(&format!("- #{} [{}]{decision} key={} — {}\n", i.number, i.state, i.body.as_deref().and_then(item_key).unwrap_or_default(), i.title));
            for c in human_comments(forge, i.number).iter().rev().take(3) {
                disc.push_str(&format!("  - maintainer/human: {c}\n"));
            }
        }
        disc.push('\n');
    }
    put("discovery-issues.md", disc)?;

    let mut dec = String::from("# Decision Records (effective status)\n\n");
    for d in inp.drs {
        dec.push_str(&format!("- {} [{}{}] decision={} — {} ({})\n", d.d.id, d.effective, if d.by.is_empty() { String::new() } else { format!(", {}", d.by) }, d.d.decision, d.d.title, d.file));
    }
    if inp.drs.is_empty() {
        dec.push_str("(none yet)\n");
    }
    put("decisions.md", dec)?;

    let mut index = String::from("# Evidence collected so far (do not collect these again)\n\n");
    let ev_dir = repo.join(ROOT).join("evidence");
    let mut files_ev: Vec<PathBuf> = std::fs::read_dir(&ev_dir).map(|it| it.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| super::model::is_evidence_file(&n.to_string_lossy()))).collect()).unwrap_or_default();
    files_ev.sort();
    for f in files_ev {
        for e in parse_evidence(&std::fs::read_to_string(&f).unwrap_or_default()) {
            index.push_str(&format!("- {} E{} {} — {}\n", e.id, e.level.map(|l| l.to_string()).unwrap_or("?".into()), e.source, short(&e.quote, 100)));
        }
    }
    put("evidence-index.md", index)?;

    let ux = forge.issues("ux-report", "open").unwrap_or_default().into_iter().next().and_then(|i| i.body).map(|b| short(&b, 4000));
    put("ux-report.md", format!("# Latest simulated-user UX report (class `simulated`, never demand evidence)\n\n{}\n", ux.unwrap_or_else(|| "(none)".into())))?;
    put("metrics.md", analytics::markdown(inp.metrics))?;
    let prev = inp.previous.map(|p| format!("# Last week ({})\n\n## Summary\n{}\n\n## Critic\n{}\n\n## Notes\n{}\n", p.week, p.summary, p.critic_summary, p.notes.iter().map(|n| format!("- {n}\n")).collect::<String>())).unwrap_or_else(|| "# Last week\n\n(no previous run)\n".into());
    put("previous-week.md", prev)?;
    for (n, path) in inp.extra.iter().enumerate() {
        let name = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| format!("note-{n}.md"));
        match std::fs::read_to_string(path) {
            Ok(text) => put(&format!("internal-{name}"), format!("<!-- internal note supplied by the maintainer ({path}); an assessment, not evidence of demand -->\n{text}"))?,
            Err(e) => crate::log::warn(&format!("Discovery-Kontextdatei {path} nicht lesbar: {e}")),
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{self, FakeForge};

    #[test]
    fn context_lists_signals_answers_and_unavailable_metrics() {
        let t = tempfile::tempdir().unwrap();
        let forge = FakeForge::new("o/r")
            .with_issue(fake::issue(3, "Routing hangs on large designs", "steps…", &["bug"]))
            .with_issue(fake::issue(9, "Approval: publish page", &format!("{}\n", super::super::actions::marker("P:gds")), &[LABEL_APPROVAL, LABEL_DECLINED]));
        forge.add_comment(9, "Not now — first talk to two designers.");
        let m = Metrics { available: false, source: "none".into(), note: "No analytics configured.".into(), pages: vec![] };
        let files = write(t.path(), &forge, &Inputs { week: "2026-w45", today: "2026-11-02", repo_name: "o/r", budget_left_usd: 25.0, caps: "- max 3".into(), drs: &[], metrics: &m, previous: None, extra: &[t.path().join("missing.md").display().to_string()] }).unwrap();
        assert!(files.contains(&"brief.md".to_string()) && files.len() == 8);
        let read = |f: &str| std::fs::read_to_string(context_dir(t.path()).join(f)).unwrap();
        assert!(read("lunima-issues.md").contains("#3 [open] Routing hangs"));
        let d = read("discovery-issues.md");
        assert!(d.contains("DECLINED by maintainer") && d.contains("key=P:gds") && d.contains("first talk to two designers"));
        assert!(read("metrics.md").contains("UNAVAILABLE"));
        assert!(out_dir(t.path()).is_dir());
    }
}
