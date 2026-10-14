//! Team: every role of the pipeline — what it does right now, which model, its pause
//! switch — plus the controls of the team agent process itself.

use super::theme::{self, MUTED};
use super::widgets::{card, level_dot, muted, page_title, pill, primary, secondary, toggle};
use crate::collector::{Action, Handle};
use crate::core::health::Level;
use crate::core::snapshot::{relative, Snapshot};
use crate::core::team::{role_status, ROLES};
use crate::sys::agent_service::Verb;
use chrono::Local;
use eframe::egui::{self, RichText, Ui};
use team_core::config::TeamConfig;
use team_core::state::parse_time;

#[derive(Default)]
pub struct TeamPage;

/// Card frame shared by all roles.
fn role_card(ui: &mut Ui, level: Level, name: &str, place: &str, status: &str, body: impl FnOnce(&mut Ui)) {
    card(ui, None, |ui| {
        ui.horizontal(|ui| {
            level_dot(ui, level, 6.0);
            ui.label(RichText::new(name).size(17.0).strong());
            ui.label(RichText::new(place).color(MUTED).size(12.0));
        });
        ui.add(egui::Label::new(RichText::new(status).color(theme::level_color(level))).wrap());
        ui.add_space(4.0);
        body(ui);
    });
}

fn model_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(MUTED).size(12.5));
        ui.label(RichText::new(if value.is_empty() { "CLI-Standard" } else { value }).monospace().size(12.5));
    });
}

impl TeamPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Team", "Wer arbeitet woran — alle Rollen laufen im Team-Agent auf diesem Rechner (Claude-Abo, gh-Login).");
        pipeline(ui);
        ui.add_space(12.0);
        agent_card(ui, s, handle);
        ui.add_space(12.0);
        let default = TeamConfig::default();
        let cfg = s.cfg().unwrap_or(&default);
        ui.columns(2, |cols| {
            for (i, (key, name)) in ROLES.iter().enumerate() {
                let ui = &mut cols[i % 2];
                role_card_for(ui, s, handle, key, name, cfg);
                ui.add_space(10.0);
            }
        });
    }
}

fn pipeline(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        for (i, (step, color)) in [("Product Owner plant", theme::ACCENT), ("Coder baut (+ Reviewer, Test-Gate)", theme::INFO), ("QA-Tester prüft", theme::WARN), ("PO reviewt & merged", theme::OK)].into_iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("›").size(18.0).color(MUTED));
            }
            pill(ui, step, color);
        }
    });
}

fn agent_card(ui: &mut Ui, s: &Snapshot, handle: &Handle) {
    let service = s.service.as_ref().and_then(|r| r.as_ref().ok());
    let (level, status) = match (&s.agent_exe, s.agent_running) {
        (None, _) => (Level::Error, "nicht installiert — lunima-team-agent fehlt neben dieser App".to_string()),
        (Some(_), true) => (Level::Ok, "läuft".to_string()),
        (Some(_), false) => (Level::Error, "gestoppt".to_string()),
    };
    let place = if cfg!(windows) { "Aufgabe LunimaTeamAgent" } else { "systemd: lunima-team-agent" };
    role_card(ui, level, "Team-Agent", place, &status, |ui| {
        ui.horizontal(|ui| {
            let can = s.agent_exe.is_some() && s.config.is_ok();
            ui.add_enabled_ui(can, |ui| {
                if s.agent_running {
                    if secondary(ui, "Neu starten") {
                        handle.send(Action::Agent(Verb::Restart));
                    }
                    if secondary(ui, "Stoppen") {
                        handle.send(Action::Agent(Verb::Stop));
                    }
                } else if primary(ui, "Starten") {
                    handle.send(Action::Agent(Verb::Start));
                }
            });
            ui.add_space(16.0);
            let mut on = service.is_some_and(|i| i.autostart);
            if ui.add_enabled(s.agent_exe.is_some() && service.is_some(), toggle(&mut on)).changed() {
                handle.send(Action::SetAgentAutostart(on));
            }
            ui.label("Agent beim Anmelden starten");
        });
        if let Some(exe) = &s.agent_exe {
            muted(ui, exe.display().to_string());
        }
    });
}

fn role_card_for(ui: &mut Ui, s: &Snapshot, handle: &Handle, role: &'static str, name: &str, cfg: &TeamConfig) {
    let (level, status) = role_status(s, role, Local::now());
    role_card(ui, level, name, "", &status, |ui| {
        details(ui, s, role, cfg);
        let paused = s.pauses.contains_key(role);
        ui.horizontal(|ui| {
            if paused {
                if primary(ui, "Fortsetzen") {
                    handle.send(Action::SetRolePaused(role.to_string(), false));
                }
            } else if s.agent_running && secondary(ui, "Pausieren") {
                handle.send(Action::SetRolePaused(role.to_string(), true));
            }
        });
    });
}

fn details(ui: &mut Ui, s: &Snapshot, role: &str, cfg: &TeamConfig) {
    let m = &cfg.models;
    match role {
        "product-owner" => {
            model_row(ui, "Modell:", &m.product_owner);
            let repo = if cfg.product_owner.repo.is_empty() { "—".to_string() } else { cfg.product_owner.repo.clone() };
            model_row(ui, "Repo:", &format!("{repo} → {}", cfg.product_owner.integration_branch));
            if let Some(po) = &s.po {
                let now = Local::now();
                let last = po.last_start.as_deref().and_then(parse_time).map(|t| relative(t, now)).unwrap_or_else(|| "—".into());
                let next = po.next_due.as_deref().and_then(parse_time).map(|t| relative(t, now)).unwrap_or_else(|| "—".into());
                muted(ui, format!("letzter Lauf {last} ({}) · nächster {next} · heute {}", if po.last_result.is_empty() { "—" } else { &po.last_result }, po.runs_today));
                if !po.last_note.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&po.last_note).color(MUTED).size(12.5)).wrap());
                }
            }
        }
        "coder" => {
            model_row(ui, "Standard:", &m.coder);
            model_row(ui, "complex / claudeapi:", &m.premium);
            model_row(ui, "Reviewer:", &m.reviewer);
            let kimi: Vec<&str> = cfg.repos.iter().filter(|r| r.routing != team_core::config::Routing::Claude).map(|r| r.name.as_str()).collect();
            if !kimi.is_empty() {
                model_row(ui, &format!("Kimi für {}:", kimi.join(", ")), &cfg.providers.openrouter.model);
            }
        }
        "qa" => {
            model_row(ui, "Review-Modell:", &m.reviewer);
            muted(ui, "Baut und testet jeden Agent-PR; optional LLM-Review (.agent.toml).");
        }
        _ => muted(ui, format!("Setzt Kommentare mit „{} …“ auf PRs um.", cfg.pr_feedback_marker)),
    }
}
