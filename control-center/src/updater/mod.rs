mod cache;
mod install;
mod model;
mod worker;

use crate::ui::{theme, widgets};
use eframe::egui::{self, RichText};
pub use worker::Worker;

#[derive(Clone, Debug)]
pub enum Status {
    Checking,
    Current,
    Downloading(String),
    Ready(String),
    Waiting(String),
    Postponing,
    Handoff,
    Installing,
    Error(String),
}

impl Worker {
    /// Returns true before installation, so queued text edits can be flushed first.
    pub fn banner(&self, ctx: &egui::Context) -> bool {
        let state = self.status();
        if matches!(state, Status::Installing) {
            std::process::exit(0);
        }
        let mut install_now = false;
        egui::TopBottomPanel::top("software_update")
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(20, 9)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("Lunima Team {}", model::VERSION))
                            .small()
                            .color(theme::MUTED),
                    );
                    ui.separator();
                    match &state {
                        Status::Checking => {
                            ui.spinner();
                            ui.label("Suche nach Updates …");
                        }
                        Status::Current => {
                            ui.label(RichText::new("Auf dem neuesten Stand").color(theme::MUTED));
                            if ui.small_button("Nach Updates suchen").clicked() {
                                self.send(worker::Command::Check);
                            }
                        }
                        Status::Downloading(v) => {
                            ui.spinner();
                            ui.label(format!("Update {v} wird heruntergeladen …"));
                        }
                        Status::Ready(v) => {
                            ui.label(
                                RichText::new(format!("Update {v} bereit"))
                                    .color(theme::OK)
                                    .strong(),
                            );
                            match self
                                .eligibility
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .as_ref()
                            {
                                Some(Ok(())) => {
                                    if widgets::primary(ui, "Jetzt aktualisieren") {
                                        install_now = true;
                                    }
                                    ui.label(
                                        RichText::new("Sonst automatisch beim nächsten Start")
                                            .small()
                                            .color(theme::MUTED),
                                    );
                                }
                                Some(Err(reason)) => {
                                    ui.label(RichText::new(reason).small().color(theme::MUTED));
                                    if ui.small_button("Download öffnen").clicked() {
                                        crate::sys::open_path(&cache::root(&self.home()));
                                    }
                                }
                                None => {
                                    ui.spinner();
                                }
                            }
                        }
                        Status::Waiting(v) => {
                            ui.spinner();
                            ui.label(format!(
                                "Update {v}: laufende Aufgaben werden fertiggestellt …"
                            ));
                            if ui.button("Verschieben").clicked() {
                                self.send(worker::Command::Cancel);
                            }
                        }
                        Status::Postponing=>{ui.spinner();ui.label("Update verschoben · Team wird nach Abschluss der Aufgaben fortgesetzt …");}
                        Status::Handoff=>{ui.spinner();ui.label("Installation wird vorbereitet …");}
                        Status::Error(e) => {
                            ui.label(
                                RichText::new("Update nicht abgeschlossen").color(theme::WARN),
                            )
                            .on_hover_text(e);
                            if ui.small_button("Erneut prüfen").clicked() {
                                self.send(worker::Command::Check);
                            }
                            if ui.small_button("Installation erneut versuchen").clicked() {
                                install_now = true;
                            }
                        }
                        Status::Installing => {}
                    }
                });
            });
        install_now
    }
    pub fn install(&self) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = Status::Waiting("…".into());
        self.send(worker::Command::Install);
    }
    pub fn blocking(&self) -> bool {
        matches!(
            self.status(),
            Status::Waiting(_) | Status::Postponing | Status::Handoff | Status::Installing
        )
    }
    pub fn save_error(&self) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) =
            Status::Error("Bitte ungültige Einstellungen vor dem Update korrigieren".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ready_banner_renders_install_action_and_restart_promise() {
        let worker = Worker::fixture(Status::Ready("0.6.0".into()));
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1240., 820.),
        ));
        let output = ctx.run(input, |ctx| {
            assert!(!worker.banner(ctx));
        });
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.job.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Update 0.6.0 bereit"), "{text}");
        assert!(text.contains("Jetzt aktualisieren"), "{text}");
        assert!(text.contains("beim nächsten Start"), "{text}");
    }
}
