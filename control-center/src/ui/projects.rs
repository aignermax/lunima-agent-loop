//! Projects: the repositories the team works on — issue agent on/off, model family,
//! roles enabled by each repo's .agent.toml. Edits go to the issue agent's .env.

use super::theme::{self, MUTED};
use super::widgets::{card, link, muted, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::envfile::EnvFile;
use crate::core::projects::{self, Project, Routing, REPOS_KEY};
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};

#[derive(Default)]
pub struct ProjectsPage {
    /// Edited copy of the .env; None = showing the file as it is.
    draft: Option<EnvFile>,
    new_repo: String,
}

const ROLES: &[(&str, &str)] = &[("coder", "Coder"), ("reviewer", "Reviewer"), ("qa", "QA"), ("pr-feedback", "PR-Feedback")];

impl ProjectsPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Projekte", "Welche Repositories das Team bearbeitet, mit welchem Modell und welchen Rollen.");
        let Some(file_env) = &s.team.env else {
            muted(ui, "Issue-Agent nicht verbunden — siehe Seite Team.");
            return;
        };
        let env = self.draft.clone().unwrap_or_else(|| file_env.clone());
        let list = projects::list(&env, &s.team.discovered, &s.config_str("githubRepo"));
        let mut edited = env.clone();
        card(ui, None, |ui| {
            for (i, p) in list.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                project_row(ui, p, s, &mut edited);
            }
        });
        if edited.list(REPOS_KEY) != env.list(REPOS_KEY) || edited.list(projects::KIMI_KEY) != env.list(projects::KIMI_KEY) || edited.list(projects::FORCE_KEY) != env.list(projects::FORCE_KEY) {
            self.draft = Some(edited);
        }
        ui.add_space(10.0);
        self.add_repo(ui, file_env);
        ui.add_space(10.0);
        self.footer(ui, handle);
        ui.add_space(6.0);
        muted(ui, "Rollen pro Repo stehen in dessen .agent.toml (agents_enabled) — Änderung dort per PR. Entdeckte Repos kommen über das Label agent-task in der Organisation dazu.");
    }

    fn add_repo(&mut self, ui: &mut Ui, file_env: &EnvFile) {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_repo).hint_text("owner/repo").desired_width(260.0));
            let valid = self.new_repo.trim().split('/').filter(|p| !p.is_empty()).count() == 2;
            if ui.add_enabled(valid, egui::Button::new("Repo hinzufügen")).clicked() {
                let draft = self.draft.get_or_insert_with(|| file_env.clone());
                projects::set_listed(draft, REPOS_KEY, self.new_repo.trim(), true);
                self.new_repo.clear();
            }
        });
    }

    fn footer(&mut self, ui: &mut Ui, handle: &Handle) {
        let dirty = self.draft.is_some();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern & Agents neu starten") {
                    if let Some(d) = self.draft.take() {
                        handle.send(Action::SaveEnv(d, true));
                    }
                }
                if secondary(ui, "Verwerfen") {
                    self.draft = None;
                }
            });
            if dirty {
                ui.label(RichText::new("● ungespeichert").color(theme::WARN));
            }
        });
    }
}

fn project_row(ui: &mut Ui, p: &Project, s: &Snapshot, env: &mut EnvFile) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(260.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(&p.repo).strong()).truncate());
        });
        if !p.handled() {
            pill(ui, "Issue-Agent inaktiv", theme::MUTED);
        }
        if p.po {
            pill(ui, "Product Owner", theme::ACCENT);
        }
        if p.discovered {
            pill(ui, "entdeckt", theme::INFO);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            link(ui, "GitHub", &format!("https://github.com/{}", p.repo));
        });
    });
    ui.horizontal(|ui| {
        let mut handled = p.manual;
        let label = if p.discovered && !p.manual { "Issue-Agent (automatisch entdeckt)" } else { "Issue-Agent bearbeitet" };
        if ui.checkbox(&mut handled, label).changed() {
            projects::set_listed(env, REPOS_KEY, &p.repo, handled);
        }
        ui.add_space(12.0);
        ui.label(RichText::new("Modell:").color(MUTED));
        let mut routing = p.routing;
        egui::ComboBox::from_id_salt(("routing", &p.repo)).selected_text(routing.label()).show_ui(ui, |ui| {
            for r in [Routing::Claude, Routing::KimiAllowed, Routing::KimiForced] {
                ui.selectable_value(&mut routing, r, r.label());
            }
        });
        if routing != p.routing {
            projects::set_routing(env, &p.repo, routing);
        }
        ui.add_space(12.0);
        roles(ui, s, &p.repo);
    });
}

fn roles(ui: &mut Ui, s: &Snapshot, repo: &str) {
    match s.team.agent_tomls.get(repo) {
        None => {
            ui.label(RichText::new("Rollen: wird geladen …").color(MUTED).size(12.0));
        }
        Some(Err(_)) => {
            ui.label(RichText::new("keine .agent.toml (nur Coder)").color(MUTED).size(12.0));
        }
        Some(Ok(enabled)) => {
            for (key, name) in ROLES {
                let on = enabled.iter().any(|e| e == key);
                pill(ui, name, if on { theme::OK } else { theme::MUTED });
            }
        }
    }
}
