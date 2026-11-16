//! Team: every role of the pipeline at a glance — what it does right now, its on/off
//! switch and a chat in which Max talks to it like to a team member — plus the controls
//! of the team agent process itself. The roles answer from the team agent (chat worker)
//! while they keep working; they can change their own settings and memory on request.

use super::theme::{self, MUTED};
use super::widgets::{card, level_dot, muted, page_title, pill, primary, secondary, toggle};
use crate::collector::{Action, Handle};
use crate::core::health::Level;
use crate::core::snapshot::{relative, ChatView, Snapshot};
use crate::core::team::{role_status, ROLES};
use crate::sys::agent_service::Verb;
use chrono::Local;
use eframe::egui::{self, CornerRadius, Margin, RichText, Ui};
use std::collections::{BTreeMap, BTreeSet};
use team_core::chat::{Kind, Message, MAX};
use team_core::config::TeamConfig;
use team_core::state::parse_time;

#[derive(Default)]
pub struct TeamPage {
    /// role → unsent text
    drafts: BTreeMap<&'static str, String>,
    /// roles whose chat is shown tall
    tall: BTreeSet<&'static str>,
    /// switch positions clicked but not yet in team.toml: shown at once instead of the
    /// switch jumping back until the save has gone through
    wanted: BTreeMap<&'static str, (bool, std::time::Instant)>,
}

/// After this the file wins again (a save that failed shows its error as a toast).
const WANTED_FOR: std::time::Duration = std::time::Duration::from_secs(15);

const CHAT_HEIGHT: f32 = 230.0;
const CHAT_HEIGHT_TALL: f32 = 620.0;

/// Card frame shared by all roles; `right` fills the right end of the title row.
fn role_card(ui: &mut Ui, level: Level, name: &str, place: &str, status: &str, right: impl FnOnce(&mut Ui), body: impl FnOnce(&mut Ui)) {
    card(ui, None, |ui| {
        ui.horizontal(|ui| {
            level_dot(ui, level, 6.0);
            ui.label(RichText::new(name).size(17.0).strong());
            ui.label(RichText::new(place).color(MUTED).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
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
        page_title(ui, "Team", "Wer arbeitet woran — schalte Rollen ein und aus und sprich mit jedem im Chat. Sie antworten, während sie weiterarbeiten.");
        pipeline(ui);
        ui.add_space(12.0);
        agent_card(ui, s, handle);
        ui.add_space(12.0);
        let default = TeamConfig::default();
        let cfg = s.cfg().unwrap_or(&default);
        ui.columns(2, |cols| {
            for (i, (key, name)) in ROLES.iter().enumerate() {
                let ui = &mut cols[i % 2];
                self.role_card_for(ui, s, handle, key, name, cfg);
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
    role_card(ui, level, "Team-Agent", place, &status, |_| {}, |ui| {
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

impl TeamPage {
    fn role_card_for(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle, role: &'static str, name: &str, cfg: &TeamConfig) {
        let (level, status) = role_status(s, role, Local::now());
        let saved = crate::core::team::role_enabled(cfg, role);
        if self.wanted.get(role).is_some_and(|(on, at)| *on == saved || at.elapsed() > WANTED_FOR || handle.failed_since(*at)) {
            self.wanted.remove(role);
        }
        let enabled = self.wanted.get(role).map_or(saved, |(on, _)| *on);
        let mut clicked = None;
        let paused = s.pauses.contains_key(role);
        let empty = ChatView::default();
        let chat = s.chats.get(role).unwrap_or(&empty);
        let right = |ui: &mut Ui| {
            let mut on = enabled;
            if ui.add_enabled(s.cfg().is_some(), toggle(&mut on)).on_hover_text(if enabled { "Rolle ausschalten" } else { "Rolle einschalten" }).changed() {
                handle.send(Action::SetSetting(format!("roles.{}", role.replace('-', "_")), on.to_string()));
                clicked = Some(on);
            }
            ui.label(RichText::new(if enabled { "an" } else { "aus" }).color(MUTED).size(12.5));
            if enabled && s.agent_running {
                if paused {
                    if ui.small_button("▶ Fortsetzen").clicked() {
                        handle.send(Action::SetRolePaused(role.to_string(), false));
                    }
                } else if ui.small_button("⏸").on_hover_text("Pausieren: laufende Arbeit wird noch beendet").clicked() {
                    handle.send(Action::SetRolePaused(role.to_string(), true));
                }
            }
            if chat.unread > 0 {
                pill(ui, &format!("{} Frage{} an dich", chat.unread, if chat.unread == 1 { "" } else { "n" }), theme::WARN);
            }
        };
        let status = if self.wanted.contains_key(role) { if enabled { "wird eingeschaltet …".to_string() } else { "wird ausgeschaltet …".to_string() } } else { status };
        role_card(ui, level, name, "", &status, right, |ui| {
            egui::CollapsingHeader::new(RichText::new("Details").size(12.5).color(MUTED)).id_salt(("details", role)).show(ui, |ui| details(ui, s, role, cfg));
            ui.add_space(4.0);
            self.chat(ui, s, handle, role, name, chat);
        });
        if let Some(on) = clicked {
            self.wanted.insert(role, (on, std::time::Instant::now()));
        }
    }

    fn chat(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle, role: &'static str, name: &str, chat: &ChatView) {
        let tall = self.tall.contains(role);
        let height = if tall { CHAT_HEIGHT_TALL } else { CHAT_HEIGHT };
        egui::Frame::new().fill(theme::BG).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt(("chat", role)).max_height(height).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                if chat.messages.is_empty() {
                    muted(ui, "Noch kein Gespräch. Frag zum Beispiel: „Woran arbeitest du gerade und warum?“ oder sag „Merk dir: …“");
                }
                for m in &chat.messages {
                    bubble(ui, m, role);
                }
                if chat.busy {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        muted(ui, format!("{name} schreibt …"));
                    });
                }
            });
        });
        ui.add_space(4.0);
        let mut send = false;
        let mut resize = false;
        {
            let draft = self.drafts.entry(role).or_default();
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 90.0).max(120.0);
                let edit = egui::TextEdit::multiline(draft).id_salt(("draft", role)).hint_text(format!("Nachricht an {name} …")).desired_rows(1).desired_width(width);
                let resp = ui.add(edit).on_hover_text("Enter sendet, Umschalt+Enter macht eine neue Zeile");
                if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                    send = true;
                }
                ui.vertical(|ui| {
                    if ui.add_enabled(!draft.trim().is_empty(), egui::Button::new("Senden")).clicked() {
                        send = true;
                    }
                    resize = ui.small_button(if tall { "⤡ kleiner" } else { "⤢ größer" }).clicked();
                });
            });
            if send && !draft.trim().is_empty() {
                handle.send(Action::SendChat(role.to_string(), draft.trim().to_string()));
                draft.clear();
            }
        }
        if resize && !self.tall.remove(role) {
            self.tall.insert(role);
        }
        if !s.agent_running {
            muted(ui, "Der Team-Agent läuft nicht — Nachrichten werden beantwortet, sobald er läuft.");
        }
    }
}

/// One chat message: Max right-aligned, the role and its teammates left.
fn bubble(ui: &mut Ui, m: &Message, role: &str) {
    let mine = m.from == MAX;
    let (fill, who_color) = match m.kind {
        _ if mine => (theme::tint(theme::ACCENT, 55), theme::ACCENT),
        Kind::Question => (theme::tint(theme::WARN, 35), theme::WARN),
        Kind::Error => (theme::tint(theme::ERROR, 30), theme::ERROR),
        Kind::Note | Kind::Ask => (theme::tint(theme::INFO, 25), theme::INFO),
        Kind::Chat => (theme::CARD, theme::OK),
    };
    let who = match (m.from.as_str(), m.to.as_str()) {
        (MAX, _) => "Du".to_string(),
        (f, t) if f == role && t == MAX => team_core::chat::label(f).to_string(),
        (f, t) => format!("{} → {}", team_core::chat::label(f), team_core::chat::label(t)),
    };
    let tag = match m.kind {
        Kind::Question => " · Frage",
        Kind::Note => " · Notiz",
        Kind::Ask => " · Rückfrage",
        _ => "",
    };
    let time = m.time().map(|t| t.format("%d.%m. %H:%M").to_string()).unwrap_or_default();
    let layout = if mine { egui::Layout::right_to_left(egui::Align::TOP) } else { egui::Layout::left_to_right(egui::Align::TOP) };
    ui.with_layout(layout, |ui| {
        let max = ui.available_width() * 0.86;
        egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(9)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_max_width(max);
            ui.vertical(|ui| {
                ui.label(RichText::new(format!("{who}{tag} · {time}")).size(11.0).color(who_color));
                ui.add(egui::Label::new(RichText::new(&m.text).color(theme::TEXT)).wrap());
            });
        });
    });
    ui.add_space(4.0);
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
        "ux-review" => {
            let ux = &cfg.ux_review;
            model_row(ui, "Modell:", &ux.model);
            model_row(ui, "Zeitfenster:", &format!("ab {:02}:00 für {} Std., {} Abläufe pro Nacht", ux.hour, team_core::config::UxReview::WINDOW_HOURS, ux.goals_per_run));
            muted(ui, "Simulierte Nutzer (Personas) bedienen die Desktop-App; Befunde werden ux-finding-Issues ohne Screenshots, der Bericht geht an den PO.");
            if let Some(st) = &s.ux {
                let now = Local::now();
                let last = st.last_attempt.as_deref().and_then(parse_time).map(|t| relative(t, now)).unwrap_or_else(|| "—".into());
                let issues = if st.filed_issues.is_empty() { String::new() } else { format!(" · Issues {}", st.filed_issues.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ")) };
                muted(ui, format!("letzter Lauf {last} ({}){issues}", if st.last_result.is_empty() { "—" } else { &st.last_result }));
                if !st.last_note.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&st.last_note).color(MUTED).size(12.5)).wrap());
                }
            }
        }
        "product-discovery" => {
            let pd = &cfg.product_discovery;
            model_row(ui, "Modell:", if pd.model.is_empty() { &m.product_owner } else { &pd.model });
            model_row(ui, "Zeitfenster:", &format!("wöchentlich {} ab {:02}:00, Budget {:.0} USD (nominell)", pd.weekday, pd.hour, pd.weekly_budget_usd));
            let d = &s.discovery;
            let last = d.state.as_ref().map(|st| format!("Woche {} · {}", st.week, crate::core::discovery::result_label(&st.last_result))).unwrap_or_else(|| "Noch kein Discovery-Lauf".into());
            muted(ui, format!("{last} — Details auf der Seite Discovery"));
        }
        "qa" => {
            model_row(ui, "Review-Modell:", &m.reviewer);
            muted(ui, "Baut und testet jeden Agent-PR; optional LLM-Review (.agent.toml).");
        }
        _ => muted(ui, format!("Setzt Kommentare mit „{} …“ auf PRs um.", cfg.pr_feedback_marker)),
    }
}
