//! Projects: the repositories the team works on — coder on/off, model family, roles
//! enabled by each repo's .agent.toml. Edits go to team.toml (applies without restart).

use super::theme::{self, MUTED};
use super::widgets::{card, link, muted, page_title, pill};
use crate::collector::{Action, Handle};
use crate::core::project_autosave::Autosave;
use crate::core::projects::{self, routing_label, Project, ROUTINGS};
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;
use team_core::config::RepoConfig;
use team_core::config::TeamConfig;

#[derive(Default)]
pub struct ProjectsPage {
    edits: Autosave,
    save_result: Option<Receiver<Result<Vec<RepoConfig>, String>>>,
    new_repo: String,
    error: Option<String>,
}

impl ProjectsPage {
    pub fn can_update(&self) -> bool {
        self.edits.is_idle() && self.save_result.is_none()
    }
    pub fn has_save_error(&self) -> bool {
        self.edits.error.is_some()
    }

    /// Runs on every app frame, including while another page is open.
    pub fn tick(&mut self, ctx: &egui::Context, handle: &Handle) {
        let now = Instant::now();
        if let Some(wait) = &self.save_result {
            let result = match wait.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "Speicherauftrag unterbrochen; erneuter Versuch folgt".into(),
                )),
            };
            if let Some(result) = result {
                self.save_result = None;
                self.edits.complete(result, now);
            }
        }
        let snapshot = handle.snapshot();
        if let Some(cfg) = snapshot.cfg() {
            self.edits.observe(&cfg.repos);
            if let Some(changes) = self.edits.request(cfg, now) {
                let (done, wait) = mpsc::channel();
                handle.send(Action::AutosaveRepos(changes, done));
                self.save_result = Some(wait);
            }
        } else if !self.edits.is_idle() {
            self.edits.error = Some(
                "team.toml kann gerade nicht gelesen werden; Eingaben bleiben erhalten".into(),
            );
        }
        if let Some(delay) = self.edits.repaint_after(now) {
            ctx.request_repaint_after(delay);
        }
    }

    pub fn flush(&mut self, ctx: &egui::Context, handle: &Handle) {
        self.edits.expedite(Instant::now());
        self.tick(ctx, handle);
    }

    /// Fixed above the scroll area, so errors never hide below a long project form.
    pub fn status(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("project-autosave-status").show(ctx, |ui| {
            if let Some(error) = &self.edits.error {
                ui.colored_label(theme::ERROR, format!("Nicht gespeichert: {error} — Eingaben bleiben erhalten."));
            } else if self.edits.is_saving() {
                ui.horizontal(|ui| { ui.add(egui::Spinner::new().size(12.0)); ui.label("Projekte werden automatisch gespeichert …"); });
            } else if !self.edits.is_idle() { muted(ui, "Änderungen werden automatisch gespeichert …"); }
            else { muted(ui, "Alle Projektänderungen gespeichert · Schalter sofort, Eingaben nach kurzer Eingabepause"); }
        });
    }
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(
            ui,
            "Projekte",
            "Welche Repositories das Team bearbeitet, mit welchem Modell und welchen Rollen.",
        );
        muted(ui, "Modelle, Anbieterfreigaben, Label-Regeln und Budgets werden pro Projekt eingestellt. Globale Einstellungen liefern Vorgaben; Zugangsdaten und Team-Chat bleiben dort.");
        ui.add_space(10.0);
        let Some(file_cfg) = s.cfg() else {
            muted(ui, "Keine gültige team.toml — siehe Einrichtung.");
            return;
        };
        let mut cfg = file_cfg.clone();
        cfg.repos = self.edits.shown(&file_cfg.repos);
        let before = cfg.repos.clone();
        let mut typing = false;
        let list = projects::list(&cfg, &s.discovered);
        card(ui, None, |ui| {
            if list.is_empty() {
                muted(ui, "Noch keine Repositories.");
            }
            for (i, p) in list.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                typing |= project_row(ui, p, s, &mut cfg);
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_repo)
                    .hint_text("owner/repo")
                    .desired_width(260.0),
            );
            if ui
                .add_enabled(
                    !self.new_repo.trim().is_empty(),
                    egui::Button::new("Repo hinzufügen"),
                )
                .clicked()
            {
                match projects::add(&mut cfg, &self.new_repo) {
                    Ok(()) => {
                        self.new_repo.clear();
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        });
        if cfg.repos != before {
            self.edits
                .edit(&file_cfg.repos, cfg.repos, typing, Instant::now());
        }
        self.tick(ui.ctx(), handle);
        if let Some(error) = &self.error {
            ui.colored_label(theme::ERROR, error);
        }
        ui.add_space(6.0);
        let org = &file_cfg.discovery.org;
        let discovery = if org.is_empty() {
            "Auto-Discovery ist aus.".to_string()
        } else {
            format!("Repos der Organisation {org} kommen über das Label {} dazu und verfallen nach {} Tagen ohne Aktivität.", file_cfg.labels.task, file_cfg.discovery.expiry_days)
        };
        muted(ui, format!("Coder und Reviewer laufen in jedem eingeschalteten Repo; QA und PR-Feedback schaltet die .agent.toml des Repos (agents_enabled). {discovery}"));
    }
}

fn project_row(ui: &mut Ui, p: &Project, s: &Snapshot, cfg: &mut TeamConfig) -> bool {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(260.0, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add(egui::Label::new(RichText::new(&p.repo).strong()).truncate());
            },
        );
        if !p.coder {
            pill(ui, "Coder aus", theme::MUTED);
        }
        if p.po {
            pill(ui, "Product Owner", theme::ACCENT);
        }
        if p.discovered {
            pill(ui, "entdeckt", theme::INFO);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            link(ui, "GitHub", &format!("https://github.com/{}", p.repo));
            if p.configured
                && ui
                    .small_button("Entfernen")
                    .on_hover_text("Eintrag aus team.toml löschen")
                    .clicked()
            {
                projects::remove(cfg, &p.repo);
            }
        });
    });
    ui.horizontal(|ui| {
        let mut coder = p.coder;
        let label = if p.discovered && !p.configured {
            "Coder (automatisch entdeckt)"
        } else {
            "Coder bearbeitet Issues"
        };
        if ui.checkbox(&mut coder, label).changed() {
            projects::set_coder(cfg, &p.repo, coder);
        }
        ui.add_space(12.0);
        ui.label(RichText::new("Basis-Routing:").color(MUTED));
        let mut routing = p.routing;
        let profiled = cfg.repo(&p.repo).is_some_and(|r| r.profile.is_some());
        ui.add_enabled_ui(!profiled, |ui| {
            egui::ComboBox::from_id_salt(("routing", &p.repo))
                .selected_text(routing_label(routing))
                .show_ui(ui, |ui| {
                    for r in ROUTINGS {
                        ui.selectable_value(&mut routing, r, routing_label(r));
                    }
                });
        })
        .response
        .on_disabled_hover_text("Das aktive Projektprofil bestimmt Anbieter und Modelle.");
        if profiled {
            pill(ui, "Profil aktiv", theme::ACCENT);
        }
        if routing != p.routing {
            projects::set_routing(cfg, &p.repo, routing);
        }
        ui.add_space(12.0);
        roles(ui, s, &p.repo);
    });
    super::project_profiles::editor(ui, cfg, &p.repo)
}

/// Coder and reviewer always run; .agent.toml switches QA and PR feedback.
fn roles(ui: &mut Ui, s: &Snapshot, repo: &str) {
    pill(ui, "Coder", theme::OK);
    pill(ui, "Reviewer", theme::OK);
    match s.agent_tomls.get(repo) {
        None => {
            ui.label(
                RichText::new("QA/PR-Feedback: wird geladen …")
                    .color(MUTED)
                    .size(12.0),
            );
        }
        Some(Err(_)) => {
            ui.label(
                RichText::new("keine .agent.toml → kein QA/PR-Feedback")
                    .color(MUTED)
                    .size(12.0),
            );
        }
        Some(Ok(enabled)) => {
            for (key, name) in [("qa", "QA"), ("pr-feedback", "PR-Feedback")] {
                let on = enabled.iter().any(|e| e == key);
                pill(ui, name, if on { theme::OK } else { theme::MUTED });
            }
        }
    }
}
