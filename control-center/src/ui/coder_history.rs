//! Coder tab of the activity page: the issue agent's run history (.sessions/issue-history.jsonl).

use super::theme::{self, MUTED};
use super::widgets::{card, link, muted, pill, stat};
use crate::core::snapshot::{relative, Snapshot};
use crate::core::state::parse_time;
use chrono::{Duration, Local};
use eframe::egui::{self, RichText, Ui};

pub fn show(ui: &mut Ui, s: &Snapshot) {
    let history = &s.team.history;
    if s.team.dir.is_none() {
        muted(ui, "Issue-Agent nicht verbunden — siehe Seite Team.");
        return;
    }
    let now = Local::now();
    let week: Vec<_> = history.iter().filter(|h| parse_time(&h.timestamp).is_some_and(|t| now - t < Duration::days(7))).collect();
    let done = week.iter().filter(|h| h.completed).count();
    let cost: f64 = week.iter().filter_map(|h| h.total_cost_usd).sum();
    ui.columns(3, |c| {
        stat(&mut c[0], "Läufe (7 Tage)", &week.len().to_string(), "");
        stat(&mut c[1], "Mit PR abgeschlossen", &done.to_string(), "");
        stat(&mut c[2], "API-Kosten (7 Tage)", &format!("${cost:.2}"), "laut Claude-Abrechnung je Lauf");
    });
    ui.add_space(10.0);
    card(ui, Some("Letzte Läufe des Coders"), |ui| {
        if history.is_empty() {
            muted(ui, "Noch keine Läufe protokolliert.");
        }
        for h in history {
            ui.horizontal(|ui| {
                let when = parse_time(&h.timestamp).map(|t| format!("{} · {}", t.format("%d.%m. %H:%M"), relative(t, now))).unwrap_or_else(|| h.timestamp.clone());
                ui.add_sized([150.0, 20.0], egui::Label::new(RichText::new(when).color(MUTED).size(12.5)));
                if h.completed {
                    pill(ui, "PR erstellt", theme::OK);
                } else {
                    pill(ui, "unvollständig", theme::WARN);
                }
                ui.label(RichText::new(format!("#{}", h.number)).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let meta = [
                        h.duration_sec.map(|d| format!("{} min", d / 60)),
                        h.total_cost_usd.map(|c| format!("${c:.2}")),
                        Some(h.repository.rsplit('/').next().unwrap_or("").to_string()),
                    ];
                    ui.label(RichText::new(meta.into_iter().flatten().collect::<Vec<_>>().join(" · ")).color(MUTED).size(12.0));
                    if let Some(url) = &h.pr_url {
                        link(ui, "PR", url);
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(&h.title).truncate());
                    });
                });
            });
        }
    });
}
