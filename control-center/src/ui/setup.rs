//! Setup assistant: what this machine still needs, with one button per gap — tools,
//! logins, team.toml, autostart. Never asks for tokens.

use super::theme::{self, MUTED};
use super::widgets::{card, dot, muted, page_title, primary, secondary, toggle};
use crate::collector::{Action, Handle};
use crate::core::setup::{ready, SetupAction};
use crate::core::snapshot::Snapshot;
use crate::sys;
use eframe::egui::{self, RichText, Ui};

pub fn show(ui: &mut Ui, s: &Snapshot, handle: &Handle) {
    page_title(ui, "Einrichtung", "Was dieser Rechner für den Team-Agent braucht. Anmeldungen laufen über deine eigenen Logins — es werden keine Schlüssel abgefragt.");
    let Some(checks) = &s.setup else {
        muted(ui, "Wird geprüft …");
        return;
    };
    card(ui, Some("Voraussetzungen"), |ui| {
        for (i, c) in checks.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            ui.horizontal(|ui| {
                let color = if c.ok { theme::OK } else if c.optional { theme::WARN } else { theme::ERROR };
                dot(ui, color, 5.0);
                ui.vertical(|ui| {
                    let optional = if c.optional { " (optional)" } else { "" };
                    ui.label(RichText::new(format!("{}{optional}", c.title)).strong());
                    ui.add(egui::Label::new(RichText::new(&c.detail).color(MUTED).size(13.0)).wrap());
                });
                if let (false, Some((label, action))) = (c.ok, &c.action) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let clicked = if c.optional { secondary(ui, label) } else { primary(ui, label) };
                        if clicked {
                            run(action, handle);
                        }
                    });
                }
            });
        }
    });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if secondary(ui, "⟳ Erneut prüfen") {
            handle.send(Action::RefreshAll);
        }
        muted(ui, "Nach einer Anmeldung im Terminal hier erneut prüfen.");
    });
    ui.add_space(12.0);
    finish(ui, s, handle, ready(checks));
}

fn run(action: &SetupAction, handle: &Handle) {
    match action {
        SetupAction::Terminal(cmd) => handle.send(Action::Terminal(cmd)),
        SetupAction::Open(url) => sys::open(url),
        SetupAction::CreateConfig => handle.send(Action::CreateConfig),
    }
}

fn finish(ui: &mut Ui, s: &Snapshot, handle: &Handle, ready: bool) {
    card(ui, Some("Team-Agent"), |ui| {
        if !ready {
            muted(ui, "Sobald alle Voraussetzungen erfüllt sind, kann der Team-Agent starten.");
            return;
        }
        let service = s.service.as_ref().and_then(|r| r.as_ref().ok());
        ui.horizontal(|ui| {
            let mut on = service.is_some_and(|i| i.autostart);
            if ui.add_enabled(s.agent_exe.is_some() && service.is_some(), toggle(&mut on)).changed() {
                handle.send(Action::SetAgentAutostart(on));
            }
            ui.label("Team-Agent beim Anmelden starten (empfohlen)");
        });
        ui.horizontal(|ui| {
            let mut on = s.autostart;
            if ui.add(toggle(&mut on)).changed() {
                handle.send(Action::SetAutostart(on));
            }
            ui.label("Control Center beim Anmelden im Tray starten");
        });
        ui.add_space(6.0);
        if s.agent_running {
            ui.label(RichText::new("✓ Der Team-Agent läuft.").color(theme::OK));
        } else if s.agent_exe.is_none() {
            ui.label(RichText::new("lunima-team-agent wurde nicht gefunden — bitte neu installieren.").color(theme::ERROR));
        } else if primary(ui, "▶  Team-Agent jetzt starten") {
            handle.send(Action::Agent(crate::sys::agent_service::Verb::Start));
        }
    });
}
