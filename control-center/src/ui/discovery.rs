//! Discovery: what the weekly discovery run learned, what needs a decision, what it cost.

use super::theme::{self, MUTED};
use super::widgets::{card, level_dot, link, muted, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::discovery::{self as dc, DiscoveryView, HISTORY_WEEKS};
use crate::core::snapshot::{relative, Snapshot};
use crate::core::team::role_status;
use crate::sys;
use chrono::Local;
use eframe::egui::{self, RichText, Ui};
use team_core::state::{parse_time, DiscoveryItem};

const ROLE: &str = "product-discovery";

pub fn show(ui: &mut Ui, s: &Snapshot, handle: &Handle) {
    page_title(ui, "Discovery", "Wöchentliche Produkt-Discovery (Scout → Stratege → Kritiker): was gelernt wurde, was sich geändert hat, was du entscheiden musst, was es gekostet hat.");
    let d = &s.discovery;
    status(ui, s, d, handle);
    ui.add_space(12.0);
    if d.is_empty() {
        card(ui, None, |ui| muted(ui, "Noch kein Discovery-Lauf auf diesem Rechner."));
        return;
    }
    decisions(ui, d);
    ui.add_space(12.0);
    costs(ui, d);
    ui.add_space(12.0);
    files(ui, d);
    ui.add_space(12.0);
    report(ui, d);
}

fn status(ui: &mut Ui, s: &Snapshot, d: &DiscoveryView, handle: &Handle) {
    let now = Local::now();
    let (level, text) = role_status(s, ROLE, now);
    card(ui, None, |ui| {
        ui.horizontal(|ui| {
            level_dot(ui, level, 6.0);
            ui.add(egui::Label::new(RichText::new(text).color(theme::level_color(level))).wrap());
        });
        if let Some(st) = &d.state {
            let stage = if st.stage.is_empty() { String::new() } else { format!(" · Stufe {}", st.stage) };
            let next = st.next_due.as_deref().and_then(parse_time).map(|t| format!(" · nächster Lauf {}", relative(t, now))).unwrap_or_default();
            ui.label(format!("Woche {}{stage} · {} · Versuche {}{next}", st.week, dc::result_label(&st.last_result), st.attempts));
            if !st.last_note.is_empty() {
                ui.add(egui::Label::new(RichText::new(&st.last_note).color(MUTED).size(12.5)).wrap());
            }
            muted(ui, format!("Neue Belege {} (Duplikate verworfen {}) · Issues angelegt {}", st.evidence_added, st.duplicates_dropped, st.issues_created));
            ui.horizontal(|ui| {
                if !st.pr_url.is_empty() {
                    link(ui, "Entwurfs-PR öffnen", &st.pr_url);
                }
                if !st.report_url.is_empty() {
                    link(ui, "Wochenbericht (Issue) öffnen", &st.report_url);
                }
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // `once` would compete with a running daemon, which gets a trigger file instead
            if primary(ui, "Discovery jetzt ausführen") {
                handle.send(if s.agent_running { Action::RequestDiscoveryRun } else { Action::RunOnce(ROLE) });
            }
            if s.pauses.contains_key(ROLE) {
                if secondary(ui, "Fortsetzen") {
                    handle.send(Action::SetRolePaused(ROLE.into(), false));
                }
            } else if secondary(ui, "Pausieren") {
                handle.send(Action::SetRolePaused(ROLE.into(), true));
            }
        });
        muted(ui, "„Jetzt ausführen“ überspringt nur das Zeitfenster — Budget, Sperre und „einmal pro Woche“ gelten weiter.");
    });
}

fn items(ui: &mut Ui, id: &str, title: &str, list: &[DiscoveryItem]) {
    ui.label(RichText::new(title).strong());
    if list.is_empty() {
        muted(ui, "keine");
        return;
    }
    egui::Grid::new(id).num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
        for it in list {
            ui.label(RichText::new(&it.id).monospace());
            if it.url.is_empty() {
                ui.label(&it.title);
            } else {
                link(ui, &it.title, &it.url);
            }
            let color = match it.status.as_str() {
                "accepted" | "approved" => theme::OK,
                "rejected" | "declined" => theme::ERROR,
                "proposed" | "pending" => theme::WARN,
                _ => MUTED,
            };
            if !it.status.is_empty() {
                pill(ui, &it.status, color);
            }
            ui.end_row();
        }
    });
    ui.add_space(6.0);
}

fn decisions(ui: &mut Ui, d: &DiscoveryView) {
    let Some(st) = &d.state else { return };
    card(ui, Some("Braucht deine Entscheidung"), |ui| {
        items(ui, "approvals", "Offene Freigaben", &st.approvals);
        items(ui, "decisions", "Decision Records", &st.decisions);
        items(ui, "opportunities", "Chancen", &st.opportunities);
    });
}

fn costs(ui: &mut Ui, d: &DiscoveryView) {
    card(ui, Some("Kosten"), |ui| {
        if let Some(st) = &d.state {
            ui.label(format!("Diese Woche: {:.2} USD · {} Tokens", st.week_cost_usd, dc::fmt_tokens(st.week_tokens)));
        }
        if d.all_notional() {
            muted(ui, "nominell (Abo) — kein Geld abgebucht");
        }
        let weeks = dc::weekly_costs(&d.ledger, HISTORY_WEEKS);
        if !weeks.is_empty() {
            let total: f64 = weeks.iter().map(|w| w.cost_usd).sum();
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Letzte {} Wochen: {total:.2} USD", weeks.len())).strong());
            egui::Grid::new("weeks").num_columns(4).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                for w in &weeks {
                    ui.label(&w.week);
                    ui.label(format!("{:.2} USD", w.cost_usd));
                    ui.label(format!("{} Tokens", dc::fmt_tokens(w.tokens)));
                    ui.label(RichText::new(format!("{} Stufen", w.stages)).color(MUTED));
                    ui.end_row();
                }
            });
        }
        let stages = d.week_stages();
        if !stages.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("Stufen dieser Woche").strong());
            egui::Grid::new("stages").num_columns(6).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                for h in ["Stufe", "Modell", "Dauer", "Tokens", "Kosten", "Ergebnis"] {
                    ui.label(RichText::new(h).color(MUTED).size(12.0));
                }
                ui.end_row();
                for r in stages {
                    ui.label(&r.stage);
                    ui.label(RichText::new(&r.model).monospace());
                    ui.label(dc::fmt_duration(r.duration_sec));
                    ui.label(dc::fmt_tokens(dc::tokens(r)));
                    ui.label(format!("{:.2} USD{}", r.cost_usd, if r.billing == "subscription-notional" { " (nominell)" } else { "" }));
                    pill(ui, &r.result, if r.result == "ok" { theme::OK } else { theme::WARN });
                    ui.end_row();
                }
            });
        }
    });
}

fn files(ui: &mut Ui, d: &DiscoveryView) {
    card(ui, Some("Vorschauen & Entwürfe (lokal)"), |ui| {
        ui.horizontal_wrapped(|ui| {
            if d.previews_dir.is_dir() && secondary(ui, "Vorschau-Ordner öffnen") {
                sys::open_path(&d.previews_dir);
            }
            for (slug, index) in &d.previews {
                if secondary(ui, &format!("🌐 {slug}")) {
                    sys::open_path(index);
                }
            }
        });
        if d.previews.is_empty() {
            muted(ui, "Keine Seitenvorschauen — Vorschauen sind nie veröffentlicht.");
        }
        match &d.outreach {
            Some(path) => {
                ui.horizontal(|ui| {
                    if secondary(ui, "Ansprache-Entwürfe öffnen") {
                        sys::open_path(path);
                    }
                    muted(ui, "privat — nicht im Repository");
                });
            }
            None => muted(ui, "Keine Ansprache-Entwürfe für diese Woche."),
        }
    });
}

fn report(ui: &mut Ui, d: &DiscoveryView) {
    card(ui, Some("Letzter Bericht"), |ui| match &d.report {
        Some(text) => {
            egui::ScrollArea::vertical().id_salt("report").max_height(420.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace().size(12.5)).wrap());
            });
        }
        None => muted(ui, "Noch kein Bericht."),
    });
}
