//! Settings: a form over team.toml plus system toggles (autostart of agent and app).
//! Every change is saved on its own — a switch at once, a text field when it loses focus
//! or on Enter — so there is no save button to hunt for. Roles are switched on the Team
//! page, not here.

use super::theme::{self, MUTED};
use super::widgets::{card, muted, page_title, primary, toggle};
use crate::collector::{Action, Handle};
use crate::core::settings_model::{self, Kind, FIELDS};
use crate::core::snapshot::Snapshot;
use crate::sys;
use eframe::egui::{self, RichText, Ui};
use std::collections::BTreeMap;
use team_core::config::TeamConfig;

#[derive(Default)]
pub struct SettingsPage {
    /// Fields being typed in (path → text); saved when the field loses focus.
    edits: BTreeMap<&'static str, String>,
    /// Values sent for saving but not yet read back from team.toml: shown meanwhile, so a
    /// switch doesn't jump back for the moment the save takes.
    sent: BTreeMap<&'static str, (String, std::time::Instant)>,
    error: Option<String>,
}

impl SettingsPage {
    /// Returns true when the user wants the setup assistant.
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) -> bool {
        page_title(ui, "Einstellungen", "Änderungen werden sofort gespeichert und gelten ab dem nächsten Zyklus jeder Rolle, ohne Neustart. Rollen schaltest du auf der Seite Team ein und aus — oder du bittest die Rolle im Chat darum.");
        let mut open_setup = false;
        match &s.config {
            Err(e) => {
                ui.label(RichText::new(e).color(theme::ERROR));
                if primary(ui, "Einrichtungsassistent öffnen") {
                    open_setup = true;
                }
            }
            Ok(cfg) => {
                self.form(ui, cfg, handle);
                ui.add_space(10.0);
                keys(ui, cfg);
            }
        }
        ui.add_space(12.0);
        open_setup |= system(ui, s, handle);
        open_setup
    }

    fn form(&mut self, ui: &mut Ui, cfg: &TeamConfig, handle: &Handle) {
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::ERROR));
            ui.add_space(6.0);
        }
        if !self.edits.is_empty() {
            ui.label(RichText::new(format!("{} Feld(er) noch nicht gespeichert — Enter drücken oder das Feld verlassen", self.edits.len())).color(theme::WARN));
            ui.add_space(6.0);
        }
        let mut groups: Vec<&str> = Vec::new();
        for f in FIELDS.iter().filter(|f| f.group != ROLES_GROUP) {
            if !groups.contains(&f.group) {
                groups.push(f.group);
            }
        }
        let mut save: Vec<&'static str> = Vec::new();
        for group in groups {
            card(ui, Some(group), |ui| {
                egui::Grid::new(group).num_columns(2).spacing([24.0, 10.0]).min_col_width(210.0).show(ui, |ui| {
                    for spec in FIELDS.iter().filter(|f| f.group == group && settings_model::exists(cfg, f.path)) {
                        ui.vertical(|ui| {
                            ui.label(spec.label);
                            if !spec.help.is_empty() {
                                ui.label(RichText::new(spec.help).color(MUTED).size(11.5));
                            }
                        });
                        let saved = settings_model::get(cfg, spec.path);
                        if self.sent.get(spec.path).is_some_and(|(v, at)| *v == saved || at.elapsed() > SENT_FOR || handle.failed_since(*at)) {
                            self.sent.remove(spec.path);
                        }
                        let shown = self.sent.get(spec.path).map_or(saved, |(v, _)| v.clone());
                        let mut text = self.edits.get(spec.path).cloned().unwrap_or(shown);
                        let before = text.clone();
                        let done = field(ui, spec.kind, &mut text);
                        if text != before {
                            self.edits.insert(spec.path, text);
                        }
                        if done && self.edits.contains_key(spec.path) {
                            save.push(spec.path);
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(10.0);
        }
        for path in save {
            self.save(cfg, path, handle);
        }
    }

    /// Saves what is still being typed — called when the page is left, where a text field
    /// never gets its "focus lost".
    pub fn flush(&mut self, s: &Snapshot, handle: &Handle) {
        if let Some(cfg) = s.cfg() {
            let paths: Vec<&'static str> = self.edits.keys().copied().collect();
            for path in paths {
                self.save(cfg, path, handle);
            }
        }
    }

    /// Update handoff must not discard invalid text which has not been saved.
    pub fn can_update(&self) -> bool { self.edits.is_empty() && self.error.is_none() }

    /// Saves one field; a rejected value stays in the field with the reason above the form.
    fn save(&mut self, cfg: &TeamConfig, path: &'static str, handle: &Handle) {
        let Some(text) = self.edits.get(path).cloned() else {
            return;
        };
        // compared with what the field shows: a value still on its way to the file counts,
        // or clicking back to the file's value before the first click is saved gets lost
        let shown = self.sent.get(path).map(|(v, _)| v.clone()).unwrap_or_else(|| settings_model::get(cfg, path));
        if text == shown {
            self.edits.remove(path);
            self.error = None;
            return;
        }
        // checked here for an immediate message, then saved as this one change onto the
        // file's current content: the roles may have changed other settings meanwhile
        let one = BTreeMap::from([(path, text.clone())]);
        match settings_model::apply(cfg, &one) {
            Ok(_) => {
                handle.send(Action::SetSetting(path.to_string(), text.clone()));
                self.sent.insert(path, (text, std::time::Instant::now()));
                self.edits.remove(path);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }
}

/// After this the file wins again (a failed save shows its error as a toast).
const SENT_FOR: std::time::Duration = std::time::Duration::from_secs(15);

/// Roles are switched on the Team page; the form leaves them out.
const ROLES_GROUP: &str = "Rollen";

/// Draws the input; `true` when the value is final (switch flipped, Enter, focus left).
fn field(ui: &mut Ui, kind: Kind, text: &mut String) -> bool {
    match kind {
        Kind::Backend => {
            let before = text.clone();
            let options = [("claude", "Claude (Abo / Repo-Routing)"), ("codex", "Codex (ChatGPT-Abo)"), ("openrouter", "OpenRouter / Kimi (API-Key)"), ("eco", "Moonshot / eco (API-Key)"), ("openai", "OpenAI / Responses-kompatibel (API-Key)")];
            let label = options.iter().find(|(value, _)| *value == text).map(|(_, label)| *label).unwrap_or(text);
            egui::ComboBox::from_id_salt("runner-backend").selected_text(label).show_ui(ui, |ui| {
                for (value, label) in options {
                    ui.selectable_value(text, value.into(), label);
                }
            });
            *text != before
        }
        Kind::Toggle => {
            let mut on = text == "true";
            let changed = ui.add(toggle(&mut on)).changed();
            if changed {
                *text = on.to_string();
            }
            changed
        }
        Kind::Number => ui.add(egui::TextEdit::singleline(text).desired_width(110.0)).lost_focus(),
        Kind::Text => ui.add(egui::TextEdit::singleline(text).desired_width(380.0)).lost_focus(),
    }
}

fn keys(ui: &mut Ui, cfg: &TeamConfig) {
    card(ui, Some("Schlüssel für Drittanbieter (nur Anzeige)"), |ui| {
        for (name, line) in settings_model::key_sources(cfg) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).strong());
                ui.label(RichText::new(line).color(MUTED));
            });
        }
        muted(ui, "Claude/Codex nutzen dein Abo; API-Provider nutzen die vorhandenen Schlüssel. GitHub nutzt deinen gh-Login. Schlüssel werden hier nie angezeigt oder geändert.");
    });
}

/// Returns true when "Einrichtungsassistent" was clicked.
fn system(ui: &mut Ui, s: &Snapshot, handle: &Handle) -> bool {
    let mut open_setup = false;
    card(ui, Some("System"), |ui| {
        let service = s.service.as_ref().and_then(|r| r.as_ref().ok());
        ui.horizontal(|ui| {
            let mut on = service.is_some_and(|i| i.autostart);
            if ui.add_enabled(s.agent_exe.is_some() && service.is_some(), toggle(&mut on)).changed() {
                handle.send(Action::SetAgentAutostart(on));
            }
            ui.label("Team-Agent beim Anmelden starten");
        });
        ui.horizontal(|ui| {
            let mut on = s.autostart;
            if ui.add(toggle(&mut on)).changed() {
                handle.send(Action::SetAutostart(on));
            }
            ui.label("Control Center beim Anmelden starten (im Tray)");
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("📁 Datenordner").clicked() {
                sys::open_path(&s.paths.root);
            }
            if ui.button("📁 Protokolle").clicked() {
                sys::open_path(&s.paths.logs());
            }
            if ui.button("🧭 Einrichtungsassistent").clicked() {
                open_setup = true;
            }
        });
        muted(ui, format!("Daten: {}", s.paths.root.display()));
        muted(ui, format!("Team-Agent: {}", s.agent_exe.as_ref().map(|c| c.display().to_string()).unwrap_or_else(|| "nicht gefunden".into())));
    });
    open_setup
}
