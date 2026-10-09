//! Turns a Product-Owner log (Claude Code stream-json, one event per line) into a readable
//! pass summary: what it did on GitHub (issues, merges, labels, comments) and how it ended.

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    IssueCreated,
    IssueClosed,
    PrCreated,
    PrMerged,
    PrRetargeted,
    LabelAdded,
    Comment,
}

impl ActionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::IssueCreated => "Issue erstellt",
            Self::IssueClosed => "Issue geschlossen",
            Self::PrCreated => "PR erstellt",
            Self::PrMerged => "PR gemergt",
            Self::PrRetargeted => "PR umgebogen",
            Self::LabelAdded => "Label gesetzt",
            Self::Comment => "Kommentar",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Action {
    pub kind: ActionKind,
    pub text: String,
    pub url: Option<String>,
    pub failed: bool,
}

/// One PO pass as reconstructed from its log.
#[derive(Debug, Clone, Default)]
pub struct Pass {
    pub actions: Vec<Action>,
    /// Final message of the session (`result` event), if it finished.
    pub summary: Option<String>,
    pub is_error: bool,
    pub finished: bool,
    pub duration_min: Option<f64>,
    pub turns: Option<u64>,
    pub cost_usd: Option<f64>,
    pub tool_errors: usize,
    pub model: Option<String>,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

fn url_in(text: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"https://github\.com/[\w.-]+/[\w.-]+/(?:issues|pull)/\d+").find(text).map(|m| m.as_str().to_string())
}

fn title_in(cmd: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r#"--title\s+(?:"([^"]*)"|'([^']*)')"#)
        .captures(cmd)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().to_string())
}

fn number_after(cmd: &str, verb: &str) -> Option<String> {
    let pattern = format!(r"{}\s+#?(\d+)", regex::escape(verb));
    Regex::new(&pattern).ok()?.captures(cmd).map(|c| format!("#{}", &c[1]))
}

fn label_call(cmd: &str) -> Option<(String, String)> {
    static R: OnceLock<Regex> = OnceLock::new();
    static L: OnceLock<Regex> = OnceLock::new();
    let n = re(&R, r"issues/(\d+)/labels").captures(cmd)?;
    let label = re(&L, r#"labels\[\]=([^"'\s]+)"#).captures(cmd).map(|c| c[1].to_string()).unwrap_or_default();
    Some((format!("#{}", &n[1]), label))
}

/// Classifies one shell command the PO ran. Several gh calls may share a line;
/// the first recognised one wins (good enough for a timeline).
fn classify(cmd: &str) -> Option<(ActionKind, String)> {
    let c = cmd.trim();
    if c.contains("gh issue create") {
        return Some((ActionKind::IssueCreated, title_in(c).unwrap_or_else(|| "Neues Issue".into())));
    }
    if c.contains("gh pr create") {
        return Some((ActionKind::PrCreated, title_in(c).unwrap_or_else(|| "Neuer PR".into())));
    }
    if c.contains("gh pr merge") {
        return Some((ActionKind::PrMerged, number_after(c, "gh pr merge").unwrap_or_default()));
    }
    if c.contains("gh issue close") {
        return Some((ActionKind::IssueClosed, number_after(c, "gh issue close").unwrap_or_default()));
    }
    if c.contains("gh pr edit") && c.contains("--base") {
        return Some((ActionKind::PrRetargeted, number_after(c, "gh pr edit").unwrap_or_default()));
    }
    if c.contains("/labels") && (c.contains("-X POST") || c.contains("--method POST")) {
        let (n, label) = label_call(c)?;
        return Some((ActionKind::LabelAdded, format!("{n} → {label}")));
    }
    for verb in ["gh issue comment", "gh pr comment"] {
        if c.contains(verb) {
            return Some((ActionKind::Comment, number_after(c, verb).unwrap_or_default()));
        }
    }
    None
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Parses a whole log. Unparseable lines (e.g. `[stderr] ...`) are ignored.
pub fn parse_pass(log: &str) -> Pass {
    let mut pass = Pass::default();
    let mut pending: HashMap<String, usize> = HashMap::new();
    for line in log.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
        match event.get("type").and_then(Value::as_str) {
            Some("system") if pass.model.is_none() => {
                pass.model = event.get("model").and_then(Value::as_str).map(String::from);
            }
            Some("assistant") => collect_tool_uses(&event, &mut pass, &mut pending),
            Some("user") => apply_tool_results(&event, &mut pass, &pending),
            Some("result") => apply_result(&event, &mut pass),
            _ => {}
        }
    }
    pass
}

fn content_items(event: &Value) -> &[Value] {
    event.pointer("/message/content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn collect_tool_uses(event: &Value, pass: &mut Pass, pending: &mut HashMap<String, usize>) {
    for item in content_items(event) {
        if item.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let Some(cmd) = item.pointer("/input/command").and_then(Value::as_str) else { continue };
        let Some((kind, text)) = classify(cmd) else { continue };
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            pending.insert(id.to_string(), pass.actions.len());
        }
        pass.actions.push(Action { kind, text, url: None, failed: false });
    }
}

fn apply_tool_results(event: &Value, pass: &mut Pass, pending: &HashMap<String, usize>) {
    for item in content_items(event) {
        if item.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let failed = item.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        if failed {
            pass.tool_errors += 1;
        }
        let Some(&idx) = item.get("tool_use_id").and_then(Value::as_str).and_then(|id| pending.get(id)) else { continue };
        let text = tool_result_text(item.get("content").unwrap_or(&Value::Null));
        let action = &mut pass.actions[idx];
        action.failed = failed;
        action.url = url_in(&text);
    }
}

fn apply_result(event: &Value, pass: &mut Pass) {
    pass.finished = true;
    pass.is_error = event.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    pass.summary = event.get("result").and_then(Value::as_str).map(str::trim).map(String::from);
    pass.duration_min = event.get("duration_ms").and_then(Value::as_f64).map(|ms| ms / 60_000.0);
    pass.turns = event.get("num_turns").and_then(Value::as_u64);
    pass.cost_usd = event.get("total_cost_usd").and_then(Value::as_f64);
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
