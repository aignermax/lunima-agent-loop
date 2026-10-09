//! Projects: the repositories the team works on — issue agent on/off, model family,
//! roles enabled by each repo's .agent.toml. Edits are saved as key changes to the agent's .env.

use super::theme::{self, MUTED};
use super::widgets::{card, link, muted, page_title, pill, primary, secondary};
use crate::collector::{Action, Handle};
use crate::core::envfile::{EnvEdits, EnvFile};
use crate::core::projects::{self, Project, Routing, FORCE_KEY, KIMI_KEY, REPOS_KEY};
use crate::core::snapshot::Snapshot;
use eframe::egui::{self, RichText, Ui};

const LIST_KEYS: [&str; 3] = [REPOS_KEY, KIMI_KEY, FORCE_KEY];

#[derive(Default)]
pub struct ProjectsPage {
    edits: EnvEdits,
    new_repo: String,
    error: Option<String>,
}

impl ProjectsPage {
    pub fn show(&mut self, ui: &mut Ui, s: &Snapshot, handle: &Handle) {
        page_title(ui, "Projekte", "Welche Repositories das Team bearbeitet, mit welchem Modell und welchen Rollen.");
        let Some(file_env) = &s.team.env else {
            muted(ui, "Issue-Agent nicht verbunden — siehe Seite Team.");
            return;
        };
        // the file as it is now, with this page's pending edits on top
        let mut env = file_env.clone();
        env.apply(&self.edits);
        let before = env.clone();
        let list = projects::list(&env, &s.team.discovered, &s.config_str("githubRepo"));
        card(ui, None, |ui| {
            for (i, p) in list.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                if let Err(e) = project_row(ui, p, s, &mut env) {
                    self.error = Some(e);
                }
            }
        });
        ui.add_space(10.0);
        self.add_repo(ui, &mut env);
        self.record(&before, &env, file_env);
        ui.add_space(10.0);
        self.footer(ui, handle);
        ui.add_space(6.0);
        muted(ui, "Coder und Reviewer laufen in jedem Repo; QA und PR-Feedback schaltet die .agent.toml des Repos (agents_enabled) — Änderung dort per PR. Entdeckte Repos kommen über das Label agent-task in der Organisation dazu und verfallen nach 8 Tagen ohne Aktivität.");
    }

    /// Turns list changes made this frame into key edits (relative to the file).
    fn record(&mut self, before: &EnvFile, after: &EnvFile, file: &EnvFile) {
        for key in LIST_KEYS {
            if before.list(key) == after.list(key) {
                continue;
            }
            let value = after.list(key).join(",");
            if value == file.list(key).join(",") {
                self.edits.remove(key);
            } else {
                self.edits.insert(key.to_string(), if value.is_empty() { None } else { Some(value) });
            }
        }
    }

    fn add_repo(&mut self, ui: &mut Ui, env: &mut EnvFile) {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_repo).hint_text("owner/repo").desired_width(260.0));
            let valid = self.new_repo.trim().split('/').filter(|p| !p.is_empty()).count() == 2;
            if ui.add_enabled(valid, egui::Button::new("Repo hinzufügen")).clicked() {
                if let Err(e) = projects::set_handled(env, self.new_repo.trim(), true) {
                    self.error = Some(e);
                }
                self.new_repo.clear();
            }
        });
    }

    fn footer(&mut self, ui: &mut Ui, handle: &Handle) {
        let dirty = !self.edits.is_empty();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if primary(ui, "Speichern & Agents neu starten") {
                    handle.send(Action::SaveEnv(std::mem::take(&mut self.edits), true));
                    self.error = None;
                }
                if secondary(ui, "Verwerfen") {
                    self.edits.clear();
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

fn project_row(ui: &mut Ui, p: &Project, s: &Snapshot, env: &mut EnvFile) -> Result<(), String> {
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
    let mut result = Ok(());
    ui.horizontal(|ui| {
        let mut handled = p.manual;
        let label = if p.discovered && !p.manual { "Issue-Agent (automatisch entdeckt)" } else { "Issue-Agent bearbeitet" };
        if ui.checkbox(&mut handled, label).changed() {
            result = projects::set_handled(env, &p.repo, handled);
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
    result
}

/// Coder and reviewer always run; .agent.toml only switches QA and PR feedback.
fn roles(ui: &mut Ui, s: &Snapshot, repo: &str) {
    pill(ui, "Coder", theme::OK);
    pill(ui, "Reviewer", theme::OK);
    match s.team.agent_tomls.get(repo) {
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
