//! Projects: the repositories the team works on — coder on/off, model family, roles
//! enabled by each repo's .agent.toml. Edits go to team.toml (applies without restart).

use super::theme::{self, MUTED};
use super::widgets::{card, link, muted, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::projects::{self, routing_label, Project, ROUTINGS};
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};
use team_core::config::TeamConfig;

#[derive(Default)]
pub struct ProjectsPage {
    /// pending edit of team.toml (None = mirror the file)
    draft: Option<TeamConfig>,
    new_repo: String,
    error: Option<String>,
}

impl ProjectsPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Projekte", "Welche Repositories das Team bearbeitet, mit welchem Modell und welchen Rollen.");
        let Some(file_cfg) = s.cfg() else {
            muted(ui, "Keine gültige team.toml — siehe Einrichtung.");
            return;
        };
        let mut cfg = self.draft.clone().unwrap_or_else(|| file_cfg.clone());
        let list = projects::list(&cfg, &s.discovered);
        card(ui, None, |ui| {
            if list.is_empty() {
                muted(ui, "Noch keine Repositories.");
            }
            for (i, p) in list.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                project_row(ui, p, s, &mut cfg);
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_repo).hint_text("owner/repo").desired_width(260.0));
            if ui.add_enabled(!self.new_repo.trim().is_empty(), egui::Button::new("Repo hinzufügen")).clicked() {
                match projects::add(&mut cfg, &self.new_repo) {
                    Ok(()) => self.new_repo.clear(),
                    Err(e) => self.error = Some(e),
                }
            }
        });
        self.draft = (cfg != *file_cfg).then_some(cfg);
        ui.add_space(10.0);
        self.footer(ui, handle);
        ui.add_space(6.0);
        let org = &file_cfg.discovery.org;
        let discovery = if org.is_empty() { "Auto-Discovery ist aus.".to_string() } else { format!("Repos der Organisation {org} kommen über das Label {} dazu und verfallen nach {} Tagen ohne Aktivität.", file_cfg.labels.task, file_cfg.discovery.expiry_days) };
        muted(ui, format!("Coder und Reviewer laufen in jedem eingeschalteten Repo; QA und PR-Feedback schaltet die .agent.toml des Repos (agents_enabled). {discovery}"));
    }

    fn footer(&mut self, ui: &mut Ui, handle: &Handle) {
        let dirty = self.draft.is_some();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern") {
                    if let Some(cfg) = self.draft.take() {
                        handle.send(Action::SaveRepos(cfg.repos));
                    }
                    self.error = None;
                }
                if secondary(ui, "Verwerfen") {
                    self.draft = None;
                    self.error = None;
                }
            });
            if dirty {
                ui.label(RichText::new("• ungespeichert").color(theme::WARN));
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme::ERROR));
            }
        });
    }
}

fn project_row(ui: &mut Ui, p: &Project, s: &Snapshot, cfg: &mut TeamConfig) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(260.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(&p.repo).strong()).truncate());
        });
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
            if p.configured && ui.small_button("Entfernen").on_hover_text("Eintrag aus team.toml löschen").clicked() {
                projects::remove(cfg, &p.repo);
            }
        });
    });
    ui.horizontal(|ui| {
        let mut coder = p.coder;
        let label = if p.discovered && !p.configured { "Coder (automatisch entdeckt)" } else { "Coder bearbeitet Issues" };
        if ui.checkbox(&mut coder, label).changed() {
            projects::set_coder(cfg, &p.repo, coder);
        }
        ui.add_space(12.0);
        ui.label(RichText::new("Modell:").color(MUTED));
        let mut routing = p.routing;
        egui::ComboBox::from_id_salt(("routing", &p.repo)).selected_text(routing_label(routing)).show_ui(ui, |ui| {
            for r in ROUTINGS {
                ui.selectable_value(&mut routing, r, routing_label(r));
            }
        });
        if routing != p.routing {
            projects::set_routing(cfg, &p.repo, routing);
        }
        ui.add_space(12.0);
        roles(ui, s, &p.repo);
    });
}

/// Coder and reviewer always run; .agent.toml switches QA and PR feedback.
fn roles(ui: &mut Ui, s: &Snapshot, repo: &str) {
    pill(ui, "Coder", theme::OK);
    pill(ui, "Reviewer", theme::OK);
    match s.agent_tomls.get(repo) {
        None => {
            ui.label(RichText::new("QA/PR-Feedback: wird geladen …").color(MUTED).size(12.0));
        }
        Some(Err(_)) => {
            ui.label(RichText::new("keine .agent.toml → kein QA/PR-Feedback").color(MUTED).size(12.0));
        }
        Some(Ok(enabled)) => {
            for (key, name) in [("qa", "QA"), ("pr-feedback", "PR-Feedback")] {
                let on = enabled.iter().any(|e| e == key);
                pill(ui, name, if on { theme::OK } else { theme::MUTED });
            }
        }
    }
}
