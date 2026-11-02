//! The product-discovery role's files (state/discovery/): state, cost ledger, weekly report,
//! local page previews and the private outreach drafts — read in the fast refresh.

use std::path::{Path, PathBuf};
use team_core::config::TeamConfig;
use team_core::state::{read_ledger, DiscoveryState, LedgerRecord, DISCOVERY_REPORT_FILE};

/// Weeks shown in the cost history.
pub const HISTORY_WEEKS: usize = 8;
/// report.md is small; the cap only guards the UI against a runaway file.
const REPORT_MAX_BYTES: usize = 200_000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveryView {
    pub state: Option<DiscoveryState>,
    pub ledger: Vec<LedgerRecord>,
    pub report: Option<String>,
    pub previews_dir: PathBuf,
    /// (slug, index.html)
    pub previews: Vec<(String, PathBuf)>,
    /// outreach-<week>.md of the current week, if written
    pub outreach: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeekCost {
    pub week: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub stages: usize,
}

impl DiscoveryView {
    /// Nothing written yet — the role never ran on this machine.
    pub fn is_empty(&self) -> bool {
        self.state.is_none() && self.ledger.is_empty() && self.report.is_none()
    }

    /// Ledger lines of the current week, in run order.
    pub fn week_stages(&self) -> Vec<&LedgerRecord> {
        let week = self.state.as_ref().map(|s| s.week.as_str()).unwrap_or_default();
        self.ledger.iter().filter(|r| !week.is_empty() && r.week == week).collect()
    }

    /// Every line is subscription usage: the cost is what the API would have charged.
    pub fn all_notional(&self) -> bool {
        self.ledger.iter().all(|r| r.billing == "subscription-notional")
    }
}

/// Same rule as the agent: `product_discovery.private_dir`, else `<state>/discovery/private`.
pub fn private_dir(state_dir: &Path, cfg: Option<&TeamConfig>) -> PathBuf {
    match cfg.map(|c| c.product_discovery.private_dir.trim()).filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => DiscoveryState::dir(state_dir).join("private"),
    }
}

pub fn load(state_dir: &Path, cfg: Option<&TeamConfig>) -> DiscoveryView {
    let dir = DiscoveryState::dir(state_dir);
    let state = DiscoveryState::load(state_dir);
    let report = std::fs::read(dir.join(DISCOVERY_REPORT_FILE)).ok().map(|mut b| {
        b.truncate(REPORT_MAX_BYTES);
        String::from_utf8_lossy(&b).into_owned()
    });
    let previews_dir = dir.join("previews");
    let mut previews: Vec<(String, PathBuf)> = std::fs::read_dir(&previews_dir)
        .map(|it| it.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().join("index.html"))).filter(|(_, p)| p.is_file()).collect())
        .unwrap_or_default();
    previews.sort();
    let outreach = state.as_ref().filter(|s| !s.week.is_empty()).map(|s| private_dir(state_dir, cfg).join(format!("outreach-{}.md", s.week))).filter(|p| p.is_file());
    DiscoveryView { state, ledger: read_ledger(state_dir), report, previews_dir, previews, outreach }
}

/// Totals of the last `n` weeks that have ledger lines, newest first. Week keys
/// ("2027-w03") sort chronologically as text.
pub fn weekly_costs(ledger: &[LedgerRecord], n: usize) -> Vec<WeekCost> {
    let mut weeks: std::collections::BTreeMap<&str, WeekCost> = std::collections::BTreeMap::new();
    for r in ledger.iter().filter(|r| !r.week.is_empty()) {
        let w = weeks.entry(&r.week).or_insert_with(|| WeekCost { week: r.week.clone(), cost_usd: 0.0, tokens: 0, stages: 0 });
        w.cost_usd += r.cost_usd;
        w.tokens += tokens(r);
        w.stages += 1;
    }
    weeks.into_values().rev().take(n).collect()
}

/// Same sum as the agent's week_tokens (cache tokens included).
pub fn tokens(r: &LedgerRecord) -> u64 {
    r.input_tokens + r.output_tokens + r.cache_tokens
}

/// 1234567 → "1,2 Mio.", 45300 → "45 Tsd."
pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{} Tsd.", n / 1000),
        _ => format!("{} Mio.", format!("{:.1}", n as f64 / 1e6).replace('.', ",")),
    }
}

pub fn fmt_duration(sec: u64) -> String {
    if sec < 60 { format!("{sec} s") } else { format!("{} Min. {:02} s", sec / 60, sec % 60) }
}

/// German name of `last_result`.
pub fn result_label(result: &str) -> &str {
    match result {
        "running" => "läuft",
        "ok" => "abgeschlossen",
        "error" => "Fehler",
        "budget" => "Wochenbudget erreicht",
        "paused" => "pausiert",
        "skipped" => "übersprungen",
        "usage-limit" => "Claude-Nutzungslimit",
        "" => "—",
        other => other,
    }
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
