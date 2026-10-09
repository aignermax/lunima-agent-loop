//! Rules: edit the Product-Owner and worker prompts (prompts/*.md override the built-in ones).

use super::theme::{self, MUTED};
use super::widgets::{card, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};
use std::path::PathBuf;

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

pub struct RulesPage {
    which: Which,
    /// (file the buffer belongs to, text on disk when loaded, edited text)
    buffer: Option<(PathBuf, String, String)>,
}

impl Default for RulesPage {
    fn default() -> Self {
        Self { which: Which::Owner, buffer: None }
    }
}

impl RulesPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Regeln", "Die Anweisungen, nach denen Product Owner und Worker arbeiten.");
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
        if self.buffer.as_ref().is_none_or(|(p, _, _)| p != &path) {
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            let text = if on_disk.is_empty() { self.which.default_text().to_string() } else { on_disk.clone() };
            self.buffer = Some((path.clone(), on_disk, text));
        }
        self.toolbar(ui, &path, handle);
        ui.add_space(6.0);
        self.placeholders(ui);
        ui.add_space(6.0);
        self.editor(ui);
    }

    fn toolbar(&mut self, ui: &mut Ui, path: &std::path::Path, handle: &Handle) {
        let Some((_, saved, text)) = self.buffer.as_mut() else { return };
        let dirty = saved != text;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern") {
                    handle.send(Action::SavePrompt(path.to_path_buf(), text.clone()));
                    *saved = text.clone();
                }
                if secondary(ui, "Verwerfen") {
                    *text = saved.clone();
                }
            });
            if secondary(ui, "Standard wiederherstellen") {
                *text = self.which.default_text().to_string();
            }
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
        let Some((_, _, text)) = self.buffer.as_mut() else { return };
        card(ui, None, |ui| {
            egui::ScrollArea::vertical().id_salt("rules-editor").max_height(ui.available_height().max(420.0)).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(text).font(egui::TextStyle::Monospace).code_editor().desired_width(f32::INFINITY).desired_rows(30).lock_focus(true));
            });
        });
    }
}
