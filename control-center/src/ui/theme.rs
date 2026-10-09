//! Visual language: a calm dark theme, one indigo accent, traffic-light status colours.

use crate::core::health::Level;
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x0E, 0x11, 0x17);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x12, 0x16, 0x1F);
pub const CARD: Color32 = Color32::from_rgb(0x18, 0x1D, 0x29);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x1E, 0x24, 0x33);
pub const BORDER: Color32 = Color32::from_rgb(0x27, 0x2F, 0x41);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE9, 0xF0);
pub const MUTED: Color32 = Color32::from_rgb(0x8B, 0x94, 0xA8);
pub const ACCENT: Color32 = Color32::from_rgb(0x7C, 0x8C, 0xFF);
pub const OK: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);
pub const WARN: Color32 = Color32::from_rgb(0xFF, 0xB5, 0x47);
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0x5C, 0x6C);
pub const INFO: Color32 = Color32::from_rgb(0x5A, 0xB0, 0xFF);

pub const RADIUS: u8 = 12;
pub const CARD_PAD: i8 = 16;

pub fn level_color(level: Level) -> Color32 {
    match level {
        Level::Ok => OK,
        Level::Info => INFO,
        Level::Warn => WARN,
        Level::Error => ERROR,
        Level::Pending => MUTED,
    }
}

/// Same hue, low alpha — for pill and banner backgrounds.
pub fn tint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.visuals = visuals();
    ctx.set_style(style);
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(0x0B, 0x0E, 0x14);
    v.faint_bg_color = CARD_HOVER;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = tint(ACCENT, 90);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.window_stroke = Stroke::new(1.0, BORDER);
    let w = &mut v.widgets;
    for (state, fill) in [
        (&mut w.inactive, Color32::from_rgb(0x22, 0x29, 0x3A)),
        (&mut w.hovered, Color32::from_rgb(0x2B, 0x33, 0x48)),
        (&mut w.active, Color32::from_rgb(0x33, 0x3D, 0x57)),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.corner_radius = CornerRadius::same(8);
        state.bg_stroke = Stroke::new(1.0, BORDER);
        state.fg_stroke = Stroke::new(1.0, TEXT);
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = CornerRadius::same(8);
    v
}

pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(CARD_PAD))
}
