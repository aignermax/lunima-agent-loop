use super::*;

fn line(week: &str, stage: &str, cost: f64, tokens: u64) -> String {
    serde_json::json!({"week": week, "stage": stage, "model": "opus", "started": "2027-01-18T06:00:00+01:00", "duration_sec": 754, "input_tokens": tokens, "output_tokens": 0, "cache_tokens": 0, "turns": 12, "cost_usd": cost, "billing": "subscription-notional", "result": "ok"}).to_string()
}

fn put(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn missing_files_give_an_empty_view() {
    let t = tempfile::tempdir().unwrap();
    let v = load(t.path(), None);
    assert!(v.is_empty() && v.previews.is_empty() && v.outreach.is_none());
    assert!(v.week_stages().is_empty());
}

#[test]
fn loads_state_ledger_report_previews_and_outreach() {
    let t = tempfile::tempdir().unwrap();
    let state = r#"{"week": "2027-w03", "stage": "critic", "last_result": "running", "week_cost_usd": 7.5, "approvals": [{"id": "8", "title": "publish page", "status": "pending", "url": ""}]}"#;
    put(t.path(), "discovery/state.json", state);
    let ledger = [line("2027-w02", "scout", 3.0, 100), line("2027-w03", "scout", 4.0, 200), "kaputt".into(), line("2027-w03", "strategist", 3.5, 300)].join("\n");
    put(t.path(), "discovery/ledger.jsonl", &ledger);
    put(t.path(), "discovery/report.md", "# Discovery — week 2027-w03\n");
    put(t.path(), "discovery/previews/gds-reconstruction/index.html", "<html>");
    put(t.path(), "discovery/previews/no-page/notes.txt", "x");
    put(t.path(), "discovery/private/outreach-2027-w03.md", "PRIVATE");
    let v = load(t.path(), None);
    let st = v.state.as_ref().unwrap();
    assert_eq!((st.week.as_str(), st.approvals.len()), ("2027-w03", 1));
    assert_eq!(v.ledger.len(), 3, "a broken line is skipped");
    assert_eq!(v.week_stages().iter().map(|r| r.stage.as_str()).collect::<Vec<_>>(), ["scout", "strategist"]);
    assert!(v.report.as_deref().unwrap().contains("2027-w03"));
    assert_eq!(v.previews.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["gds-reconstruction"]);
    assert!(v.outreach.as_ref().unwrap().ends_with("outreach-2027-w03.md"));
    assert!(v.all_notional());
}

#[test]
fn outreach_follows_the_configured_private_dir() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), "discovery/state.json", r#"{"week": "2027-w03"}"#);
    put(t.path(), "elsewhere/outreach-2027-w03.md", "PRIVATE");
    let mut cfg = TeamConfig::default();
    assert!(load(t.path(), Some(&cfg)).outreach.is_none());
    cfg.product_discovery.private_dir = t.path().join("elsewhere").display().to_string();
    assert!(load(t.path(), Some(&cfg)).outreach.is_some());
}

#[test]
fn weekly_costs_newest_first_and_capped() {
    let ledger: Vec<LedgerRecord> = (1..=10).flat_map(|w| [line(&format!("2027-w{w:02}"), "scout", 1.0, 10), line(&format!("2027-w{w:02}"), "critic", 0.5, 5)]).map(|l| serde_json::from_str(&l).unwrap()).collect();
    let weeks = weekly_costs(&ledger, HISTORY_WEEKS);
    assert_eq!(weeks.len(), 8);
    assert_eq!(weeks[0], WeekCost { week: "2027-w10".into(), cost_usd: 1.5, tokens: 15, stages: 2 });
    assert_eq!(weeks[7].week, "2027-w03");
}

#[test]
fn formatting() {
    assert_eq!(fmt_tokens(950), "950");
    assert_eq!(fmt_tokens(45_300), "45 Tsd.");
    assert_eq!(fmt_tokens(1_234_567), "1,2 Mio.");
    assert_eq!(fmt_duration(754), "12 Min. 34 s");
    assert_eq!(result_label("usage-limit"), "Claude-Nutzungslimit");
    let mut v = DiscoveryView::default();
    v.ledger.push(LedgerRecord { billing: "api".into(), ..Default::default() });
    assert!(!v.all_notional());
}
