//! Overview: one glance answers "does it work, what's next, what needs me?"

use super::theme::{self, MUTED};
use super::widgets::{self, card, level_dot, pill, primary, secondary, stat};
use crate::collector::{Action, Handle};
use crate::core::health::{Fix, Health, Level};
use crate::core::snapshot::{relative, Snapshot};
use crate::core::state::{parse_time, Pause};
use chrono::Local;
use eframe::egui::{self, RichText, Ui};

pub fn show(ui: &mut Ui, s: &Snapshot, health: &Health, handle: &Handle) {
    hero(ui, s, health, handle);
    ui.add_space(12.0);
    stats(ui, s);
    ui.add_space(12.0);
    checks(ui, health, handle);
    ui.add_space(12.0);
    recent_runs(ui, s);
}

fn hero(ui: &mut Ui, s: &Snapshot, health: &Health, handle: &Handle) {
    let color = theme::level_color(health.overall);
    egui::Frame::new()
        .fill(theme::tint(color, 22))
        .stroke(egui::Stroke::new(1.0, theme::tint(color, 120)))
        .corner_radius(egui::CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                level_dot(ui, health.overall, 11.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&health.headline).size(26.0).strong());
                    if !health.subline.is_empty() {
                        ui.label(RichText::new(&health.subline).color(MUTED));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| hero_buttons(ui, s, handle));
            });
        });
}

fn hero_buttons(ui: &mut Ui, s: &Snapshot, handle: &Handle) {
    if ui.button("⟳").on_hover_text("Alles neu prüfen").clicked() {
        handle.send(Action::RefreshAll);
    }
    let paused = s.state.as_ref().map(|st| st.pause(Local::now()) != Pause::None).unwrap_or(false);
    if paused {
        if primary(ui, "▶  Fortsetzen") {
            handle.send(Action::Resume);
        }
    } else if secondary(ui, "⏸  Pausieren") {
        handle.send(Action::Pause);
    }
    if !s.loop_running && secondary(ui, "🚀  PO jetzt starten") {
        handle.send(Action::RunOwner);
    }
}

fn stats(ui: &mut Ui, s: &Snapshot) {
    let now = Local::now();
    let today = s.state.as_ref().map(|st| st.today(now)).unwrap_or_default();
    let cap = s.config.as_ref().ok().and_then(|c| c.int("maxTasksPerDay")).map(|c| format!(" / {c}")).unwrap_or_default();
    let last_owner = s.state.as_ref().ok().and_then(|st| st.last_owner_run()).map(|t| relative(t, now)).unwrap_or_else(|| "—".into());
    let next = s.task.as_ref().and_then(|t| t.as_ref().ok()).and_then(|t| t.next_run);
    let created = s.passes.iter().filter(|p| p.file.started.is_some_and(|t| now - t < chrono::Duration::hours(48))).flat_map(|p| &p.pass.actions).filter(|a| a.kind == crate::core::activity::ActionKind::IssueCreated).count();
    ui.columns(4, |cols| {
        stat(&mut cols[0], "Worker-Tasks heute", &format!("{}{cap}", today.tasks), "");
        stat(&mut cols[1], "Issues vom PO (48 h)", &created.to_string(), &format!("{} PO-Läufe heute", today.owner_runs));
        stat(&mut cols[2], "Letzter PO-Lauf", &last_owner, "");
        stat(&mut cols[3], "Nächster Lauf", &next.map(|n| n.format("%H:%M").to_string()).unwrap_or_else(|| "—".into()), &next.map(|n| relative(n, now)).unwrap_or_default());
    });
}

fn checks(ui: &mut Ui, health: &Health, handle: &Handle) {
    card(ui, Some("Gesundheit"), |ui| {
        for (i, c) in health.checks.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            ui.horizontal(|ui| {
                level_dot(ui, c.level, 5.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&c.title).strong());
                    ui.add(egui::Label::new(RichText::new(&c.detail).color(MUTED).size(13.0)).wrap());
                });
                if let Some((label, fix)) = &c.fix {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let clicked = if c.level == Level::Error { primary(ui, label) } else { secondary(ui, label) };
                        if clicked {
                            handle.send(fix_action(fix));
                        }
                    });
                }
            });
        }
    });
}

fn fix_action(fix: &Fix) -> Action {
    match fix {
        Fix::Resume => Action::Resume,
        Fix::EnableConfig => Action::EnableConfig,
        Fix::EnableTask => Action::SetTaskEnabled(true),
        Fix::RescueWip => Action::RescueWip,
        Fix::RunOwner => Action::RunOwner,
    }
}

fn recent_runs(ui: &mut Ui, s: &Snapshot) {
    card(ui, Some("Letzte Läufe"), |ui| {
        let runs = s.state.as_ref().map(|st| st.recent_runs(12)).unwrap_or_default();
        if runs.is_empty() {
            widgets::muted(ui, "Noch keine Läufe protokolliert.");
            return;
        }
        let now = Local::now();
        egui::Grid::new("runs").num_columns(5).striped(true).spacing([18.0, 6.0]).show(ui, |ui| {
            for h in ["Zeit", "Art", "Issue", "Ergebnis", "Notiz"] {
                ui.label(RichText::new(h).color(MUTED).size(12.0));
            }
            ui.end_row();
            for r in runs {
                let when = parse_time(&r.timestamp).map(|t| format!("{} · {}", t.format("%d.%m. %H:%M"), relative(t, now))).unwrap_or(r.timestamp.clone());
                ui.label(when);
                ui.label(if r.kind == "owner" { "Product Owner" } else { "Worker" });
                ui.label(r.issue.map(|i| format!("#{i}")).unwrap_or_else(|| "—".into()));
                if r.exit_code == 0 {
                    pill(ui, &format!("ok · {:.0} min", r.duration_sec / 60.0), theme::OK);
                } else {
                    pill(ui, &format!("Exit {}", r.exit_code), theme::ERROR);
                }
                ui.label(RichText::new(r.note.unwrap_or_default()).color(MUTED));
                ui.end_row();
            }
        });
    });
}
