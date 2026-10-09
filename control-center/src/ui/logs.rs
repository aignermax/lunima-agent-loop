//! Logs: raw view of every log file — including the loop's own console mirror, where
//! the real error behind a failed run shows up.

use super::theme::MUTED;
use super::widgets::{card, page_title};
use crate::core::logs::{tail, LogKind};
use crate::core::snapshot::Snapshot;
use crate::sys;
use eframe::egui::{self, RichText, Ui};
use std::path::PathBuf;

const TAIL_LINES: usize = 400;
const FILES_SHOWN: usize = 80;
const LIST_WIDTH: f32 = 300.0;

#[derive(Default)]
pub struct LogsPage {
    filter: Filter,
    selected: Option<PathBuf>,
    cached: Option<(PathBuf, u64, String)>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Filter {
    #[default]
    Loop,
    Owner,
    Task,
    All,
}

impl Filter {
    fn matches(self, kind: LogKind) -> bool {
        match self {
            Filter::Loop => kind == LogKind::Loop,
            Filter::Owner => kind == LogKind::Owner,
            Filter::Task => matches!(kind, LogKind::Task(_)),
            Filter::All => true,
        }
    }
}

impl LogsPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot) {
        page_title(ui, "Protokolle", "Loop-Konsole (Fehlerursachen), PO-Läufe und Worker-Läufe im Rohformat.");
        ui.horizontal(|ui| {
            for (f, label) in [(Filter::Loop, "Loop-Konsole"), (Filter::Owner, "PO-Läufe"), (Filter::Task, "Worker"), (Filter::All, "Alle")] {
                if ui.selectable_label(self.filter == f, label).clicked() {
                    self.filter = f;
                }
            }
        });
        ui.add_space(8.0);
        // the console mirror only exists from loop v0.3 on — fall back to PO passes until then
        if self.filter == Filter::Loop && !s.logs.iter().any(|f| f.kind == LogKind::Loop) {
            self.filter = Filter::Owner;
        }
        let files: Vec<_> = s.logs.iter().filter(|f| self.filter.matches(f.kind)).take(FILES_SHOWN).collect();
        if self.selected.as_ref().is_none_or(|sel| !files.iter().any(|f| &f.path == sel)) {
            self.selected = files.first().map(|f| f.path.clone());
        }
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(LIST_WIDTH, 560.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                card(ui, None, |ui| {
                    egui::ScrollArea::vertical().id_salt("log-list").max_height(540.0).show(ui, |ui| {
                        for f in &files {
                            let label = format!("{}  ·  {} KB", f.name, f.size.div_ceil(1024));
                            if ui.selectable_label(self.selected.as_ref() == Some(&f.path), RichText::new(label).size(12.5)).clicked() {
                                self.selected = Some(f.path.clone());
                            }
                        }
                        if files.is_empty() {
                            ui.label(RichText::new("Keine Dateien.").color(MUTED));
                        }
                    });
                });
            });
            ui.vertical(|ui| self.viewer(ui, s));
        });
    }

    fn viewer(&mut self, ui: &mut Ui, s: &Snapshot) {
        let Some(path) = self.selected.clone() else { return };
        let size = s.logs.iter().find(|f| f.path == path).map(|f| f.size).unwrap_or(0);
        if self.cached.as_ref().is_none_or(|(p, sz, _)| p != &path || *sz != size) {
            self.cached = Some((path.clone(), size, tail(&path, TAIL_LINES)));
        }
        let text = self.cached.as_ref().map(|(_, _, t)| t.as_str()).unwrap_or("");
        card(ui, None, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("letzte {TAIL_LINES} Zeilen")).color(MUTED).size(12.0));
                if ui.small_button("Datei öffnen").clicked() {
                    sys::open_path(&path);
                }
            });
            egui::ScrollArea::both().id_salt("log-view").max_height(530.0).stick_to_bottom(true).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).extend());
            });
        });
    }
}
