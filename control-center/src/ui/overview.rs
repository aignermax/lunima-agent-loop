//! Overview: one glance answers "does it work, what's next, what needs me?"

use super::theme::{self, MUTED};
use super::widgets::{self, card, level_dot, pill, primary, secondary, stat};
use crate::app::Page;
use crate::collector::{Action, Handle};
use crate::core::health::{Fix, Health, Level};
use crate::core::snapshot::{relative, Snapshot};
use crate::sys::agent_service::Verb;
use chrono::{Duration, Local};
use eframe::egui::{self, RichText, Ui};
use team_core::state::parse_time;

/// Returns a page to switch to (a fix that lives elsewhere, e.g. the setup assistant).
pub fn show(ui: &mut Ui, s: &Snapshot, health: &Health, handle: &Handle) -> Option<Page> {
    hero(ui, s, health, handle);
    ui.add_space(12.0);
    stats(ui, s);
    ui.add_space(12.0);
    let target = checks(ui, health, handle);
    ui.add_space(12.0);
    recent_runs(ui, s);
    target
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
    if !s.agent_running {
        if s.config.is_ok() && s.agent_exe.is_some() && primary(ui, "▶  Team-Agent starten") {
            handle.send(Action::Agent(Verb::Start));
        }
        return;
    }
    if s.all_paused() {
        if primary(ui, "▶  Fortsetzen") {
            handle.send(Action::PauseAll(false));
        }
    } else if secondary(ui, "⏸  Alle pausieren") {
        handle.send(Action::PauseAll(true));
    }
}

fn stats(ui: &mut Ui, s: &Snapshot) {
    let now = Local::now();
    let within = |h: &crate::core::agent_files::HistoryEntry, hours: i64| parse_time(&h.timestamp).is_some_and(|t| now - t < Duration::hours(hours));
    let day = s.history.iter().filter(|h| within(h, 24)).count();
    let prs = s.history.iter().filter(|h| within(h, 24 * 7) && h.pr_url.is_some()).count();
    let po = s.po.as_ref();
    let last = po.and_then(|p| p.last_start.as_deref()).and_then(parse_time).map(|t| relative(t, now)).unwrap_or_else(|| "—".into());
    let result = po.map(|p| p.last_result.clone()).filter(|r| !r.is_empty()).unwrap_or_default();
    let next = po.and_then(|p| p.next_due.as_deref()).and_then(parse_time);
    ui.columns(4, |cols| {
        stat(&mut cols[0], "Coder-Läufe (24 h)", &day.to_string(), "");
        stat(&mut cols[1], "PRs vom Coder (7 Tage)", &prs.to_string(), "");
        stat(&mut cols[2], "Letzter PO-Lauf", &last, &result);
        stat(&mut cols[3], "Nächster PO-Lauf", &next.map(|n| n.format("%H:%M").to_string()).unwrap_or_else(|| "—".into()), &next.map(|n| relative(n, now)).unwrap_or_default());
    });
}

fn checks(ui: &mut Ui, health: &Health, handle: &Handle) -> Option<Page> {
    let mut target = None;
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
                            match fix_action(fix) {
                                Some(a) => handle.send(a),
                                None => target = Some(Page::Setup),
                            }
                        }
                    });
                }
            });
        }
    });
    target
}

/// `None` = the fix is a page (setup assistant), not an action.
fn fix_action(fix: &Fix) -> Option<Action> {
    Some(match fix {
        Fix::OpenSetup => return None,
        Fix::StartAgent => Action::Agent(Verb::Start),
        Fix::RestartAgent => Action::Agent(Verb::Restart),
        Fix::EnableAgentAutostart => Action::SetAgentAutostart(true),
        Fix::ResumeRole(role) => Action::SetRolePaused(role.clone(), false),
    })
}

fn recent_runs(ui: &mut Ui, s: &Snapshot) {
    card(ui, Some("Letzte Läufe des Coders"), |ui| {
        if s.history.is_empty() {
            widgets::muted(ui, "Noch keine Läufe protokolliert.");
            return;
        }
        let now = Local::now();
        egui::Grid::new("runs").num_columns(5).striped(true).spacing([18.0, 6.0]).show(ui, |ui| {
            for h in ["Zeit", "Repo", "Issue", "Ergebnis", "Rechner"] {
                ui.label(RichText::new(h).color(MUTED).size(12.0));
            }
            ui.end_row();
            for r in s.history.iter().take(12) {
                let when = parse_time(&r.timestamp).map(|t| format!("{} · {}", t.format("%d.%m. %H:%M"), relative(t, now))).unwrap_or(r.timestamp.clone());
                ui.label(when);
                ui.label(r.repository.rsplit('/').next().unwrap_or(""));
                ui.label(format!("#{}", r.number));
                match (&r.pr_url, r.completed) {
                    (Some(_), _) => pill(ui, &format!("PR · {} min", r.duration_sec.unwrap_or(0) / 60), theme::OK),
                    (None, true) => pill(ui, "nichts zu tun", theme::MUTED),
                    (None, false) => pill(ui, "aufgegeben", theme::ERROR),
                }
                ui.label(RichText::new(r.machine.clone().unwrap_or_default()).color(MUTED));
                ui.end_row();
            }
        });
    });
}
