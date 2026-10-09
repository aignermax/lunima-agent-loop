//! Small reusable building blocks: cards, status dots, pills, stat tiles, buttons, links.

use super::theme::{self, ACCENT, MUTED, TEXT};
use crate::core::health::Level;
use crate::sys;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Sense, Stroke, Ui};

/// A titled card that fills the available width.
pub fn card<R>(ui: &mut Ui, title: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    theme::card_frame()
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(t) = title {
                ui.label(RichText::new(t).size(15.5).strong());
                ui.add_space(6.0);
            }
            add(ui)
        })
        .inner
}

/// A filled circle with a soft halo.
pub fn dot(ui: &mut Ui, color: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(radius * 2.6, radius * 2.6), Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), radius * 1.3, theme::tint(color, 40));
    painter.circle_filled(rect.center(), radius, color);
}

pub fn level_dot(ui: &mut Ui, level: Level, radius: f32) {
    dot(ui, theme::level_color(level), radius);
}

/// Rounded label chip.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(theme::tint(color, 38))
        .stroke(Stroke::new(1.0, theme::tint(color, 110)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).extend()));
}

/// Big number with a caption, used in the overview row.
pub fn stat(ui: &mut Ui, caption: &str, value: &str, sub: &str) {
    theme::card_frame().show(ui, |ui| {
        ui.set_min_width(150.0);
        ui.label(RichText::new(caption).size(12.0).color(MUTED));
        ui.label(RichText::new(value).size(24.0).strong().color(TEXT));
        if !sub.is_empty() {
            ui.label(RichText::new(sub).size(12.0).color(MUTED));
        }
    });
}

pub fn primary(ui: &mut Ui, text: &str) -> bool {
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong()).fill(ACCENT).corner_radius(CornerRadius::same(8));
    ui.add(button).clicked()
}

pub fn secondary(ui: &mut Ui, text: &str) -> bool {
    ui.button(text).clicked()
}

/// Text that opens a URL (or folder) when clicked.
pub fn link(ui: &mut Ui, text: &str, target: &str) {
    if ui.link(RichText::new(text).color(ACCENT)).on_hover_text(target).clicked() {
        sys::open(target);
    }
}

pub fn muted(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).color(MUTED).size(13.0));
}

pub fn page_title(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(26.0).strong());
    if !subtitle.is_empty() {
        muted(ui, subtitle);
    }
    ui.add_space(12.0);
}
