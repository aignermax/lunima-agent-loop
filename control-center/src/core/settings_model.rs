//! The settings form over team.toml: one table of fields addressed by dotted paths
//! (`budgets.max_turns_regular`). Values are edited as text and written back through
//! TeamConfig, so nothing unknown can end up in the file. Secrets never appear here: keys
//! live in the environment or key files the agent reads itself.

use std::collections::BTreeMap;
use team_core::config::TeamConfig;
use toml::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Text,
    Number,
    Toggle,
}

pub struct Field {
    pub group: &'static str,
    pub path: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

const fn f(group: &'static str, path: &'static str, label: &'static str, kind: Kind, help: &'static str) -> Field {
    Field { group, path, label, kind, help }
}

use Kind::{Number as N, Text as T, Toggle as B};

pub const FIELDS: &[Field] = &[
    f("Rollen", "roles.product_owner", "Product Owner", B, "Plant Issues und reviewt/merged PRs im PO-Repo."),
    f("Rollen", "roles.coder", "Coder", B, "Setzt agent-task-Issues als PRs um."),
    f("Rollen", "roles.qa", "QA-Tester", B, "Baut und testet Agent-PRs (.agent.toml)."),
    f("Rollen", "roles.pr_feedback", "PR-Feedback", B, "Setzt @agent-Kommentare auf PRs um."),
    f("Rollen", "roles.ux_review", "UX-Review", B, "Nachts: simulierte Nutzer bedienen die Desktop-App (entsperrter Bildschirm nötig)."),
    f("UX-Review", "ux_review.repo", "Repository", T, "Leer = Repo des Product Owners"),
    f("UX-Review", "ux_review.hour", "Startzeit (Stunde)", N, "Lokale Stunde; das Fenster dauert 4 Std."),
    f("UX-Review", "ux_review.goals_per_run", "Abläufe pro Nacht", N, "Rotiert durch alle Persona-Abläufe"),
    f("UX-Review", "ux_review.max_steps", "Max. UI-Aktionen", N, ""),
    f("UX-Review", "ux_review.model", "Modell", T, "Computer-Use über die Anthropic-API"),
    f("UX-Review", "ux_review.min_severity", "Issues ab Schwere", T, "minor | major | critical"),
    f("UX-Review", "ux_review.app_project", "App-Projekt", T, "z. B. CAP.Desktop"),
    f("UX-Review", "ux_review.tester_dir", "Tester-Ordner", T, "…\\lunima-agent-loop\\tools\\ux-tester"),
    f("UX-Review", "ux_review.python", "Python-Aufruf", T, "z. B. py -3"),
    f("UX-Review", "ux_review.api_key_file", "Schlüsseldatei (API)", T, "Pfad zur Datei — der Schlüssel selbst wird nie angezeigt"),
    f("UX-Review", "ux_review.goals_file", "Eigene Abläufe (JSON)", T, "Leer = eingebaute Persona-Abläufe"),
    f("UX-Review", "ux_review.timeout_minutes", "Zeitlimit (Min.)", N, ""),
    f("Product Owner", "product_owner.repo", "Repository", T, "owner/name"),
    f("Product Owner", "product_owner.integration_branch", "Integrations-Branch", T, "Ziel der Agent-PRs, z. B. dev"),
    f("Product Owner", "product_owner.base_branch", "Release-Branch", T, "z. B. main"),
    f("Product Owner", "product_owner.interval_minutes", "Intervall (Min.)", N, "Abstand zwischen zwei PO-Läufen"),
    f("Product Owner", "product_owner.timeout_minutes", "Zeitlimit (Min.)", N, "Ein PO-Lauf wird danach abgebrochen."),
    f("Product Owner", "product_owner.max_runs_per_day", "Max. Läufe pro Tag", N, ""),
    f("Product Owner", "product_owner.backlog_min", "Mindest-Backlog", N, "Darunter plant der PO neue Issues"),
    f("Product Owner", "product_owner.max_turns", "Max. Schritte pro Lauf", N, ""),
    f("Product Owner", "product_owner.customer_command", "Kunden-Review (Befehl)", T, "Leer = aus"),
    f("Product Owner", "product_owner.customer_max_age_hours", "Kunden-Review gültig (Std.)", N, ""),
    f("Modelle", "models.product_owner", "Product Owner", T, ""),
    f("Modelle", "models.coder", "Coder (Standard)", T, "Leer = Standard der Claude-CLI"),
    f("Modelle", "models.premium", "Coder (complex / claudeapi)", T, ""),
    f("Modelle", "models.reviewer", "Reviewer & QA-Review", T, ""),
    f("Modelle", "models.reviewer_critical", "Reviewer (Label critical)", T, ""),
    f("Modelle", "providers.complex_uses_premium", "complex → Premium-Modell", B, ""),
    f("Modelle", "providers.openrouter.model", "Kimi über OpenRouter", T, "Für Repos mit Modell „Kimi“ (Seite Projekte)"),
    f("Modelle", "providers.eco.model", "eco-Label", T, "Nur in für Kimi freigegebenen Repos"),
    f("Budgets", "budgets.max_turns_regular", "Max. Schritte (normal)", N, ""),
    f("Budgets", "budgets.max_turns_complex", "Max. Schritte (complex)", N, ""),
    f("Budgets", "budgets.max_sessions", "Sitzungen je Issue", N, "Fortsetzungen nach dem Schritt-Limit"),
    f("Budgets", "budgets.max_attempts", "Versuche je Issue", N, "Danach needs-human"),
    f("Budgets", "budgets.max_tokens_regular", "Token-Budget (normal)", N, ""),
    f("Budgets", "budgets.max_tokens_complex", "Token-Budget (complex)", N, ""),
    f("Budgets", "budgets.max_review_rounds_regular", "Review-Runden (normal)", N, ""),
    f("Budgets", "budgets.max_review_rounds_complex", "Review-Runden (complex)", N, ""),
    f("Budgets", "budgets.reviewer_max_turns", "Max. Schritte Reviewer", N, ""),
    f("Budgets", "budgets.max_qa_fix_rounds", "QA-Fix-Runden", N, "Danach Eskalation an needs-human"),
    f("Budgets", "budgets.pr_feedback_max_rounds", "PR-Feedback-Runden", N, ""),
    f("Budgets", "budgets.pr_feedback_max_turns", "Max. Schritte PR-Feedback", N, ""),
    f("Budgets", "budgets.inactivity_minutes", "Hänger-Erkennung (Min.)", N, "Claude ohne Ausgabe wird danach beendet"),
    f("Budgets", "budgets.test_gate", "Test-Gate vor dem Review", B, "test_cmd aus .agent.toml"),
    f("Labels", "labels.task", "Task-Label", T, ""),
    f("Labels", "labels.pr", "Label für Coder-PRs", T, "Leer = keines"),
    f("Labels", "labels.running", "In Arbeit", T, ""),
    f("Labels", "labels.needs_human", "Braucht Menschen", T, ""),
    f("Labels", "labels.complex", "Schwer", T, ""),
    f("Labels", "labels.critical", "Kritisch", T, ""),
    f("Labels", "labels.eco", "Günstig (eco)", T, ""),
    f("Labels", "labels.claudeapi", "Premium erzwingen", T, ""),
    f("Betrieb", "machine", "Name dieses Rechners", T, "Erscheint in Claim-Kommentaren"),
    f("Betrieb", "poll_seconds", "Abfrage-Intervall (s)", N, ""),
    f("Betrieb", "pr_feedback_marker", "PR-Feedback-Auslöser", T, "z. B. @agent"),
    f("Betrieb", "discovery.org", "Organisation für Auto-Discovery", T, "Leer = Discovery aus"),
    f("Betrieb", "discovery.interval_secs", "Discovery-Intervall (s)", N, ""),
    f("Betrieb", "discovery.expiry_days", "Entdeckte Repos verfallen nach (Tagen)", N, ""),
];

/// Read-only lines about where third-party keys come from (never the keys themselves).
pub fn key_sources(cfg: &TeamConfig) -> Vec<(String, String)> {
    let p = &cfg.providers;
    [("OpenRouter", &p.openrouter), ("eco", &p.eco)]
        .into_iter()
        .map(|(name, e)| {
            let mut src = Vec::new();
            if !e.key_env.is_empty() {
                src.push(format!("Umgebungsvariable {}", e.key_env));
            }
            if !e.key_file.is_empty() {
                src.push(format!("Datei {}", e.key_file));
            }
            let state = if e.api_key().is_some() { "vorhanden" } else { "fehlt" };
            (name.to_string(), format!("{} — {state}", if src.is_empty() { "nicht konfiguriert".into() } else { src.join(" oder ") }))
        })
        .collect()
}

fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |v, key| v.get(key))
}

/// The field exists in this version of team.toml (the form hides the others).
pub fn exists(cfg: &TeamConfig, path: &str) -> bool {
    Value::try_from(cfg).ok().is_some_and(|root| lookup(&root, path).is_some())
}

/// The field's current value as text.
pub fn get(cfg: &TeamConfig, path: &str) -> String {
    let Ok(root) = Value::try_from(cfg) else { return String::new() };
    match lookup(&root, path) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Integer(i)) => i.to_string(),
        Some(Value::Boolean(b)) => b.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Applies text edits (path → text) and validates the result.
pub fn apply(cfg: &TeamConfig, edits: &BTreeMap<&'static str, String>) -> Result<TeamConfig, String> {
    let mut root = Value::try_from(cfg).map_err(|e| e.to_string())?;
    for (path, text) in edits {
        let label = FIELDS.iter().find(|f| f.path == *path).map(|f| f.label).unwrap_or(path);
        let (parents, key) = path.rsplit_once('.').map(|(p, k)| (Some(p), k)).unwrap_or((None, path));
        let table = match parents {
            Some(p) => p.split('.').try_fold(&mut root, |v, k| v.get_mut(k)),
            None => Some(&mut root),
        }
        .and_then(Value::as_table_mut)
        .ok_or_else(|| format!("{label}: unbekannte Einstellung"))?;
        let old = table.get(key).ok_or_else(|| format!("{label}: unbekannte Einstellung"))?;
        let new = match old {
            Value::Integer(_) => Value::Integer(text.trim().parse::<i64>().ok().filter(|n| *n >= 0).ok_or_else(|| format!("{label}: „{text}“ ist keine Zahl"))?),
            Value::Boolean(_) => Value::Boolean(text.trim() == "true"),
            _ => Value::String(text.trim().to_string()),
        };
        table.insert(key.to_string(), new);
    }
    let updated: TeamConfig = root.try_into().map_err(|e: toml::de::Error| e.to_string())?;
    updated.validate()?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fields the PO port adds to `[product_owner]` (feat/po-role); shown once they exist.
    const PO_PORT: &[&str] = &["product_owner.max_runs_per_day", "product_owner.backlog_min", "product_owner.max_turns", "product_owner.customer_command", "product_owner.customer_max_age_hours"];

    #[test]
    fn every_field_exists_in_team_toml() {
        let cfg = TeamConfig::default();
        let root = Value::try_from(&cfg).unwrap();
        for f in FIELDS {
            let Some(v) = lookup(&root, f.path) else {
                assert!(PO_PORT.contains(&f.path), "{} fehlt in TeamConfig", f.path);
                continue;
            };
            let ok = match f.kind {
                Kind::Number => v.is_integer(),
                Kind::Toggle => v.is_bool(),
                Kind::Text => v.is_str(),
            };
            assert!(ok, "{} hat den falschen Typ", f.path);
        }
    }

    #[test]
    fn edits_round_trip_through_a_saved_file() {
        let cfg = TeamConfig::default();
        let mut edits = BTreeMap::new();
        edits.insert("budgets.max_turns_regular", "42".to_string());
        edits.insert("roles.product_owner", "true".to_string());
        edits.insert("product_owner.repo", " aignermax/Lunima ".to_string());
        edits.insert("machine", "BOX".to_string());
        let updated = apply(&cfg, &edits).unwrap();
        assert_eq!(updated.budgets.max_turns_regular, 42);
        assert!(updated.roles.product_owner);
        assert_eq!(get(&updated, "product_owner.repo"), "aignermax/Lunima");
        let t = tempfile::tempdir().unwrap();
        updated.save(&t.path().join("team.toml")).unwrap();
        assert_eq!(TeamConfig::load(&t.path().join("team.toml")).unwrap(), updated);
    }

    #[test]
    fn bad_numbers_and_invalid_configs_are_rejected() {
        let cfg = TeamConfig::default();
        let mut edits = BTreeMap::new();
        edits.insert("budgets.max_turns_regular", "viele".to_string());
        assert!(apply(&cfg, &edits).unwrap_err().contains("keine Zahl"));
        let mut edits = BTreeMap::new();
        edits.insert("poll_seconds", "0".to_string());
        assert!(apply(&cfg, &edits).is_err(), "validate() rejects poll_seconds = 0");
    }

    #[test]
    fn key_sources_never_show_keys() {
        let mut cfg = TeamConfig::default();
        cfg.providers.openrouter.key_env = "LUNIMA_TEST_SECRET_XYZ".into();
        std::env::set_var("LUNIMA_TEST_SECRET_XYZ", "sk-very-secret");
        let lines = key_sources(&cfg);
        assert!(lines.iter().all(|(_, l)| !l.contains("sk-very-secret")));
        assert!(lines[0].1.contains("vorhanden"));
    }
}
