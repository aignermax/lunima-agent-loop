//! The eframe application: sidebar + pages; closing the window keeps it in the tray.

use crate::collector::{Action, Handle};
use crate::core::health::{self, Health, Level};
use crate::core::snapshot::{relative, Snapshot};
use crate::ui::{activity::ActivityPage, discovery, github, logs::LogsPage, overview, projects::ProjectsPage, rules::RulesPage, settings::SettingsPage, setup, team::TeamPage};
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
    Projects,
    Activity,
    Github,
    Settings,
    Rules,
    Logs,
    Setup,
    Discovery,
}

const PAGES: &[(Page, &str, &str)] = &[
    (Page::Overview, "🏠", "Übersicht"),
    (Page::Team, "👥", "Team"),
    (Page::Projects, "📦", "Projekte"),
    (Page::Activity, "📋", "Aktivität"),
    (Page::Github, "🐙", "GitHub"),
    (Page::Discovery, "🔭", "Discovery"),
    (Page::Settings, "⚙", "Einstellungen"),
    (Page::Rules, "📜", "Regeln"),
    (Page::Logs, "🗒", "Protokolle"),
    (Page::Setup, "🧭", "Einrichtung"),
];

const NOTICE_SECONDS: u64 = 6;

/// Window size on first start and the smallest one the layout works with (points).
pub const DEFAULT_SIZE: [f32; 2] = [1240.0, 820.0];
pub const MIN_SIZE: [f32; 2] = [900.0, 600.0];

/// eframe restores the size it saved last. Saved while the window was minimised, that is
/// 0×0, which eframe raises only to 64×64 — an unreadable window that the minimum size
/// doesn't fix, because Windows applies it only while the user drags the border.
/// Returns true once it has looked: only the first frame with a known, non-minimised size
/// counts, so a window manager that refuses the size can't make it resend every frame.
fn fix_tiny_window(ctx: &egui::Context) -> bool {
    let (rect, minimized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().minimized));
    let Some(rect) = rect.filter(|_| minimized != Some(true)) else { return false };
    if rect.width() < MIN_SIZE[0] - 1.0 || rect.height() < MIN_SIZE[1] - 1.0 {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(DEFAULT_SIZE[0], DEFAULT_SIZE[1])));
    }
    true
}

impl Page {
    /// `--page overview|team|projects|activity|github|discovery|settings|rules|logs|setup`
    pub fn from_arg(name: &str) -> Option<Self> {
        let all = [Page::Overview, Page::Team, Page::Projects, Page::Activity, Page::Github, Page::Discovery, Page::Settings, Page::Rules, Page::Logs, Page::Setup];
        let names = ["overview", "team", "projects", "activity", "github", "discovery", "settings", "rules", "logs", "setup"];
        names.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| all[i])
    }
}

pub struct App {
    handle: Handle,
    page: Page,
    activity: ActivityPage,
    team: TeamPage,
    projects: ProjectsPage,
    settings: SettingsPage,
    rules: RulesPage,
    logs: LogsPage,
    screenshot: Option<crate::screenshot::Screenshot>,
    /// the restored window size was checked (`fix_tiny_window`)
    window_checked: bool,
    updater: Option<crate::updater::Worker>,
    hide_requested: bool,
    quit_flush: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, handle: Handle, page: Page, screenshot: Option<crate::screenshot::Screenshot>) -> Self {
        theme::apply(&cc.egui_ctx);
        win::register(cc);
        if screenshot.is_none() {
            tray::install(handle.clone());
        }
        let updater = screenshot.is_none().then(|| crate::updater::Worker::start(handle.snapshot().paths.root.clone(),handle.clone()));
        Self { handle, page, activity: ActivityPage::default(), team: TeamPage::default(), projects: ProjectsPage::default(), settings: SettingsPage::default(), rules: RulesPage::default(), logs: LogsPage::default(), screenshot, window_checked: false, updater, hide_requested: false, quit_flush: None }
    }

    fn sidebar(&mut self, ctx: &egui::Context, s: &Snapshot, h: &Health) {
        egui::SidePanel::left("nav").exact_width(220.0).resizable(false).frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(egui::Margin::same(16))).show(ctx, |ui| {
            if self.updater.as_ref().is_some_and(|u|u.blocking()) || tray::quit_requested() || self.quit_flush.is_some() {ui.disable();}
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
                let badge = if *page == Page::Team && s.unread_questions() > 0 { format!("   ● {}", s.unread_questions()) } else { String::new() };
                let text = RichText::new(format!("{icon}   {label}{badge}")).size(15.0).color(if selected { theme::TEXT } else { MUTED });
                let button = egui::Button::new(text).fill(if selected { theme::tint(theme::ACCENT, 45) } else { egui::Color32::TRANSPARENT }).stroke(egui::Stroke::NONE).min_size(egui::vec2(ui.available_width(), 36.0));
                if ui.add(button).clicked() {
                    if self.page == Page::Settings && *page != Page::Settings {
                        self.settings.flush(s, &self.handle);
                    }
                    if self.page == Page::Projects && *page != Page::Projects {
                        self.projects.flush(ctx, &self.handle);
                    }
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
    ui.label(RichText::new(format!("GitHub & Werkzeuge {slow}")).size(11.5).color(MUTED));
    if ui.small_button("⟳ Jetzt prüfen").clicked() {
        handle.send(Action::RefreshAll);
    }
    ui.add_space(4.0);
    ui.separator();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // closing hides the window into the tray; a screenshot run has no tray, so it quits
        let closing = self.screenshot.is_none() && ctx.input(|i| i.viewport().close_requested());
        if closing || tray::quit_requested() {
            self.settings.flush(&self.handle.snapshot(), &self.handle);
            self.projects.flush(ctx, &self.handle);
            if closing { ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose); self.hide_requested = true; }
        }
        if !self.window_checked {
            self.window_checked = fix_tiny_window(ctx);
        }
        self.projects.tick(ctx, &self.handle);
        if let Some(wait) = &self.quit_flush {
            match wait.try_recv() {
                Ok(Ok(())) => std::process::exit(0),
                Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.quit_flush = None; tray::cancel_quit(); self.hide_requested = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => { ctx.request_repaint_after(Duration::from_millis(50)); }
            }
        }
        if tray::quit_requested() && self.quit_flush.is_none() {
            if self.projects.has_save_error() || !self.settings.can_update() {
                tray::cancel_quit(); self.hide_requested = false;
            } else if self.projects.can_update() {
                let (done, wait) = std::sync::mpsc::channel();
                self.handle.send(Action::Flush(done)); self.quit_flush = Some(wait);
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        let s = self.handle.snapshot();
        let h = health::evaluate(&s, Local::now());
        if let Some(updater)=&self.updater {
            if updater.banner(ctx) {
                self.settings.flush(&s,&self.handle);
                self.projects.flush(ctx, &self.handle);
                if self.settings.can_update() && self.projects.can_update() {updater.install();} else {updater.save_error();}
            }
        }
        tray::sync(&self.handle);
        if self.page == Page::Projects || !self.projects.can_update() { self.projects.status(ctx); }
        self.sidebar(ctx, &s, &h);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(28, 22))).show(ctx, |ui| {
            if self.updater.as_ref().is_some_and(|u|u.blocking()) || tray::quit_requested() || self.quit_flush.is_some() {ui.disable();}
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // forms and lists read best at a limited width; the Team page's two columns of
                // role cards with chats use whatever the window offers
                let max = if self.page == Page::Team { f32::INFINITY } else { 1080.0 };
                ui.set_max_width(ui.available_width().min(max));
                match self.page {
                    Page::Overview => {
                        if let Some(target) = overview::show(ui, &s, &h, &self.handle) {
                            self.page = target;
                        }
                    }
                    Page::Team => self.team.show(ui, &s, &self.handle),
                    Page::Projects => self.projects.show(ui, &s, &self.handle),
                    Page::Activity => self.activity.show(ui, &s),
                    Page::Github => github::show(ui, &s),
                    Page::Discovery => discovery::show(ui, &s, &self.handle),
                    Page::Settings => {
                        if self.settings.show(ui, &s, &self.handle) {
                            self.page = Page::Setup;
                        }
                    }
                    Page::Setup => setup::show(ui, &s, &self.handle),
                    Page::Rules => self.rules.show(ui, &s, &self.handle),
                    Page::Logs => self.logs.show(ui, &s),
                }
            });
        });
        // Process the final frame's input before hiding; its last keystroke may
        // have created a new debounced project edit.
        if self.hide_requested && !tray::quit_requested() {
            if self.projects.has_save_error() || !self.settings.can_update() { self.hide_requested = false; }
            else if self.projects.can_update() { self.hide_requested = false; win::hide_main(); }
        }
        self.notice(ctx);
        if let Some(shot) = &mut self.screenshot {
            shot.tick(ctx);
        }
        let working = s.heartbeats.values().any(|hb| hb.state == "working");
        let poll = if h.overall == Level::Pending || working { 1 } else { 5 };
        ctx.request_repaint_after(Duration::from_secs(poll));
    }
}
