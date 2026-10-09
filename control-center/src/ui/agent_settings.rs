//! Settings → Issue-Agent: models per role, budgets, labels and operation as a form over
//! the agent's .env (comments survive; secrets are shown masked and never edited here).

use super::theme::{self, MUTED};
use super::widgets::{card, muted, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::envfile::EnvFile;
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Number,
    Toggle,
}

struct EnvField {
    group: &'static str,
    key: &'static str,
    label: &'static str,
    kind: Kind,
    default: &'static str,
    help: &'static str,
}

const fn e(group: &'static str, key: &'static str, label: &'static str, kind: Kind, default: &'static str, help: &'static str) -> EnvField {
    EnvField { group, key, label, kind, default, help }
}

const FIELDS: &[EnvField] = &[
    e("Modelle", "AGENT_CODER_MODEL", "Coder (Standard)", Kind::Text, "CLI-Standard", "Interne Repos und alles ohne Kosten-Label."),
    e("Modelle", "AGENT_PREMIUM_CODER_MODEL", "Coder (complex / claudeapi)", Kind::Text, "= Coder", "Für schwere Issues und das Label claudeapi."),
    e("Modelle", "AGENT_REVIEWER_MODEL", "Reviewer & QA-Review", Kind::Text, "claude-sonnet-4-6", ""),
    e("Modelle", "AGENT_REVIEWER_MODEL_CRITICAL", "Reviewer (Label critical)", Kind::Text, "claude-opus-5-5", ""),
    e("Modelle", "AGENT_OPENROUTER_MODEL", "Kimi über OpenRouter", Kind::Text, "qwen/qwen3-coder", "Für Repos mit Modell „Kimi“ (Seite Projekte)."),
    e("Modelle", "AGENT_ECO_MODEL", "eco-Label", Kind::Text, "kimi-k2-thinking", "Nur in für Kimi freigegebenen Repos."),
    e("Budgets", "AGENT_MAX_TURNS_REGULAR", "Max. Schritte (normal)", Kind::Number, "150", ""),
    e("Budgets", "AGENT_MAX_TURNS_COMPLEX", "Max. Schritte (complex)", Kind::Number, "500", ""),
    e("Budgets", "AGENT_MAX_REVIEW_ROUNDS_REGULAR", "Review-Runden (normal)", Kind::Number, "1", ""),
    e("Budgets", "AGENT_MAX_REVIEW_ROUNDS_COMPLEX", "Review-Runden (complex)", Kind::Number, "2", ""),
    e("Budgets", "AGENT_MAX_QA_FIX_ROUNDS", "QA-Fix-Runden", Kind::Number, "2", "Danach Eskalation an needs-human."),
    e("Budgets", "AGENT_PR_FEEDBACK_MAX_ROUNDS", "PR-Feedback-Runden", Kind::Number, "3", ""),
    e("Labels", "AGENT_ISSUE_LABEL", "Task-Label", Kind::Text, "agent-task", ""),
    e("Labels", "AGENT_PR_LABELS", "Labels für Coder-PRs", Kind::Text, "", "agent-pr = der PO reviewt und merged sie."),
    e("Labels", "AGENT_SKIP_LABELS", "Nie bearbeiten bei", Kind::Text, "agent-running,needs-human", ""),
    e("Labels", "AGENT_COMPLEXITY_TAG", "Label „schwer“", Kind::Text, "complex", ""),
    e("Labels", "AGENT_CRITICAL_LABEL", "Label „kritisch“", Kind::Text, "critical", ""),
    e("Betrieb", "AGENT_COMPLEX_USES_CLAUDE", "complex → Premium-Modell", Kind::Toggle, "true", ""),
    e("Betrieb", "AGENT_POLL_INTERVAL", "Abfrage-Intervall (s)", Kind::Number, "15", ""),
    e("Betrieb", "AGENT_DISCOVERY_ORG", "Organisation für Auto-Discovery", Kind::Text, "Akhetonics", "Leer = aus."),
];

const SECRETS: &[&str] = &["ANTHROPIC_API_KEY", "GITHUB_TOKEN", "AGENT_OPENROUTER_API_KEY", "AGENT_ECO_API_KEY"];

#[derive(Default)]
pub struct AgentSettings {
    draft: Option<EnvFile>,
    error: Option<String>,
}

impl AgentSettings {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        let Some(file_env) = &s.team.env else {
            muted(ui, "Issue-Agent nicht verbunden — siehe Seite Team.");
            return;
        };
        let mut env = self.draft.clone().unwrap_or_else(|| file_env.clone());
        let before: Vec<Option<String>> = FIELDS.iter().map(|f| env.get(f.key)).collect();
        let mut groups: Vec<&str> = FIELDS.iter().map(|f| f.group).collect();
        groups.dedup();
        for group in groups {
            card(ui, Some(group), |ui| {
                egui::Grid::new(("agent", group)).num_columns(2).spacing([24.0, 10.0]).min_col_width(210.0).show(ui, |ui| {
                    for f in FIELDS.iter().filter(|f| f.group == group) {
                        field_row(ui, f, &mut env);
                        ui.end_row();
                    }
                });
            });
            ui.add_space(10.0);
        }
        if FIELDS.iter().map(|f| env.get(f.key)).collect::<Vec<_>>() != before {
            self.draft = Some(env);
            self.error = None;
        }
        secrets(ui, file_env);
        ui.add_space(10.0);
        self.footer(ui, handle);
    }

    fn footer(&mut self, ui: &mut Ui, handle: &Handle) {
        let dirty = self.draft.is_some();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern & Agents neu starten") {
                    match self.draft.as_ref().map(validate) {
                        Some(Err(e)) => self.error = Some(e),
                        _ => {
                            if let Some(d) = self.draft.take() {
                                handle.send(Action::SaveEnv(d, true));
                            }
                        }
                    }
                }
                if secondary(ui, "Verwerfen") {
                    self.draft = None;
                    self.error = None;
                }
            });
            if dirty {
                ui.label(RichText::new("• ungespeichert — laufende Arbeit wird beim Neustart abgebrochen").color(theme::WARN));
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme::ERROR));
            }
        });
    }
}

fn field_row(ui: &mut Ui, f: &EnvField, env: &mut EnvFile) {
    ui.vertical(|ui| {
        ui.label(f.label);
        let hint = if f.help.is_empty() { format!("{} · Standard: {}", f.key, f.default) } else { format!("{} · {}", f.key, f.help) };
        ui.label(RichText::new(hint).color(MUTED).size(11.0));
    });
    let current = env.get(f.key).unwrap_or_default();
    let mut text = current.clone();
    match f.kind {
        Kind::Toggle => {
            let mut on = if current.is_empty() { f.default == "true" } else { current.eq_ignore_ascii_case("true") };
            if ui.checkbox(&mut on, "").changed() {
                text = on.to_string();
            }
        }
        Kind::Number => {
            ui.add(egui::TextEdit::singleline(&mut text).hint_text(f.default).desired_width(110.0));
        }
        Kind::Text => {
            ui.add(egui::TextEdit::singleline(&mut text).hint_text(f.default).desired_width(360.0));
        }
    }
    if text != current {
        env.set(f.key, &text);
    }
}

fn validate(env: &EnvFile) -> Result<(), String> {
    for f in FIELDS.iter().filter(|f| f.kind == Kind::Number) {
        if let Some(v) = env.get(f.key).filter(|v| !v.is_empty()) {
            if v.parse::<u32>().is_err() {
                return Err(format!("{}: „{v}“ ist keine Zahl", f.label));
            }
        }
    }
    Ok(())
}

fn secrets(ui: &mut Ui, env: &EnvFile) {
    card(ui, Some("Zugangsdaten (nur Anzeige)"), |ui| {
        for key in SECRETS {
            ui.horizontal(|ui| {
                ui.label(RichText::new(*key).monospace().size(12.5));
                ui.label(RichText::new(env.display(key)).color(MUTED));
            });
        }
        muted(ui, "Schlüssel werden nur direkt in der .env geändert — nie über diese Oberfläche.");
    });
}
