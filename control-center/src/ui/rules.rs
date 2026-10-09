//! Rules: edit the Product-Owner and worker prompts. A file in prompts/ overrides the
//! prompt built into the CLI; without one the built-in rules are in effect.

use super::theme::{self, MUTED};
use super::widgets::{card, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};
use std::path::{Path, PathBuf};

const DEFAULT_OWNER: &str = include_str!("../../../prompts/owner.md");
const DEFAULT_WORKER: &str = include_str!("../../../prompts/worker.md");

const OWNER_PLACEHOLDERS: &[&str] = &["{REPO}", "{INTEGRATION_BRANCH}", "{BASE_BRANCH}", "{TASK_LABEL}", "{PR_LABEL}", "{BLOCKED_LABEL}", "{DATE}", "{ROADMAP}", "{OPEN_PRS}", "{OPEN_TASK_ISSUES}"];
const WORKER_PLACEHOLDERS: &[&str] = &["{REPO}", "{ISSUE_NUMBER}", "{ISSUE_TITLE}", "{ISSUE_BODY}", "{BRANCH}", "{INTEGRATION_BRANCH}", "{BASE_BRANCH}", "{PR_LABEL}", "{BLOCKED_LABEL}", "{DATE}"];

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Owner,
    Worker,
}

impl Which {
    fn file(self) -> &'static str {
        match self {
            Which::Owner => "owner.md",
            Which::Worker => "worker.md",
        }
    }
    fn default_text(self) -> &'static str {
        match self {
            Which::Owner => DEFAULT_OWNER,
            Which::Worker => DEFAULT_WORKER,
        }
    }
    fn placeholders(self) -> &'static [&'static str] {
        match self {
            Which::Owner => OWNER_PLACEHOLDERS,
            Which::Worker => WORKER_PLACEHOLDERS,
        }
    }
}

/// Editor state for one prompt file.
struct Buffer {
    path: PathBuf,
    /// What is in effect: the override file, or the built-in default when there is none.
    saved: String,
    text: String,
    /// An override file exists in prompts/ (it wins over the prompt built into the CLI).
    overridden: bool,
}

pub struct RulesPage {
    which: Which,
    buffer: Option<Buffer>,
}

impl Default for RulesPage {
    fn default() -> Self {
        Self { which: Which::Owner, buffer: None }
    }
}

impl RulesPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Regeln", "Die Anweisungen, nach denen Product Owner und Worker arbeiten. Änderungen gelten ab dem nächsten Lauf.");
        ui.horizontal(|ui| {
            for (w, label) in [(Which::Owner, "Product Owner"), (Which::Worker, "Worker")] {
                if ui.selectable_label(self.which == w, label).clicked() && self.which != w {
                    self.which = w;
                    self.buffer = None;
                }
            }
        });
        ui.add_space(8.0);
        let path = s.prompts_dir().join(self.which.file());
        if self.buffer.as_ref().is_none_or(|b| b.path != path) {
            let on_disk = std::fs::read_to_string(&path).ok();
            let overridden = on_disk.is_some();
            let saved = on_disk.unwrap_or_else(|| self.which.default_text().to_string());
            self.buffer = Some(Buffer { path: path.clone(), text: saved.clone(), saved, overridden });
        }
        self.toolbar(ui, &path, handle);
        ui.add_space(6.0);
        self.placeholders(ui);
        ui.add_space(6.0);
        self.editor(ui);
    }

    fn toolbar(&mut self, ui: &mut Ui, path: &Path, handle: &Handle) {
        let default_text = self.which.default_text();
        let Some(b) = self.buffer.as_mut() else { return };
        let dirty = b.saved != b.text;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern") {
                    handle.send(Action::SavePrompt(path.to_path_buf(), b.text.clone()));
                    b.saved = b.text.clone();
                    b.overridden = true;
                }
                if secondary(ui, "Verwerfen") {
                    b.text = b.saved.clone();
                }
            });
            let reset = ui
                .add_enabled(b.overridden, egui::Button::new("Eingebaute Regeln verwenden"))
                .on_hover_text("Löscht die eigene Datei — der Loop nutzt dann wieder seine eingebauten Regeln.");
            if reset.clicked() {
                handle.send(Action::DeletePrompt(path.to_path_buf()));
                b.overridden = false;
                b.saved = default_text.to_string();
                b.text = b.saved.clone();
            }
            let (state, color) = if b.overridden { ("Eigene Regeln aktiv", theme::ACCENT) } else { ("Eingebaute Regeln aktiv", theme::OK) };
            pill(ui, state, color);
            if dirty {
                ui.label(RichText::new("● ungespeichert").color(theme::WARN));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(path.display().to_string()).color(MUTED).size(12.0));
            });
        });
    }

    fn placeholders(&self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Platzhalter:").color(MUTED).size(12.0));
            for p in self.which.placeholders() {
                pill(ui, p, theme::ACCENT);
            }
        });
    }

    fn editor(&mut self, ui: &mut Ui) {
        let Some(b) = self.buffer.as_mut() else { return };
        card(ui, None, |ui| {
            egui::ScrollArea::vertical().id_salt("rules-editor").max_height(ui.available_height().max(420.0)).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut b.text).font(egui::TextStyle::Monospace).code_editor().desired_width(f32::INFINITY).desired_rows(30).lock_focus(true));
            });
        });
    }
}
