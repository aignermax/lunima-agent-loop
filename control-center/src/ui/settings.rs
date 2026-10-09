//! Settings: a form over agent-loop.json plus system toggles (schedule, autostart).

use super::theme::{self, MUTED};
use super::widgets::{card, muted, page_title, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::config::{ConfigDoc, FieldKind, FIELDS};
use crate::core::snapshot::Snapshot;
use crate::sys;
use eframe::egui::{self, RichText, Ui};
use std::collections::HashMap;

#[derive(Default)]
pub struct SettingsPage {
    /// Edit buffer (key → text). Empty = not editing, mirror the file.
    edits: HashMap<&'static str, String>,
    error: Option<String>,
}

impl SettingsPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Einstellungen", "agent-loop.json — Änderungen gelten ab dem nächsten Lauf.");
        match &s.config {
            Err(e) => {
                ui.label(RichText::new(e).color(theme::ERROR));
            }
            Ok(doc) => self.form(ui, doc, handle),
        }
        ui.add_space(12.0);
        system(ui, s, handle);
    }

    fn value(&self, doc: &ConfigDoc, key: &'static str) -> String {
        self.edits.get(key).cloned().unwrap_or_else(|| doc.str(key))
    }

    fn form(&mut self, ui: &mut Ui, doc: &ConfigDoc, handle: &Handle) {
        let mut groups: Vec<&str> = FIELDS.iter().map(|f| f.group).collect();
        groups.dedup();
        for group in groups {
            card(ui, Some(group), |ui| {
                egui::Grid::new(group).num_columns(2).spacing([24.0, 10.0]).min_col_width(190.0).show(ui, |ui| {
                    for spec in FIELDS.iter().filter(|f| f.group == group) {
                        ui.vertical(|ui| {
                            ui.label(spec.label);
                            if !spec.help.is_empty() {
                                ui.label(RichText::new(spec.help).color(MUTED).size(11.5));
                            }
                        });
                        let mut text = self.value(doc, spec.key);
                        let before = text.clone();
                        field(ui, spec.kind, &mut text);
                        if text != before {
                            self.edits.insert(spec.key, text);
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(10.0);
        }
        self.footer(ui, doc, handle);
    }

    fn footer(&mut self, ui: &mut Ui, doc: &ConfigDoc, handle: &Handle) {
        ui.horizontal(|ui| {
            let dirty = !self.edits.is_empty();
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern") {
                    self.save(doc, handle);
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

    fn save(&mut self, doc: &ConfigDoc, handle: &Handle) {
        let mut updated = doc.clone();
        for spec in FIELDS {
            if let Some(text) = self.edits.get(spec.key) {
                if let Err(e) = updated.set_from_text(spec, text) {
                    self.error = Some(e);
                    return;
                }
            }
        }
        handle.send(Action::SaveConfig(updated));
        self.edits.clear();
        self.error = None;
    }
}

fn field(ui: &mut Ui, kind: FieldKind, text: &mut String) {
    match kind {
        FieldKind::Toggle => {
            let mut on = text == "true";
            if ui.add(toggle(&mut on)).changed() {
                *text = on.to_string();
            }
        }
        FieldKind::Choice(options) => {
            egui::ComboBox::from_id_salt(options.as_ptr()).selected_text(text.as_str()).show_ui(ui, |ui| {
                for o in options {
                    ui.selectable_value(text, o.to_string(), *o);
                }
            });
        }
        FieldKind::Number => {
            ui.add(egui::TextEdit::singleline(text).desired_width(110.0));
        }
        FieldKind::Text => {
            ui.add(egui::TextEdit::singleline(text).desired_width(380.0));
        }
    }
}

/// iOS-style switch.
fn toggle(on: &mut bool) -> impl egui::Widget + '_ {
    move |ui: &mut Ui| {
        let size = egui::vec2(40.0, 22.0);
        let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let fill = if *on { theme::ACCENT } else { egui::Color32::from_rgb(0x33, 0x3B, 0x4F) };
        ui.painter().rect_filled(rect, egui::CornerRadius::same(11), fill);
        let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, egui::Color32::WHITE);
        response
    }
}

fn system(ui: &mut Ui, s: &Snapshot, handle: &Handle) {
    card(ui, Some("System"), |ui| {
        let task = s.task.as_ref().and_then(|t| t.as_ref().ok());
        ui.horizontal(|ui| {
            let mut on = task.is_some_and(|t| t.exists && t.state != "Disabled");
            if ui.add_enabled(task.is_some_and(|t| t.exists), toggle(&mut on)).changed() {
                handle.send(Action::SetTaskEnabled(on));
            }
            ui.label("Stündlicher Zeitplan (Windows-Aufgabe 'LunimaAgentLoop')");
        });
        ui.horizontal(|ui| {
            let mut on = s.autostart;
            if ui.add(toggle(&mut on)).changed() {
                handle.send(Action::SetAutostart(on));
            }
            ui.label("Control Center mit Windows starten (im Tray)");
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("📁 Loop-Ordner").clicked() {
                sys::open_path(&s.root);
            }
            if let Some(clone) = s.clone_path() {
                if ui.button("📁 Lunima-Clone").clicked() {
                    sys::open_path(&clone);
                }
            }
            if ui.button("📁 Protokolle").clicked() {
                sys::open_path(&s.logs_dir());
            }
        });
        muted(ui, format!("Loop: {}", s.root.display()));
        muted(ui, format!("CLI: {}", s.cli.as_ref().map(|c| c.display().to_string()).unwrap_or_else(|| "nicht gefunden".into())));
    });
}
