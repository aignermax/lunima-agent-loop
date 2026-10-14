//! Settings: a form over team.toml plus system toggles (autostart of agent and app).

use super::theme::{self, MUTED};
use super::widgets::{card, muted, page_title, primary, secondary, toggle};
use crate::collector::{Action, Handle};
use crate::core::settings_model::{self, Kind, FIELDS};
use crate::core::snapshot::Snapshot;
use crate::sys;
use eframe::egui::{self, RichText, Ui};
use std::collections::BTreeMap;
use team_core::config::TeamConfig;

#[derive(Default)]
pub struct SettingsPage {
    /// Edit buffer (path → text). Empty = not editing, mirror the file.
    edits: BTreeMap<&'static str, String>,
    error: Option<String>,
}

impl SettingsPage {
    /// Returns true when the user wants the setup assistant.
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) -> bool {
        page_title(ui, "Einstellungen", "team.toml — gilt ab dem nächsten Zyklus jeder Rolle, ohne Neustart.");
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
        let mut groups: Vec<&str> = FIELDS.iter().map(|f| f.group).collect();
        groups.dedup();
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
                        let mut text = self.edits.get(spec.path).cloned().unwrap_or_else(|| settings_model::get(cfg, spec.path));
                        let before = text.clone();
                        field(ui, spec.kind, &mut text);
                        if text != before {
                            if text == settings_model::get(cfg, spec.path) {
                                self.edits.remove(spec.path);
                            } else {
                                self.edits.insert(spec.path, text);
                            }
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(10.0);
        }
        self.footer(ui, cfg, handle);
    }

    fn footer(&mut self, ui: &mut Ui, cfg: &TeamConfig, handle: &Handle) {
        ui.horizontal(|ui| {
            let dirty = !self.edits.is_empty();
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern") {
                    match settings_model::apply(cfg, &self.edits) {
                        Ok(updated) => {
                            handle.send(Action::SaveConfig(Box::new(updated)));
                            self.edits.clear();
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                if secondary(ui, "Verwerfen") {
                    self.edits.clear();
                    self.error = None;
                }
            });
            if dirty {
                ui.label(RichText::new(format!("{} ungespeicherte Änderung(en)", self.edits.len())).color(theme::WARN));
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme::ERROR));
            }
        });
    }
}

fn field(ui: &mut Ui, kind: Kind, text: &mut String) {
    match kind {
        Kind::Toggle => {
            let mut on = text == "true";
            if ui.add(toggle(&mut on)).changed() {
                *text = on.to_string();
            }
        }
        Kind::Number => {
            ui.add(egui::TextEdit::singleline(text).desired_width(110.0));
        }
        Kind::Text => {
            ui.add(egui::TextEdit::singleline(text).desired_width(380.0));
        }
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
        muted(ui, "Claude läuft über dein Abo, GitHub über deinen gh-Login. Schlüssel werden hier nie angezeigt oder geändert.");
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
