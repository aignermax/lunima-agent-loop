//! Project policy editor. Valid edits are automatically persisted by Projects.
use super::widgets::muted;
use eframe::egui::{self, Ui};
use team_core::config::{Backend, TeamConfig};
use team_core::project::{
    self, ModelSpec, ProjectBudgets, ProjectProfile, ReasoningEffort, SessionRole,
};

const BACKENDS: [Backend; 5] = [
    Backend::Claude,
    Backend::Codex,
    Backend::Openrouter,
    Backend::Eco,
    Backend::OpenAi,
];

pub fn editor(ui: &mut Ui, cfg: &mut TeamConfig, repo: &str) -> bool {
    ui.push_id(("project-profile", repo), |ui| {
        let mut typing = false;
        let saved = cfg.repo(repo).and_then(|r| r.profile.clone());
        let mut profile = saved.clone();
        let mut enabled = profile.is_some();
        if ui.checkbox(&mut enabled, "Eigenes Modell- und Budgetprofil").changed() {
            profile = enabled.then(|| ProjectProfile::from_legacy(cfg, repo));
        }
        if let Some(p) = &mut profile {
            egui::CollapsingHeader::new("Anbieter, Modelle und Budgets").default_open(true).show(ui, |ui| {
                ui.label("Erlaubte Anbieter");
                ui.horizontal_wrapped(|ui| {
                    for backend in BACKENDS {
                        let mut allowed = p.allowed_backends.contains(&backend);
                        if ui.checkbox(&mut allowed, backend.label()).changed() {
                            p.allowed_backends.retain(|b| *b != backend);
                            if allowed { p.allowed_backends.push(backend); }
                        }
                    }
                });
                muted(ui, "Diese Freigaben gelten auch für Labels, Rollen und Ausweichmodelle. Nicht freigegebene Regeln können nicht gespeichert werden.");
                ui.separator();
                ui.label("Standard für dieses Projekt");
                model(ui, &mut p.standard, &p.allowed_backends, true, &mut typing);
                egui::CollapsingHeader::new("Label-Regeln").default_open(true).show(ui, |ui| {
                    optional(ui, "complex — komplexe Aufgaben", &mut p.complex, &p.allowed_backends, &mut typing);
                    optional(ui, "Premium — bisheriges claudeapi-Label", &mut p.premium, &p.allowed_backends, &mut typing);
                    optional(ui, "eco — günstige Aufgaben", &mut p.eco, &p.allowed_backends, &mut typing);
                    muted(ui, "Premium hat Vorrang vor complex, danach eco. Die Label-Namen stammen aus den globalen Vorgaben. Ohne eigene Regel bleibt das Rollenmodell aktiv; größere Budgets sind separat einstellbar.");
                });
                egui::CollapsingHeader::new("Modelle nach Rolle").show(ui, |ui| {
                    optional(ui, "Reviewer", &mut p.reviewer, &p.allowed_backends, &mut typing);
                    optional(ui, "critical — kritisches Review", &mut p.critical, &p.allowed_backends, &mut typing);
                    optional(ui, "QA-Review", &mut p.qa, &p.allowed_backends, &mut typing);
                    optional(ui, "PR-Feedback", &mut p.feedback, &p.allowed_backends, &mut typing);
                    optional(ui, "Product Owner", &mut p.product_owner, &p.allowed_backends, &mut typing);
                    optional(ui, "Discovery", &mut p.discovery, &p.allowed_backends, &mut typing);
                });
                egui::CollapsingHeader::new("Ausweichmodell").show(ui, |ui| {
                    optional(ui, "Freigegebenes Ausweichmodell verwenden", &mut p.fallback, &p.allowed_backends, &mut typing);
                    muted(ui, "Ohne Ersatz wartet die Sitzung. Ausweichen erfolgt nur vor dem Start: fehlender API-Key, fehlende CLI oder bekanntes Nutzungslimit. Laufende/fehlgeschlagene Arbeit wird nicht automatisch bei einem anderen Anbieter wiederholt.");
                });
                egui::CollapsingHeader::new("Arbeitsbudgets — unabhängig von der Modellwahl").show(ui, |ui| budgets(ui, &mut p.budgets, cfg, &mut typing));
                let mut preview = cfg.clone();
                if let Some(r) = preview.repos.iter_mut().find(|r| r.name.eq_ignore_ascii_case(repo)) { r.profile = Some(p.clone()); }
                else { preview.repos.push(team_core::config::RepoConfig { name: repo.into(), profile: Some(p.clone()), ..Default::default() }); }
                match project::validate(&preview, repo, p) {
                    Err(e) => { ui.colored_label(super::theme::ERROR, e); }
                    Ok(()) => {
                        egui::CollapsingHeader::new("Wirksame Modelle anzeigen").show(ui, |ui| {
                            for (title, role, tags) in [
                                ("Standard", SessionRole::Coder, vec![]),
                                ("complex", SessionRole::Coder, vec![cfg.labels.complex.clone()]),
                                ("eco", SessionRole::Coder, vec![cfg.labels.eco.clone()]),
                                ("Reviewer", SessionRole::Reviewer, vec![]),
                                ("critical", SessionRole::Reviewer, vec![cfg.labels.critical.clone()]),
                                ("QA", SessionRole::Qa, vec![]),
                                ("PR-Feedback", SessionRole::Feedback, vec![]),
                                ("Product Owner", SessionRole::ProductOwner, vec![]),
                                ("Discovery", SessionRole::Discovery, vec![]),
                            ] {
                                let text = match project::resolve(&preview, repo, role, &tags) {
                                    Ok(d) => format!("{} · {}{}", d.backend.label(), if d.choice.model.is_empty() { "CLI-Standard" } else { &d.choice.model }, d.effort.map(|e| format!(" · Denkaufwand {}", e.as_str())).unwrap_or_default()),
                                    Err(e) => e,
                                };
                                ui.label(format!("{title}: {text}"));
                            }
                        });
                    }
                }
            });
        } else {
            muted(ui, "Bisheriges Projekt-Routing mit globalen Modell-/Budgetvorgaben. Aktivieren übernimmt die bisherige Anbieterfreigabe; globale Vorgaben bleiben vererbt.");
        }
        if profile != saved { crate::core::projects::set_profile(cfg, repo, profile); }
        typing
    }).inner
}

fn optional(
    ui: &mut Ui,
    label: &str,
    spec: &mut Option<ModelSpec>,
    allowed: &[Backend],
    typing: &mut bool,
) {
    ui.push_id(label, |ui| {
        let mut own = spec.is_some();
        if ui.checkbox(&mut own, label).changed() {
            *spec = own.then(ModelSpec::default);
        }
        if let Some(spec) = spec {
            ui.indent("model", |ui| model(ui, spec, allowed, false, typing));
        } else {
            muted(ui, "Vom Projekt-/Rollenstandard übernommen");
        }
    });
}

fn model(
    ui: &mut Ui,
    spec: &mut ModelSpec,
    allowed: &[Backend],
    standard: bool,
    typing: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Anbieter:");
        egui::ComboBox::from_id_salt("backend")
            .selected_text(spec.backend.map(|b| b.label()).unwrap_or(if standard {
                "Bisheriges Projekt-Routing"
            } else {
                "Vom Projektstandard"
            }))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut spec.backend,
                    None,
                    if standard {
                        "Bisheriges Projekt-Routing"
                    } else {
                        "Vom Projektstandard"
                    },
                );
                for backend in BACKENDS.into_iter().filter(|b| allowed.contains(b)) {
                    if ui
                        .selectable_value(&mut spec.backend, Some(backend), backend.label())
                        .changed()
                    {
                        spec.model = None;
                        spec.effort = None;
                    }
                }
            });
        let mut inherited = spec.model.is_none();
        if ui.checkbox(&mut inherited, "Modell übernehmen").changed() {
            spec.model = if inherited { None } else { Some(String::new()) };
        }
        if let Some(value) = &mut spec.model {
            if ui
                .add(
                    egui::TextEdit::singleline(value)
                        .desired_width(210.0)
                        .hint_text("Modell-ID; leer = CLI-Standard"),
                )
                .changed()
            {
                *typing = true;
            }
        }
        ui.label("Denkaufwand:");
        egui::ComboBox::from_id_salt("effort")
            .selected_text(spec.effort.map(|e| e.as_str()).unwrap_or("Übernehmen"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut spec.effort, None, "Übernehmen / CLI-Standard");
                for effort in [
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::Xhigh,
                    ReasoningEffort::Max,
                    ReasoningEffort::Ultra,
                ] {
                    ui.selectable_value(&mut spec.effort, Some(effort), effort.as_str());
                }
            });
    });
    muted(ui, "Modell übernehmen: Projektstandard, danach globale Vorgabe des Anbieters/der Rolle. Denkaufwand ist nur bei Codex/Responses einstellbar; Unterstützung hängt vom Modell ab. API-Modelle benötigen eine Modell-ID.");
}

fn budgets(ui: &mut Ui, p: &mut ProjectBudgets, cfg: &TeamConfig, typing: &mut bool) {
    let base = &cfg.budgets;
    muted(ui, "Ohne Häkchen gilt die angezeigte globale Vorgabe. Schrittlimits wirken nur mit der Claude-CLI; Codex wird durch Zeitlimits begrenzt. Tokenlimits werden nach einer Sitzung geprüft und sind keine harte Begrenzung innerhalb eines Aufrufs.");
    egui::Grid::new("budgets")
        .num_columns(3)
        .spacing([12.0, 7.0])
        .show(ui, |ui| {
            macro_rules! number {
                ($field:ident, $label:literal) => {{
                    ui.label($label);
                    let mut own = p.$field.is_some();
                    if ui.checkbox(&mut own, "Eigener Wert").changed() {
                        p.$field = own.then_some(base.$field);
                    }
                    if let Some(value) = &mut p.$field {
                        if ui.add(egui::DragValue::new(value)).changed() {
                            *typing = true;
                        }
                    } else {
                        ui.label(format!("{} (global)", base.$field));
                    }
                    ui.end_row();
                }};
            }
            number!(timeout_minutes_regular, "Zeitlimit normal (Min.)");
            number!(timeout_minutes_complex, "Zeitlimit complex (Min.)");
            number!(max_tokens_regular, "Tokenbudget normal je Issue");
            number!(max_tokens_complex, "Tokenbudget complex je Issue");
            number!(max_review_rounds_regular, "Review-Runden normal");
            number!(max_review_rounds_complex, "Review-Runden complex");
            number!(max_sessions, "Sitzungen je Issue");
            number!(max_attempts, "Versuche je Issue");
            number!(max_qa_fix_rounds, "QA-Nachbesserungen");
            number!(pr_feedback_max_rounds, "PR-Feedback-Runden");
            number!(max_turns_regular, "Schritte normal (Claude)");
            number!(max_turns_complex, "Schritte complex (Claude)");
            number!(reviewer_max_turns, "Reviewer-Schritte (Claude)");
            number!(pr_feedback_max_turns, "PR-Feedback-Schritte (Claude)");
            number!(inactivity_minutes, "Inaktivitätslimit (Min.)");
            macro_rules! role_number {
                ($field:ident, $label:literal, $base:expr) => {{
                    ui.label($label);
                    let mut own = p.$field.is_some();
                    if ui.checkbox(&mut own, "Eigener Wert").changed() {
                        p.$field = own.then_some($base);
                    }
                    if let Some(value) = &mut p.$field {
                        if ui.add(egui::DragValue::new(value)).changed() {
                            *typing = true;
                        }
                    } else {
                        ui.label(format!("{} (global)", $base));
                    }
                    ui.end_row();
                }};
            }
            role_number!(
                product_owner_max_turns,
                "PO-Schritte (Claude)",
                cfg.product_owner.max_turns
            );
            role_number!(
                product_owner_timeout_minutes,
                "PO-Zeitlimit (Min.)",
                cfg.product_owner.timeout_minutes
            );
            role_number!(
                discovery_max_turns,
                "Discovery-Schritte (Claude)",
                cfg.product_discovery.max_turns
            );
            role_number!(
                discovery_timeout_minutes,
                "Discovery-Zeitlimit je Stufe (Min.)",
                cfg.product_discovery.timeout_minutes
            );
            ui.label("Test-Gate vor dem Review");
            let mut own = p.test_gate.is_some();
            if ui.checkbox(&mut own, "Eigener Wert").changed() {
                p.test_gate = own.then_some(base.test_gate);
            }
            if let Some(value) = &mut p.test_gate {
                ui.checkbox(value, "Aktiv");
            } else {
                ui.label(if base.test_gate {
                    "An (global)"
                } else {
                    "Aus (global)"
                });
            }
            ui.end_row();
        });
}
