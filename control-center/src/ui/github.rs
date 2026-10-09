//! GitHub: newest issues (flagging ones the workers will never pick up) and open PRs.

use super::theme::{self, MUTED};
use super::widgets::{card, muted, page_title, pill};
use crate::core::snapshot::{relative, Snapshot};
use crate::core::state::parse_time;
use crate::sys::github::{GithubData, Label};
use chrono::{Duration, Local};
use eframe::egui::{self, RichText, Ui};

pub fn show(ui: &mut Ui, s: &Snapshot) {
    let repo = s.config_str("githubRepo");
    page_title(ui, "GitHub", &format!("Live aus {repo} über die GitHub CLI · alle 90 s aktualisiert"));
    match &s.github {
        None => muted(ui, "Wird geladen …"),
        Some(Err(e)) => {
            ui.label(RichText::new(format!("GitHub nicht erreichbar: {e}")).color(theme::ERROR));
        }
        Some(Ok(data)) => {
            let task_label = s.config_str("taskLabel");
            let integration = s.config_str("integrationBranch");
            summary(ui, data, &task_label);
            ui.add_space(12.0);
            issues(ui, data, &task_label);
            ui.add_space(12.0);
            prs(ui, data, &integration);
        }
    }
}

fn is_recent(created_at: &str, hours: i64) -> bool {
    parse_time(created_at).is_some_and(|t| Local::now() - t < Duration::hours(hours))
}

fn summary(ui: &mut Ui, data: &GithubData, task_label: &str) {
    let recent: Vec<_> = data.recent_issues.iter().filter(|i| is_recent(&i.created_at, 48)).collect();
    let unlabeled = recent.iter().filter(|i| i.state == "OPEN" && !i.has_label(task_label)).count();
    let open_tasks = data.recent_issues.iter().filter(|i| i.state == "OPEN" && i.has_label(task_label)).count();
    ui.columns(3, |c| {
        super::widgets::stat(&mut c[0], "Neue Issues (48 h)", &recent.len().to_string(), "");
        super::widgets::stat(&mut c[1], &format!("Offen mit '{task_label}'"), &open_tasks.to_string(), "warten auf Worker");
        super::widgets::stat(&mut c[2], &format!("Neu ohne '{task_label}'"), &unlabeled.to_string(), if unlabeled > 0 { "werden nie bearbeitet" } else { "" });
    });
}

fn labels(ui: &mut Ui, labels: &[Label]) {
    for l in labels {
        pill(ui, &l.name, theme::MUTED);
    }
}

/// One row: "#123  Title (truncated, clickable) …  pills  age".
fn row(ui: &mut Ui, number: u64, title: &str, url: &str, age: &str, pills: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([52.0, 20.0], egui::Label::new(RichText::new(format!("#{number}")).color(MUTED)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_sized([64.0, 20.0], egui::Label::new(RichText::new(age).color(MUTED).size(12.0)));
            pills(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let link = ui.add(egui::Label::new(RichText::new(title).color(theme::ACCENT)).truncate().sense(egui::Sense::click()));
                if link.on_hover_text(title).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    crate::sys::open(url);
                }
            });
        });
    });
}

fn issues(ui: &mut Ui, data: &GithubData, task_label: &str) {
    card(ui, Some("Neueste Issues"), |ui| {
        let now = Local::now();
        for i in &data.recent_issues {
            let age = parse_time(&i.created_at).map(|t| relative(t, now)).unwrap_or_default();
            row(ui, i.number, &i.title, &i.url, &age, |ui| {
                labels(ui, &i.labels);
                if i.state != "OPEN" {
                    pill(ui, "geschlossen", theme::MUTED);
                } else if !i.has_label(task_label) && is_recent(&i.created_at, 48) {
                    pill(ui, &format!("kein {task_label}"), theme::WARN);
                }
            });
        }
    });
}

fn prs(ui: &mut Ui, data: &GithubData, integration: &str) {
    card(ui, Some("Offene Pull Requests"), |ui| {
        if data.open_prs.is_empty() {
            muted(ui, "Keine offenen PRs.");
            return;
        }
        let now = Local::now();
        for p in &data.open_prs {
            let age = parse_time(&p.created_at).map(|t| relative(t, now)).unwrap_or_default();
            row(ui, p.number, &p.title, &p.url, &age, |ui| {
                labels(ui, &p.labels);
                if p.is_draft {
                    pill(ui, "Entwurf", theme::MUTED);
                }
                let color = if p.base_ref_name == integration { theme::OK } else { theme::WARN };
                pill(ui, &format!("→ {}", p.base_ref_name), color);
            });
        }
    });
}
