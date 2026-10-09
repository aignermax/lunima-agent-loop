//! Rules: edit the prompt of every role — PO and loop worker (loop) as well as coder,
//! reviewer, QA and PR-feedback (issue agent). An override file wins over the built-in.

use super::theme::{self, MUTED};
use super::widgets::{card, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::prompts::{self, Builtin, PromptDoc};
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};
use std::time::{Duration, Instant};

const LIST_WIDTH: f32 = 250.0;
/// Prompt files are re-read at most this often (the agent's live on \\wsl$ / a slow mount).
const RESCAN: Duration = Duration::from_secs(3);

/// Editor state for the selected prompt.
struct Buffer {
    id: String,
    /// What is in effect: the override, or the built-in when there is none.
    saved: String,
    text: String,
    overridden: bool,
    builtin: String,
}

#[derive(Default)]
pub struct RulesPage {
    selected: Option<String>,
    buffer: Option<Buffer>,
    /// (when, ids of prompts with a custom override) — avoids file I/O every frame
    customised: Option<(Instant, Vec<String>)>,
}

impl RulesPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Regeln", "Die Anweisungen jeder Rolle. Änderungen gelten ab dem nächsten Lauf der Rolle.");
        let docs = prompts::all(&s.root, s.team.dir.as_deref());
        if self.customised.as_ref().is_none_or(|(t, _)| t.elapsed() > RESCAN) {
            self.customised = Some((Instant::now(), docs.iter().filter(|d| d.is_customised()).map(|d| d.id.clone()).collect()));
        }
        let selected = self.selected.clone().filter(|id| docs.iter().any(|d| &d.id == id)).unwrap_or_else(|| docs[0].id.clone());
        // measured before the row: nested horizontal layouts don't bound their width
        let width = (ui.available_width() - LIST_WIDTH - ui.spacing().item_spacing.x * 2.0).max(320.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(LIST_WIDTH, 600.0), egui::Layout::top_down(egui::Align::Min), |ui| self.list(ui, &docs, &selected));
            ui.allocate_ui_with_layout(egui::vec2(width, 600.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(width);
                if let Some(doc) = docs.iter().find(|d| d.id == selected) {
                    self.editor(ui, doc, handle, width);
                }
            });
        });
        if s.team.dir.is_none() {
            ui.add_space(8.0);
            ui.label(RichText::new("Coder-, Reviewer-, QA- und PR-Feedback-Regeln erscheinen, sobald der Issue-Agent verbunden ist (Seite Team).").color(MUTED));
        }
    }

    fn list(&mut self, ui: &mut Ui, docs: &[PromptDoc], selected: &str) {
        card(ui, None, |ui| {
            let mut last_role = "";
            for d in docs {
                if d.role != last_role {
                    ui.add_space(4.0);
                    ui.label(RichText::new(d.role).strong().size(13.5));
                    last_role = d.role;
                }
                let custom = self.customised.as_ref().is_some_and(|(_, ids)| ids.contains(&d.id));
                let marker = if custom { "  •" } else { "" };
                if ui.selectable_label(d.id == selected, RichText::new(format!("{}{marker}", d.title)).size(13.0)).clicked() {
                    self.selected = Some(d.id.clone());
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("• = eigene Fassung aktiv").color(MUTED).size(11.5));
        });
    }

    fn editor(&mut self, ui: &mut Ui, doc: &PromptDoc, handle: &Handle, width: f32) {
        if self.buffer.as_ref().is_none_or(|b| b.id != doc.id) {
            let builtin = doc.builtin_text();
            let saved = doc.override_text().unwrap_or_else(|| builtin.clone());
            self.buffer = Some(Buffer { id: doc.id.clone(), text: saved.clone(), saved, overridden: doc.is_customised(), builtin });
            self.selected = Some(doc.id.clone());
        }
        let Some(b) = self.buffer.as_mut() else { return };
        let builtin = b.builtin.clone();
        toolbar(ui, doc, b, &builtin, handle);
        ui.add_space(6.0);
        ui.scope(|ui| {
            ui.set_max_width(width);
            placeholder_line(ui, doc, &builtin, &b.text);
        });
        ui.add_space(6.0);
        card(ui, None, |ui| {
            egui::ScrollArea::vertical().id_salt(("rules-editor", &doc.id)).max_height(ui.available_height().max(460.0)).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut b.text).font(egui::TextStyle::Monospace).code_editor().desired_width(width - 2.0 * f32::from(theme::CARD_PAD) - 16.0).desired_rows(28).lock_focus(true));
            });
        });
    }
}

fn toolbar(ui: &mut Ui, doc: &PromptDoc, b: &mut Buffer, builtin: &str, handle: &Handle) {
    let dirty = b.saved != b.text;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} — {}", doc.role, doc.title)).size(16.0).strong());
        let (state, color) = if b.overridden { ("eigene Fassung", theme::ACCENT) } else { ("eingebaut", theme::OK) };
        pill(ui, state, color);
    });
    ui.horizontal(|ui| {
        ui.add_enabled_ui(dirty, |ui| {
            if primary(ui, "Speichern") {
                handle.send(Action::SavePrompt(doc.override_path.clone(), b.text.clone()));
                b.saved = b.text.clone();
                b.overridden = true;
            }
            if secondary(ui, "Verwerfen") {
                b.text = b.saved.clone();
            }
        });
        // in a checkout of the loop repo prompts/*.md are the tracked sources: managed with
        // git, never reset from the (possibly older) copy compiled into this app
        let hint = if doc.deletable { "Löscht die eigene Datei." } else { "Im Repo-Checkout: Quelldatei — Änderungen per git verwalten." };
        let reset = ui.add_enabled(b.overridden && doc.deletable, egui::Button::new("Eingebaute Fassung verwenden")).on_disabled_hover_text(hint).on_hover_text(hint);
        if reset.clicked() {
            handle.send(Action::DeletePrompt(doc.override_path.clone()));
            b.overridden = false;
            b.saved = builtin.to_string();
            b.text = b.saved.clone();
        }
        if dirty {
            ui.label(RichText::new("• ungespeichert").color(theme::WARN));
        }
    });
    ui.label(RichText::new(doc.override_path.display().to_string()).color(MUTED).size(11.5));
}

/// Placeholders of the built-in; for the issue agent (Python `str.format`) unknown ones in
/// the edited text are flagged — the agent would fall back to its built-in prompt.
fn placeholder_line(ui: &mut Ui, doc: &PromptDoc, builtin: &str, text: &str) {
    let known = prompts::placeholders(builtin);
    // plain wrapped text: framed pills can't wrap (their width is unknown before layout)
    let names: Vec<String> = known.iter().map(|p| format!("{{{p}}}")).collect();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Platzhalter:").color(MUTED).size(12.0));
        ui.add(egui::Label::new(RichText::new(names.join("   ")).monospace().size(12.0).color(theme::ACCENT)).wrap());
    });
    if matches!(doc.builtin, Builtin::File(_)) {
        let problems = prompts::format_problems(text, &known);
        if !problems.is_empty() {
            let msg = format!("{} — so würde der Agent seine eingebaute Fassung nutzen. Echte geschweifte Klammern verdoppeln: {{{{ }}}}", problems.join("; "));
            ui.add(egui::Label::new(RichText::new(msg).color(theme::WARN).size(12.5)).wrap());
        }
    }
}
