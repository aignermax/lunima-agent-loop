//! Activity: every Product-Owner pass with what it actually did on GitHub.

use super::theme::{self, MUTED};
use super::widgets::{card, link, muted, page_title, pill};
use crate::core::activity::{ActionKind, Pass};
use crate::core::logs::{is_newest_run_log, Outcome, PassEntry};
use crate::core::snapshot::{relative, Snapshot};
use crate::sys;
use chrono::Local;
use eframe::egui::{self, RichText, Ui};

#[derive(Default)]
pub struct ActivityPage {
    only_with_actions: bool,
}

impl ActivityPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot) {
        page_title(ui, "Aktivität", "Was der Product Owner in seinen Läufen getan hat — direkt aus seinen Protokollen.");
        ui.checkbox(&mut self.only_with_actions, "Nur Läufe mit Aktionen oder Fehlern");
        ui.add_space(8.0);
        let entries: Vec<&PassEntry> = s
            .passes
            .iter()
            .filter(|e| !self.only_with_actions || !e.pass.actions.is_empty() || matches!(e.outcome(false), Outcome::Failed(_)))
            .collect();
        if entries.is_empty() {
            muted(ui, "Keine PO-Läufe gefunden.");
        }
        for (i, entry) in entries.into_iter().enumerate() {
            pass_card(ui, entry, i == 0, s.loop_running && is_newest_run_log(&s.logs, &entry.file));
            ui.add_space(10.0);
        }
    }
}

fn status(entry: &PassEntry, running: bool) -> (&'static str, egui::Color32) {
    match entry.outcome(running) {
        Outcome::Running => ("läuft", theme::INFO),
        Outcome::Aborted => ("abgebrochen", theme::WARN),
        Outcome::Failed(_) => ("Fehler", theme::ERROR),
        Outcome::Succeeded => ("erfolgreich", theme::OK),
    }
}

fn counts(pass: &Pass) -> String {
    let n = |k: ActionKind| pass.actions.iter().filter(|a| a.kind == k).count();
    let parts = [
        (n(ActionKind::IssueCreated), "Issue erstellt", "Issues erstellt"),
        (n(ActionKind::PrMerged), "PR gemergt", "PRs gemergt"),
        (n(ActionKind::IssueClosed), "Issue geschlossen", "Issues geschlossen"),
        (n(ActionKind::LabelAdded), "Label", "Labels"),
        (n(ActionKind::Comment), "Kommentar", "Kommentare"),
    ];
    let text: Vec<String> = parts.iter().filter(|(c, _, _)| *c > 0).map(|(c, one, many)| format!("{c} {}", if *c == 1 { one } else { many })).collect();
    if text.is_empty() { "keine GitHub-Aktionen".into() } else { text.join(" · ") }
}

fn pass_card(ui: &mut Ui, entry: &PassEntry, open: bool, running: bool) {
    let pass = &entry.pass;
    let now = Local::now();
    let (label, color) = status(entry, running);
    card(ui, None, |ui| {
        ui.horizontal(|ui| {
            let when = entry.file.started.map(|t| format!("{} · {}", t.format("%d.%m. %H:%M"), relative(t, now))).unwrap_or_else(|| entry.file.name.clone());
            ui.label(RichText::new(when).strong().size(15.0));
            pill(ui, label, color);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Log öffnen").clicked() {
                    sys::open_path(&entry.file.path);
                }
            });
        });
        let meta = [
            Some(counts(pass)),
            pass.duration_min.map(|m| format!("{m:.0} min")),
            pass.turns.map(|t| format!("{t} Schritte")),
            pass.cost_usd.filter(|c| *c > 0.0).map(|c| format!("${c:.2}")),
        ];
        ui.label(RichText::new(meta.into_iter().flatten().collect::<Vec<_>>().join("  ·  ")).color(MUTED));
        egui::CollapsingHeader::new("Details").id_salt(&entry.file.name).default_open(open).show(ui, |ui| details(ui, pass));
    });
}

fn details(ui: &mut Ui, pass: &Pass) {
    if pass.actions.is_empty() {
        muted(ui, "Keine Issues, Merges oder Labels in diesem Lauf.");
    }
    for a in &pass.actions {
        ui.horizontal(|ui| {
            let color = if a.failed { theme::ERROR } else { kind_color(a.kind) };
            pill(ui, a.kind.label(), color);
            match &a.url {
                Some(url) => link(ui, &a.text, url),
                None => {
                    ui.label(&a.text);
                }
            }
            if a.failed {
                ui.label(RichText::new("fehlgeschlagen").color(theme::ERROR).size(12.0));
            }
        });
    }
    if pass.tool_errors > 0 {
        muted(ui, format!("{} fehlgeschlagene Befehle im Lauf", pass.tool_errors));
    }
    if let Some(summary) = &pass.summary {
        ui.add_space(6.0);
        ui.label(RichText::new("Abschlussbericht").color(MUTED).size(12.0));
        egui::Frame::new().fill(theme::BG).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(summary).size(13.5)).wrap());
        });
    }
}

fn kind_color(kind: ActionKind) -> egui::Color32 {
    match kind {
        ActionKind::IssueCreated | ActionKind::PrCreated => theme::ACCENT,
        ActionKind::PrMerged => theme::OK,
        ActionKind::IssueClosed => theme::MUTED,
        ActionKind::LabelAdded | ActionKind::PrRetargeted => theme::INFO,
        ActionKind::Comment => theme::WARN,
    }
}
