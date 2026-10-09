//! Team: every role of the pipeline — what it does right now, which model, and its controls.

use super::theme::{self, MUTED};
use super::widgets::{card, level_dot, muted, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::health::Level;
use crate::core::issue_agent::Heartbeat;
use crate::core::snapshot::{relative, Snapshot};
use crate::core::state::Pause;
use crate::core::team::TeamSnapshot;
use chrono::Local;
use eframe::egui::{self, RichText, Ui};

#[derive(Default)]
pub struct TeamPage {
    dir_input: String,
}

/// Status line of an issue-agent role: (level, text).
fn role_status(team: &TeamSnapshot, role: &str) -> (Level, String) {
    let now = Local::now();
    let unit = match &team.units {
        Some(Err(e)) => return (Level::Warn, format!("WSL nicht erreichbar: {e}")),
        Some(Ok(u)) => u.get(role).cloned(),
        None => None,
    };
    match unit.as_deref() {
        None => return (Level::Pending, "wird geprüft …".into()),
        Some("active") => {}
        Some(other) => return (Level::Error, format!("gestoppt ({other})")),
    }
    if let Some(reason) = team.pauses.get(role) {
        return (Level::Warn, format!("pausiert — {reason}"));
    }
    match team.heartbeats.get(role) {
        Some(hb) if hb.state == "working" => (Level::Info, working_text(hb, now)),
        Some(hb) => (Level::Ok, format!("wartet auf Arbeit · Lebenszeichen {}", hb.updated.map(|u| relative(u, now)).unwrap_or_default())),
        None => (Level::Ok, "läuft (noch kein Status)".into()),
    }
}

fn working_text(hb: &Heartbeat, now: chrono::DateTime<Local>) -> String {
    let what = match (hb.detail.get("issue"), hb.detail.get("pr")) {
        (Some(i), _) => format!("Issue #{i}"),
        (_, Some(p)) => format!("PR #{p}"),
        _ => "eine Aufgabe".into(),
    };
    let title = hb.detail.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(|t| format!(" „{t}“")).unwrap_or_default();
    let since = hb.since.map(|s| format!(" · seit {}", relative(s, now).trim_start_matches("vor ").to_string())).unwrap_or_default();
    format!("arbeitet an {what}{title}{since}")
}

fn env_value(team: &TeamSnapshot, key: &str) -> String {
    team.env.as_ref().map(|e| e.display(key)).unwrap_or_else(|| "—".into())
}

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
        ui.label(RichText::new(value).monospace().size(12.5));
    });
}

impl TeamPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Team", "Wer arbeitet woran — Product Owner unter Windows, Coder, QA und PR-Feedback im Issue-Agent (WSL).");
        pipeline(ui);
        ui.add_space(12.0);
        if s.team.dir.is_none() {
            self.connect(ui, handle);
            ui.add_space(12.0);
        }
        ui.columns(2, |cols| {
            product_owner(&mut cols[0], s, handle);
            cols[0].add_space(10.0);
            agent_role(&mut cols[1], s, handle, "coder", "Coder", |ui, t| {
                model_row(ui, "Standard:", &env_value(t, "AGENT_CODER_MODEL"));
                model_row(ui, "complex / claudeapi:", &env_value(t, "AGENT_PREMIUM_CODER_MODEL"));
                let kimi = t.env.as_ref().map(|e| e.list("AGENT_OPENROUTER_FORCE_REPOS").join(", ")).unwrap_or_default();
                if !kimi.is_empty() {
                    model_row(ui, &format!("Kimi für {kimi}:"), &env_value(t, "AGENT_OPENROUTER_MODEL"));
                }
            });
            cols[1].add_space(10.0);
            agent_role(&mut cols[0], s, handle, "qa", "QA-Tester", |ui, t| {
                model_row(ui, "Review-Modell:", &env_value(t, "AGENT_REVIEWER_MODEL"));
                muted(ui, "Baut und testet jeden Agent-PR; optional LLM-Review (.agent.toml).");
            });
            cols[0].add_space(10.0);
            agent_role(&mut cols[1], s, handle, "pr-feedback", "PR-Feedback", |ui, t| {
                let marker = t.env.as_ref().and_then(|e| e.get("AGENT_PR_FEEDBACK_MARKER")).unwrap_or_else(|| "@agent".into());
                muted(ui, format!("Setzt Kommentare mit „{marker} …“ auf PRs um."));
            });
            cols[1].add_space(10.0);
            reviewer(&mut cols[0], s);
            cols[0].add_space(10.0);
            loop_toggles(&mut cols[1], s, handle);
        });
        ui.add_space(10.0);
        footer(ui, s);
    }

    fn connect(&mut self, ui: &mut Ui, handle: &Handle) {
        card(ui, Some("Issue-Agent verbinden"), |ui| {
            muted(ui, "Ordner von autonomous-issue-agent (mit main.py), z. B. C:\\Users\\…\\autonomous-issue-agent");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.dir_input).desired_width(420.0));
                if primary(ui, "Verbinden") && !self.dir_input.trim().is_empty() {
                    handle.send(Action::SetAgentDir(self.dir_input.trim().into()));
                }
            });
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

fn product_owner(ui: &mut Ui, s: &Snapshot, handle: &Handle) {
    let paused = s.state.as_ref().map(|st| st.pause(Local::now()) != Pause::None).unwrap_or(false);
    let last = s.state.as_ref().ok().and_then(|st| st.last_owner_run()).map(|t| relative(t, Local::now())).unwrap_or_else(|| "—".into());
    let (level, status) = if paused { (Level::Warn, "pausiert".to_string()) } else if s.loop_running { (Level::Info, "arbeitet".to_string()) } else { (Level::Ok, format!("bereit · letzter Lauf {last}")) };
    role_card(ui, level, "Product Owner", "Windows · stündlich", &status, |ui| {
        model_row(ui, "Modell:", &format!("{} ({})", s.config_str("ownerModel"), s.config_str("ownerRunner")));
        ui.horizontal(|ui| {
            if paused {
                if primary(ui, "Fortsetzen") {
                    handle.send(Action::Resume);
                }
            } else if secondary(ui, "Pausieren") {
                handle.send(Action::Pause);
            }
            if !s.loop_running && secondary(ui, "Jetzt starten") {
                handle.send(Action::RunOwner);
            }
        });
    });
}

fn agent_role(ui: &mut Ui, s: &Snapshot, handle: &Handle, role: &str, name: &str, details: impl FnOnce(&mut Ui, &TeamSnapshot)) {
    let (level, status) = role_status(&s.team, role);
    let unit = crate::sys::wsl::unit_of(role).unwrap_or_default();
    role_card(ui, level, name, &format!("WSL · {unit}"), &status, |ui| {
        details(ui, &s.team);
        if s.team.dir.is_none() {
            return;
        }
        let paused = s.team.pauses.contains_key(role);
        let running = level != Level::Error && level != Level::Pending;
        ui.horizontal(|ui| {
            if paused {
                if primary(ui, "Fortsetzen") {
                    handle.send(Action::SetRolePaused(role.to_string(), false));
                }
            } else if running && secondary(ui, "Pausieren") {
                handle.send(Action::SetRolePaused(role.to_string(), true));
            }
            if running {
                if secondary(ui, "Neu starten") {
                    handle.send(Action::Unit(role.to_string(), "restart"));
                }
                if secondary(ui, "Stoppen") {
                    handle.send(Action::Unit(role.to_string(), "stop"));
                }
            } else if level == Level::Error && primary(ui, "Starten") {
                handle.send(Action::Unit(role.to_string(), "start"));
            }
        });
    });
}

fn reviewer(ui: &mut Ui, s: &Snapshot) {
    role_card(ui, Level::Ok, "Reviewer & Test-Gate", "läuft im Coder", "prüft jeden PR des Coders, bevor er geöffnet wird", |ui| {
        model_row(ui, "Review:", &env_value(&s.team, "AGENT_REVIEWER_MODEL"));
        model_row(ui, "Label critical:", &env_value(&s.team, "AGENT_REVIEWER_MODEL_CRITICAL"));
    });
}

fn loop_toggles(ui: &mut Ui, s: &Snapshot, handle: &Handle) {
    let workers = s.config.as_ref().is_ok_and(|c| c.bool("workersEnabled"));
    let customer = s.config.as_ref().is_ok_and(|c| c.bool("customerEnabled"));
    let status = if workers { "Loop-Worker aktiv (codet selbst)" } else { "Team-Modus: das Coden macht der Issue-Agent" };
    role_card(ui, if workers { Level::Info } else { Level::Ok }, "PO-Loop: Worker & Kunde", "Windows", status, |ui| {
        let mut w = workers;
        if ui.checkbox(&mut w, format!("Eigene Worker ({})", s.config_str("workerModel"))).changed() {
            handle.send(Action::SetLoopFlag("workersEnabled", w));
        }
        let mut c = customer;
        if ui.checkbox(&mut c, format!("Kunden-Review / UX-Tester ({})", s.config_str("customerModel"))).changed() {
            handle.send(Action::SetLoopFlag("customerEnabled", c));
        }
    });
}

fn footer(ui: &mut Ui, s: &Snapshot) {
    let t = &s.team;
    let version = match &t.claude_version {
        Some(Ok(v)) => v.clone(),
        Some(Err(e)) => format!("Fehler: {e}"),
        None => "wird geprüft …".into(),
    };
    let dir = t.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "nicht verbunden".into());
    muted(ui, format!("Issue-Agent: {dir}  ·  Claude-CLI in WSL: {version}"));
}
