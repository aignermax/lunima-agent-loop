//! Interpreting Claude Code's stream-json output.

use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone};
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }
    fn add(&mut self, u: &Value) {
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        self.input += n("input_tokens");
        self.output += n("output_tokens");
        self.cache_read += n("cache_read_input_tokens");
        self.cache_creation += n("cache_creation_input_tokens");
    }
}

/// What a finished session produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// final message (`result` event)
    pub text: String,
    pub is_error: bool,
    /// `success`, `error_max_turns`, `error_during_execution`, …
    pub subtype: String,
    pub reached_max_turns: bool,
    pub num_turns: u32,
    pub usage: Usage,
    /// the CLI's own figure (informational under a subscription)
    pub cost_usd: f64,
    pub session_id: String,
    /// the subscription's usage limit hit; resume at this time
    pub usage_limit_until: Option<DateTime<Local>>,
    /// shell commands the session ran (tool usage stats)
    pub bash_commands: Vec<String>,
}

impl Outcome {
    /// Python-compatible cost estimate (tokens × 5.6e-7) for budgets and PR stats.
    pub fn estimated_cost(&self) -> f64 {
        self.usage.total() as f64 * 0.000_000_56
    }
}

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).expect("static regex"))
}

/// Recognises "usage limit reached" messages and returns when to try again:
/// `…limit reached|1760000000` (epoch), `… resets 3pm` / `resets 15:30`, else in one hour.
pub fn usage_limit_reset(text: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    static HIT: OnceLock<Regex> = OnceLock::new();
    static EPOCH: OnceLock<Regex> = OnceLock::new();
    static CLOCK: OnceLock<Regex> = OnceLock::new();
    if !re(&HIT, r"(?i)usage limit|limit reached|hit your limit|rate limit").is_match(text) {
        return None;
    }
    if let Some(c) = re(&EPOCH, r"\|(\d{10})\b").captures(text) {
        if let Some(t) = c[1].parse::<i64>().ok().and_then(|s| Local.timestamp_opt(s, 0).single()) {
            return Some(t);
        }
    }
    if let Some(c) = re(&CLOCK, r"(?i)resets?\s+(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?").captures(text) {
        let mut h: u32 = c[1].parse().unwrap_or(0);
        let m: u32 = c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        match c.get(3).map(|x| x.as_str().to_ascii_lowercase()) {
            Some(ref p) if p == "pm" && h < 12 => h += 12,
            Some(ref p) if p == "am" && h == 12 => h = 0,
            _ => {}
        }
        if let Some(t) = NaiveTime::from_hms_opt(h, m, 0) {
            let today = now.date_naive().and_time(t);
            if let Some(at) = Local.from_local_datetime(&today).single() {
                return Some(if at > now { at } else { at + Duration::days(1) });
            }
        }
    }
    Some(now + Duration::hours(1))
}

/// Folds the event lines into an outcome; `None` if no `result` event arrived.
pub fn parse_events(lines: &[String], max_turns: u32) -> Option<Outcome> {
    let mut out = Outcome::default();
    let mut seen_messages = HashSet::new();
    let mut result: Option<Value> = None;
    for line in lines {
        let Ok(ev) = serde_json::from_str::<Value>(line) else { continue };
        match ev.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let msg = &ev["message"];
                // one API message can be split over several events carrying the same usage
                let id = msg.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                if seen_messages.insert(id) {
                    if let Some(u) = msg.get("usage") {
                        out.usage.add(u);
                    }
                }
                for item in msg.get("content").and_then(Value::as_array).into_iter().flatten() {
                    if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                        if let Some(cmd) = item.pointer("/input/command").and_then(Value::as_str) {
                            out.bash_commands.push(cmd.to_string());
                        }
                    }
                }
            }
            Some("system") if out.session_id.is_empty() => {
                out.session_id = ev.get("session_id").and_then(Value::as_str).unwrap_or("").to_string();
            }
            Some("result") => result = Some(ev),
            _ => {}
        }
    }
    let r = result?;
    out.text = r.get("result").and_then(Value::as_str).unwrap_or("").to_string();
    out.is_error = r.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    out.subtype = r.get("subtype").and_then(Value::as_str).unwrap_or("").to_string();
    out.num_turns = r.get("num_turns").and_then(Value::as_u64).unwrap_or(0) as u32;
    out.cost_usd = r.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
    out.reached_max_turns = out.subtype == "error_max_turns" || (max_turns > 0 && out.num_turns >= max_turns);
    if out.usage.total() == 0 {
        if let Some(u) = r.get("usage") {
            out.usage.add(u);
        }
    }
    if out.is_error {
        out.usage_limit_until = usage_limit_reset(&out.text, Local::now());
    }
    Some(out)
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
