//! The eframe application: sidebar + pages; closing the window keeps it in the tray.

use crate::collector::{Action, Handle};
use crate::core::health::{self, Health, Level};
use crate::core::snapshot::{relative, Snapshot};
use crate::ui::{activity::ActivityPage, github, logs::LogsPage, overview, rules::RulesPage, settings::SettingsPage, team::TeamPage};
use crate::ui::theme::{self, MUTED};
use crate::ui::widgets::level_dot;
use crate::{tray, win};
use chrono::Local;
use eframe::egui::{self, RichText};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Overview,
    Team,
    Activity,
    Github,
    Settings,
    Rules,
    Logs,
}

const PAGES: &[(Page, &str, &str)] = &[
    (Page::Overview, "🏠", "Übersicht"),
    (Page::Team, "👥", "Team"),
    (Page::Activity, "📋", "Aktivität"),
    (Page::Github, "🐙", "GitHub"),
    (Page::Settings, "⚙", "Einstellungen"),
    (Page::Rules, "📜", "Regeln"),
    (Page::Logs, "🗒", "Protokolle"),
];

const NOTICE_SECONDS: u64 = 6;

impl Page {
    /// `--page overview|activity|github|settings|rules|logs`
    pub fn from_arg(name: &str) -> Option<Self> {
        let all = [Page::Overview, Page::Team, Page::Activity, Page::Github, Page::Settings, Page::Rules, Page::Logs];
        let names = ["overview", "team", "activity", "github", "settings", "rules", "logs"];
        names.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| all[i])
    }
}

pub struct App {
    handle: Handle,
    page: Page,
    activity: ActivityPage,
    team: TeamPage,
    settings: SettingsPage,
    rules: RulesPage,
    logs: LogsPage,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, handle: Handle, page: Page) -> Self {
        theme::apply(&cc.egui_ctx);
        win::register(cc);
        tray::install(handle.clone());
        Self { handle, page, activity: ActivityPage::default(), team: TeamPage::default(), settings: SettingsPage::default(), rules: RulesPage::default(), logs: LogsPage::default() }
    }

    fn sidebar(&mut self, ctx: &egui::Context, s: &Snapshot, h: &Health) {
        egui::SidePanel::left("nav").exact_width(220.0).resizable(false).frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(egui::Margin::same(16))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                level_dot(ui, h.overall, 7.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Lunima Team").size(18.0).strong());
                    ui.label(RichText::new("Control Center").size(12.0).color(MUTED));
                });
            });
            ui.add_space(18.0);
            for (page, icon, label) in PAGES {
                let selected = self.page == *page;
                let text = RichText::new(format!("{icon}   {label}")).size(15.0).color(if selected { theme::TEXT } else { MUTED });
                let button = egui::Button::new(text).fill(if selected { theme::tint(theme::ACCENT, 45) } else { egui::Color32::TRANSPARENT }).stroke(egui::Stroke::NONE).min_size(egui::vec2(ui.available_width(), 36.0));
                if ui.add(button).clicked() {
                    self.page = *page;
                }
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| footer(ui, s, &self.handle));
        });
    }

    fn notice(&self, ctx: &egui::Context) {
        let notice = self.handle.shared.notice.lock().expect("notice lock").clone();
        let busy = self.handle.shared.busy.lock().expect("busy lock").clone();
        let (text, color) = match (busy, notice) {
            (Some(_), _) => ("Wird ausgeführt …".to_string(), theme::INFO),
            (None, Some(n)) if n.at.elapsed() < Duration::from_secs(NOTICE_SECONDS) => (n.text, if n.ok { theme::OK } else { theme::ERROR }),
            _ => return,
        };
        egui::Area::new(egui::Id::new("toast")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-20.0, -20.0)).show(ctx, |ui| {
            egui::Frame::new().fill(theme::CARD).stroke(egui::Stroke::new(1.0, color)).corner_radius(egui::CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                ui.set_max_width(420.0);
                ui.horizontal(|ui| {
                    crate::ui::widgets::dot(ui, color, 4.0);
                    ui.add(egui::Label::new(RichText::new(text)).wrap());
                });
            });
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn footer(ui: &mut egui::Ui, s: &Snapshot, handle: &Handle) {
    let now = Local::now();
    ui.label(RichText::new(format!("Dateien {}", relative(s.refreshed, now))).size(11.5).color(MUTED));
    let slow = s.slow_refreshed.map(|t| relative(t, now)).unwrap_or_else(|| "läuft …".into());
    ui.label(RichText::new(format!("GitHub & Zeitplan {slow}")).size(11.5).color(MUTED));
    if ui.small_button("⟳ Jetzt prüfen").clicked() {
        handle.send(Action::RefreshAll);
    }
    ui.add_space(4.0);
    ui.separator();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            win::hide_main();
        }
        let s = self.handle.snapshot();
        let h = health::evaluate(&s, Local::now());
        tray::sync(&self.handle);
        self.sidebar(ctx, &s, &h);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(28, 22))).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(1080.0));
                match self.page {
                    Page::Overview => overview::show(ui, &s, &h, &self.handle),
                    Page::Team => self.team.show(ui, &s, &self.handle),
                    Page::Activity => self.activity.show(ui, &s),
                    Page::Github => github::show(ui, &s),
                    Page::Settings => self.settings.show(ui, &s, &self.handle),
                    Page::Rules => self.rules.show(ui, &s, &self.handle),
                    Page::Logs => self.logs.show(ui, &s),
                }
            });
        });
        self.notice(ctx);
        let poll = if h.overall == Level::Pending || s.loop_running { 1 } else { 5 };
        ctx.request_repaint_after(Duration::from_secs(poll));
    }
}
