//! agent-loop.json as an editable document. Kept as a JSON object (not a struct) so
//! fields this app doesn't know survive a save untouched, in their original order.

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
    f("Kunden-Review", "customerMaxReviewsPerCycle", "Reviews pro Zyklus", FieldKind::Number, ""),
];

/// The config document plus where it lives.
#[derive(Clone, Debug)]
pub struct ConfigDoc {
    pub path: PathBuf,
    pub map: Map<String, Value>,
}

impl ConfigDoc {
    /// Reads and parses the config (comments are not supported by this editor).
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Ok(Self { path: path.to_path_buf(), map }),
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

    pub fn str(&self, key: &str) -> String {
        match self.map.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    }

    pub fn bool(&self, key: &str) -> bool {
        self.map.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.map.get(key).and_then(Value::as_i64)
    }

    /// Sets a field from form text; numbers must parse, toggles take "true"/"false".
    pub fn set_from_text(&mut self, spec: &FieldSpec, text: &str) -> Result<(), String> {
        let value = match spec.kind {
            FieldKind::Number => Value::from(text.trim().parse::<i64>().map_err(|_| format!("{}: keine Zahl", spec.label))?),
            FieldKind::Toggle => Value::Bool(text == "true"),
            FieldKind::Text | FieldKind::Choice(_) => Value::String(text.to_string()),
        };
        self.map.insert(spec.key.to_string(), value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(json: &str) -> (tempfile::TempDir, ConfigDoc) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent-loop.json");
        std::fs::write(&path, json).unwrap();
        let d = ConfigDoc::load(&path).unwrap();
        (tmp, d)
    }

    #[test]
    fn unknown_fields_and_order_survive_a_save() {
        let (_t, mut d) = doc(r#"{"zeta":1,"enabled":false,"custom":{"x":[1,2]}}"#);
        let spec = FIELDS.iter().find(|s| s.key == "enabled").unwrap();
        d.set_from_text(spec, "true").unwrap();
        d.save().unwrap();
        let text = std::fs::read_to_string(&d.path).unwrap();
        let keys: Vec<_> = ConfigDoc::load(&d.path).unwrap().map.keys().cloned().collect();
        assert_eq!(keys, ["zeta", "enabled", "custom"]);
        assert!(text.contains("\"enabled\": true"));
        assert!(text.contains("\"x\""));
    }

    #[test]
    fn number_fields_reject_garbage() {
        let (_t, mut d) = doc(r#"{"maxTasksPerDay":2}"#);
        let spec = FIELDS.iter().find(|s| s.key == "maxTasksPerDay").unwrap();
        assert!(d.set_from_text(spec, "zwölf").is_err());
        d.set_from_text(spec, " 12 ").unwrap();
        assert_eq!(d.int("maxTasksPerDay"), Some(12));
    }

    #[test]
    fn invalid_json_reports_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent-loop.json");
        std::fs::write(&path, "{ nope").unwrap();
        assert!(ConfigDoc::load(&path).is_err());
    }
}
