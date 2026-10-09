//! Lunima PO Control Center — status, health, activity, settings and rules of the
//! Product-Owner loop, with a tray icon.
//!
//! Usage: `lunima-po [--root <loop folder>] [--tray] [--page <name>]`
//! (`--tray` starts hidden in the tray; pages: overview, activity, github, settings, rules, logs)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod collector;
mod core;
mod sys;
mod team_ops;
mod tray;
mod ui;
mod win;

use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;

struct Args {
    root: Option<PathBuf>,
    start_hidden: bool,
    page: app::Page,
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args { root: None, start_hidden: false, page: app::Page::Overview };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => parsed.root = args.next().map(PathBuf::from),
            "--tray" => parsed.start_hidden = true,
            "--page" => parsed.page = args.next().and_then(|p| app::Page::from_arg(&p)).unwrap_or(parsed.page),
            _ => {}
        }
    }
    parsed
}

fn app_icon() -> egui::IconData {
    let c = ui::theme::ACCENT;
    egui::IconData { rgba: tray::status_icon_rgba([c.r(), c.g(), c.b()], 64), width: 64, height: 64 }
}

fn main() -> eframe::Result<()> {
    if !win::claim_single_instance() {
        return Ok(());
    }
    let args = parse_args();
    let root = core::root::resolve_root(args.root);
    let cli = core::root::find_cli(&root);
    let on_change: Arc<dyn Fn() + Send + Sync> = Arc::new(|| win::request_repaint());
    let handle = collector::start(root, cli, on_change);
    let page = args.page;

    let viewport = egui::ViewportBuilder::default()
        .with_title(win::TITLE)
        .with_inner_size([1240.0, 820.0])
        .with_min_inner_size([900.0, 600.0])
        .with_icon(app_icon())
        .with_visible(!args.start_hidden);
    let options = eframe::NativeOptions { viewport, persist_window: true, ..Default::default() };
    eframe::run_native(win::TITLE, options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, handle, page)))))
}
