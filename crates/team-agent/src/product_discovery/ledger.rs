//! Cost accounting for discovery: one ledger line per Claude stage, the weekly spend and
//! the budget check before each stage.
//!
//! The check runs *between* stages, so it is not a hard cap: a stage that starts below the
//! limit can end above it. The overshoot is bounded by one stage (its `max_turns` and
//! `timeout_minutes`), and the estimate uses the stage's own history, so a stage that is
//! likely to cross the limit is not started at all.

use chrono::{DateTime, Local};
use team_core::claude::Outcome;
use team_core::state::{append_ledger, read_ledger, LedgerRecord};

/// Ledger line for one finished (or failed) stage session.
pub fn record(week: &str, stage: &str, machine: &str, model: &str, started: DateTime<Local>, outcome: Option<&Outcome>, result: &str) -> LedgerRecord {
    let o = outcome.cloned().unwrap_or_default();
    LedgerRecord {
        week: week.into(),
        stage: stage.into(),
        machine: machine.into(),
        model: model.into(),
        started: started.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        duration_sec: (Local::now() - started).num_seconds().max(0) as u64,
        input_tokens: o.usage.input,
        output_tokens: o.usage.output,
        cache_tokens: o.usage.cache_read + o.usage.cache_creation,
        turns: o.num_turns,
        cost_usd: o.cost_usd,
        // sessions run on the subscription login (ChildEnv::subscription): no per-token charge
        billing: "subscription-notional".into(),
        result: result.into(),
    }
}

pub fn append(state_dir: &std::path::Path, r: &LedgerRecord) {
    if let Err(e) = append_ledger(state_dir, r) {
        crate::log::warn(&format!("Discovery-Kostenbuch nicht geschrieben: {e}"));
    }
}

pub fn week_cost(records: &[LedgerRecord], week: &str) -> f64 {
    records.iter().filter(|r| r.week == week).map(|r| r.cost_usd).sum()
}

pub fn week_tokens(records: &[LedgerRecord], week: &str) -> u64 {
    records.iter().filter(|r| r.week == week).map(|r| r.input_tokens + r.output_tokens + r.cache_tokens).sum()
}

/// Expected cost of `stage`: the highest of its last four runs (repairs included), else the
/// configured default. The maximum rather than the mean keeps the overshoot rare.
pub fn estimate(records: &[LedgerRecord], stage: &str, default: f64) -> f64 {
    let recent: Vec<f64> = records.iter().rev().filter(|r| r.stage == stage || r.stage == format!("{stage}-repair")).take(4).map(|r| r.cost_usd).collect();
    recent.into_iter().fold(None, |m: Option<f64>, c| Some(m.map_or(c, |m| m.max(c)))).unwrap_or(default)
}

/// `Err(reason)` when starting `stage` would probably exceed the weekly budget.
pub fn check_budget(state_dir: &std::path::Path, week: &str, stage: &str, budget: f64, default_estimate: f64) -> Result<(), String> {
    let records = read_ledger(state_dir);
    let spent = week_cost(&records, week);
    let next = estimate(&records, stage, default_estimate);
    if spent + next > budget {
        return Err(format!("Wochenbudget erreicht: {spent:.2} $ verbraucht, {stage} kostet erfahrungsgemäß bis {next:.2} $, Grenze {budget:.2} $ (nominell, Abo)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(week: &str, stage: &str, cost: f64) -> LedgerRecord {
        LedgerRecord { week: week.into(), stage: stage.into(), cost_usd: cost, input_tokens: 10, output_tokens: 5, ..Default::default() }
    }

    #[test]
    fn weekly_sums_and_estimates() {
        let l = vec![r("w1", "scout", 3.0), r("w1", "critic", 1.0), r("w2", "scout", 5.0), r("w2", "scout-repair", 6.5)];
        assert_eq!(week_cost(&l, "w1"), 4.0);
        assert_eq!(week_tokens(&l, "w2"), 30);
        assert_eq!(estimate(&l, "scout", 2.0), 6.5);
        assert_eq!(estimate(&l, "strategist", 2.0), 2.0);
    }

    #[test]
    fn budget_check_stops_before_the_stage_that_would_cross_it() {
        let t = tempfile::tempdir().unwrap();
        assert!(check_budget(t.path(), "w1", "scout", 10.0, 4.0).is_ok());
        append(t.path(), &r("w1", "scout", 7.0));
        let err = check_budget(t.path(), "w1", "critic", 10.0, 4.0).unwrap_err();
        assert!(err.contains("7.00"), "{err}");
        assert!(check_budget(t.path(), "w2", "critic", 10.0, 4.0).is_ok(), "a new week has a new budget");
        assert!(check_budget(t.path(), "w1", "critic", 0.0, 0.0).is_err(), "budget 0 = discovery off");
    }
}
