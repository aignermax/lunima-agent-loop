//! Tray icon: status colour at a glance, quick actions in the menu.

use crate::collector::{Action, Handle};
use crate::sys::agent_service::Verb;
use crate::core::health::{self, Level};
use crate::ui::theme;
use crate::win;
use chrono::Local;
use std::cell::RefCell;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

struct Tray {
    icon: TrayIcon,
    pause: MenuItem,
    level: Option<Level>,
    tooltip: String,
    paused: bool,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

const ID_OPEN: &str = "open";
const ID_PAUSE: &str = "pause";
const ID_RESTART: &str = "restart";
const ID_QUIT: &str = "quit";

/// RGBA image of a status disc with a soft ring, `size`×`size` pixels.
pub fn status_icon_rgba(color: [u8; 3], size: u32) -> Vec<u8> {
    let c = size as f32 / 2.0;
    let (outer, inner) = (c - 0.5, c * 0.62);
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let edge = |r: f32| (r - d + 0.5).clamp(0.0, 1.0);
            let (ring, core) = (edge(outer), edge(inner));
            let alpha = (ring * 0.45 + core * 0.55).min(1.0);
            rgba.extend_from_slice(&[color[0], color[1], color[2], (alpha * 255.0) as u8]);
        }
    }
    rgba
}

fn icon_for(level: Level) -> Icon {
    let c = theme::level_color(level);
    Icon::from_rgba(status_icon_rgba([c.r(), c.g(), c.b()], 32), 32, 32).expect("valid icon")
}

/// Creates the tray icon and wires menu/click events on the current thread. A machine
/// without a tray (e.g. a Linux desktop without AppIndicator) just runs without one.
fn build(handle: &Handle) {
    let menu = Menu::new();
    let open = MenuItem::with_id(ID_OPEN, "Control Center öffnen", true, None);
    let pause = MenuItem::with_id(ID_PAUSE, "Alle Rollen pausieren", true, None);
    let restart = MenuItem::with_id(ID_RESTART, "Team-Agent neu starten", true, None);
    let quit = MenuItem::with_id(ID_QUIT, "Beenden", true, None);
    let _ = menu.append_items(&[&open, &PredefinedMenuItem::separator(), &pause, &restart, &PredefinedMenuItem::separator(), &quit]);
    let built = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Lunima Team — wird geprüft …")
        .with_icon(icon_for(Level::Pending))
        .with_menu_on_left_click(false)
        .build();
    let icon = match built {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Kein Tray-Symbol: {e}");
            return;
        }
    };
    TRAY.with(|t| *t.borrow_mut() = Some(Tray { icon, pause, level: None, tooltip: String::new(), paused: false }));

    let menu_handle = handle.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
        ID_OPEN => win::show_main(),
        ID_PAUSE => menu_handle.send(Action::PauseAll(!menu_handle.snapshot().all_paused())),
        ID_RESTART => menu_handle.send(Action::Agent(Verb::Restart)),
        ID_QUIT => std::process::exit(0),
        _ => {}
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, .. } | TrayIconEvent::DoubleClick { .. } = e {
            win::show_main();
        }
    }));
}

/// Windows: the tray lives on the UI thread; a thread timer keeps it current while the
/// window is hidden.
#[cfg(not(target_os = "linux"))]
pub fn install(handle: Handle) {
    build(&handle);
    win::start_timer(3000, move || sync(&handle));
}

/// Linux: tray-icon needs GTK initialised and its main loop running on the thread that
/// owns the icon — eframe (winit) doesn't run one, so the tray gets its own thread.
#[cfg(target_os = "linux")]
pub fn install(handle: Handle) {
    let spawned = std::thread::Builder::new().name("tray".into()).spawn(move || {
        if let Err(e) = gtk::init() {
            eprintln!("Kein Tray-Symbol (GTK nicht verfügbar): {e}");
            return;
        }
        build(&handle);
        gtk::glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            sync(&handle);
            gtk::glib::ControlFlow::Continue
        });
        gtk::main();
    });
    if let Err(e) = spawned {
        eprintln!("Kein Tray-Symbol: {e}");
    }
}

/// Brings icon, tooltip and the pause item in line with the current snapshot. A no-op on
/// threads without the tray (on Linux the UI thread calls it too).
pub fn sync(handle: &Handle) {
    let snap = handle.snapshot();
    let h = health::evaluate(&snap, Local::now());
    let paused = snap.all_paused();
    let tooltip = format!("Lunima Team — {}{}", h.headline, if h.subline.is_empty() { String::new() } else { format!("
{}", h.subline) });
    TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let Some(tray) = t.as_mut() else { return };
        if tray.level != Some(h.overall) {
            let _ = tray.icon.set_icon(Some(icon_for(h.overall)));
            tray.level = Some(h.overall);
        }
        if tray.tooltip != tooltip {
            let _ = tray.icon.set_tooltip(Some(&tooltip));
            tray.tooltip = tooltip;
        }
        if tray.paused != paused {
            tray.pause.set_text(if paused { "Alle Rollen fortsetzen" } else { "Alle Rollen pausieren" });
            tray.paused = paused;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_opaque_in_the_middle_and_clear_in_the_corner() {
        let px = status_icon_rgba([255, 0, 0], 32);
        assert_eq!(px.len(), 32 * 32 * 4);
        let alpha = |x: usize, y: usize| px[(y * 32 + x) * 4 + 3];
        assert_eq!(alpha(16, 16), 255);
        assert_eq!(alpha(0, 0), 0);
    }
}
