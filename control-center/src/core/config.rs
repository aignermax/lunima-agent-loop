//! agent-loop.json as an editable document. Kept as a JSON object (not a struct) so
//! fields this app doesn't know survive a save untouched, in their original order.
//! Read as leniently as the C# loop reads it (BOM, comments, key casing, defaults).

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// How a field is edited in the settings form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldKind {
    Text,
    Number,
    Toggle,
    Choice(&'static [&'static str]),
}

/// Settings-form metadata for one config key.
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: FieldKind,
    pub group: &'static str,
}

const fn f(group: &'static str, key: &'static str, label: &'static str, kind: FieldKind, help: &'static str) -> FieldSpec {
    FieldSpec { key, label, help, kind, group }
}

/// Every field the settings page offers, grouped for display.
pub const FIELDS: &[FieldSpec] = &[
    f("Betrieb", "enabled", "Loop aktiviert", FieldKind::Toggle, "Aus = der Zeitplan läuft weiter, tut aber nichts."),
    f("Betrieb", "workersEnabled", "Loop-Worker aktiv", FieldKind::Toggle, "Aus = Team-Modus: der PO plant und merged, das Coden macht der Issue-Agent."),
    f("Betrieb", "maxTasksPerDay", "Max. Tasks pro Tag", FieldKind::Number, "Budget-Grenze für Worker-Läufe pro Kalendertag."),
    f("Betrieb", "ownerIntervalMinutes", "PO-Intervall (Min.)", FieldKind::Number, "Mindestabstand zwischen zwei Product-Owner-Läufen."),
    f("Modelle", "ownerRunner", "PO-Laufzeit", FieldKind::Choice(&["claude", "kimi"]), "CLI für den Product-Owner-Lauf."),
    f("Modelle", "ownerModel", "PO-Modell", FieldKind::Text, "z. B. claude-opus-5-5"),
    f("Modelle", "workerModel", "Worker-Modell", FieldKind::Text, "Modell der Worker (Kimi CLI), z. B. openrouter/moonshotai/kimi-k3"),
    f("Modelle", "ownerTimeoutMinutes", "PO-Timeout (Min.)", FieldKind::Number, "Harte Obergrenze für einen PO-Lauf."),
    f("Modelle", "workerTimeoutMinutes", "Worker-Timeout (Min.)", FieldKind::Number, "Harte Obergrenze für einen Worker-Lauf."),
    f("Repository", "githubRepo", "GitHub-Repo", FieldKind::Text, "owner/name"),
    f("Repository", "clonePath", "Lokaler Clone", FieldKind::Text, "Arbeitsverzeichnis von PO und Workern."),
    f("Repository", "integrationBranch", "Integrations-Branch", FieldKind::Text, "Ziel aller Agent-PRs (z. B. dev)."),
    f("Repository", "baseBranch", "Release-Branch", FieldKind::Text, "Wird vom Loop nie angefasst (z. B. main)."),
    f("Labels", "taskLabel", "Task-Label", FieldKind::Text, "Issues mit diesem Label bearbeiten die Worker."),
    f("Labels", "prLabel", "PR-Label", FieldKind::Text, "Markiert PRs der Agenten."),
    f("Labels", "blockedLabel", "Blockiert-Label", FieldKind::Text, "Braucht einen Menschen."),
    f("Labels", "runningLabel", "In-Arbeit-Label", FieldKind::Text, "Claim, damit kein zweiter Rechner dasselbe Issue nimmt."),
    f("Kunden-Review", "customerEnabled", "Kunden-Review aktiv", FieldKind::Toggle, "Simulierter Desktop-Kunde testet PRs (braucht Repo-Checkout)."),
    f("Kunden-Review", "customerModel", "Kunden-Modell", FieldKind::Text, ""),
    f("Kunden-Review", "customerMaxReviewsPerCycle", "Reviews pro Zyklus", FieldKind::Number, "1–10"),
];

/// Defaults of the C# `LoopConfig` — a missing key means this value to the loop.
const DEFAULTS: &[(&str, &str)] = &[
    ("githubRepo", "\"aignermax/Lunima\""),
    ("integrationBranch", "\"dev\""),
    ("baseBranch", "\"main\""),
    ("maxTasksPerDay", "2"),
    ("ownerIntervalMinutes", "60"),
    ("workerModel", "\"moonshot-ai/kimi-k2.7-code\""),
    ("ownerModel", "\"moonshot-ai/kimi-k3\""),
    ("ownerRunner", "\"kimi\""),
    ("workerTimeoutMinutes", "120"),
    ("ownerTimeoutMinutes", "60"),
    ("taskLabel", "\"agent-task\""),
    ("prLabel", "\"agent-pr\""),
    ("blockedLabel", "\"needs-human\""),
    ("runningLabel", "\"agent-running\""),
    ("enabled", "true"),
    ("workersEnabled", "true"),
    ("customerEnabled", "false"),
    ("customerPython", "\"python\""),
    ("customerModel", "\"claude-fable-5-1\""),
    ("customerMaxReviewsPerCycle", "2"),
    ("customerMaxAgeHours", "24"),
    ("customerMaxSteps", "80"),
    ("customerMaxTurns", "60"),
    ("customerTimeoutMinutes", "20"),
];

/// The config document plus where it lives.
#[derive(Clone, Debug)]
pub struct ConfigDoc {
    pub path: PathBuf,
    pub map: Map<String, Value>,
    /// The file had // or /* */ comments (accepted by the loop, lost on save).
    pub had_comments: bool,
}

/// Removes // and /* */ comments outside JSON strings (the loop accepts them).
fn strip_comments(text: &str) -> (String, bool) {
    let (mut out, mut found) = (String::with_capacity(text.len()), false);
    let mut chars = text.chars().peekable();
    let (mut in_str, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                found = true;
                while chars.peek().is_some_and(|&n| n != '\n') {
                    chars.next();
                }
            }
            ('/', Some('*')) => {
                found = true;
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    (out, found)
}

impl ConfigDoc {
    /// Reads the config as leniently as the loop does: BOM, comments, any key casing.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (clean, had_comments) = strip_comments(text.trim_start_matches('\u{feff}'));
        match serde_json::from_str::<Value>(&clean) {
            Ok(Value::Object(map)) => Ok(Self { path: path.to_path_buf(), map, had_comments }),
            Ok(_) => Err(format!("{}: kein JSON-Objekt", path.display())),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the document back (pretty, original key order) via a temp file.
    pub fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&Value::Object(self.map.clone())).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text + "\n").map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// The key as stored in the file (case-insensitive match, like the loop).
    fn stored_key(&self, key: &str) -> Option<&String> {
        self.map.keys().find(|k| k.eq_ignore_ascii_case(key))
    }

    /// Raw value, falling back to the loop's default for missing keys.
    fn value(&self, key: &str) -> Option<Value> {
        if let Some(v) = self.stored_key(key).and_then(|k| self.map.get(k)) {
            return Some(v.clone());
        }
        DEFAULTS.iter().find(|(k, _)| *k == key).and_then(|(_, d)| serde_json::from_str(d).ok())
    }

    pub fn str(&self, key: &str) -> String {
        match self.value(key) {
            Some(Value::String(s)) => s,
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    }

    pub fn bool(&self, key: &str) -> bool {
        self.value(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.value(key).and_then(|v| v.as_i64())
    }

    /// Sets a field from form text; numbers must parse, toggles take "true"/"false".
    /// An existing key keeps its spelling in the file.
    pub fn set_from_text(&mut self, spec: &FieldSpec, text: &str) -> Result<(), String> {
        let value = match spec.kind {
            FieldKind::Number => Value::from(text.trim().parse::<i64>().map_err(|_| format!("{}: keine Zahl", spec.label))?),
            FieldKind::Toggle => Value::Bool(text == "true"),
            FieldKind::Text | FieldKind::Choice(_) => Value::String(text.to_string()),
        };
        let key = self.stored_key(spec.key).cloned().unwrap_or_else(|| spec.key.to_string());
        self.map.insert(key, value);
        Ok(())
    }

    /// The checks `LoopConfig.Load` enforces — a config failing them stops every run.
    pub fn validate(&self) -> Result<(), String> {
        if self.str("clonePath").trim().is_empty() {
            return Err("Lokaler Clone darf nicht leer sein.".into());
        }
        if !self.bool("customerEnabled") {
            return Ok(());
        }
        let ranges = [
            ("customerMaxReviewsPerCycle", 10),
            ("customerMaxAgeHours", 168),
            ("customerMaxSteps", 500),
            ("customerMaxTurns", 500),
            ("customerTimeoutMinutes", 120),
        ];
        for (key, max) in ranges {
            if !(1..=max).contains(&self.int(key).unwrap_or(0)) {
                return Err(format!("{key} muss bei aktivem Kunden-Review zwischen 1 und {max} liegen."));
            }
        }
        if self.str("customerPython").trim().is_empty() || self.str("customerModel").trim().is_empty() {
            return Err("Kunden-Review braucht customerPython und customerModel.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
