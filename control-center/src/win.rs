//! Native window plumbing egui can't do while the window is hidden: show/hide by handle,
//! a UI-thread timer, and the single-instance guard.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

pub const TITLE: &str = "Lunima PO Control Center";

static HWND: AtomicIsize = AtomicIsize::new(0);
static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

/// Remembers the main window (from eframe's creation context) and the egui context.
pub fn register(cc: &eframe::CreationContext<'_>) {
    let _ = CTX.set(cc.egui_ctx.clone());
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = cc.window_handle() {
        if let RawWindowHandle::Win32(w) = h.as_raw() {
            HWND.store(w.hwnd.get(), Ordering::SeqCst);
        }
    }
}

/// Wakes the UI after background updates (no-op until the window exists).
pub fn request_repaint() {
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND as RawHwnd};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, SetTimer, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW};

    thread_local! {
        static TIMER_FN: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn show(hwnd: RawHwnd) {
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }

    pub fn show_main() {
        let hwnd = HWND.load(Ordering::SeqCst);
        if hwnd != 0 {
            show(hwnd as RawHwnd);
        }
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }

    pub fn hide_main() {
        let hwnd = HWND.load(Ordering::SeqCst);
        if hwnd != 0 {
            unsafe { ShowWindow(hwnd as RawHwnd, SW_HIDE) };
        }
    }

    /// False if another instance already runs — that one is brought to the front instead.
    pub fn claim_single_instance() -> bool {
        let name = wide("Local\\LunimaPOControlCenter");
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if !handle.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
                let other = FindWindowW(std::ptr::null(), wide(TITLE).as_ptr());
                if !other.is_null() {
                    show(other);
                }
                return false;
            }
        }
        true
    }

    unsafe extern "system" fn on_timer(_: RawHwnd, _: u32, _: usize, _: u32) {
        TIMER_FN.with(|f| {
            if let Some(f) = f.borrow().as_ref() {
                f();
            }
        });
    }

    /// Thread timer: WM_TIMER is dispatched by the event loop even with the window hidden.
    pub fn start_timer(ms: u32, f: impl Fn() + 'static) {
        TIMER_FN.with(|slot| *slot.borrow_mut() = Some(Box::new(f)));
        unsafe { SetTimer(std::ptr::null_mut(), 0, ms, Some(on_timer)) };
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn show_main() {
        if let Some(ctx) = super::CTX.get() {
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
        }
    }
    pub fn hide_main() {
        if let Some(ctx) = super::CTX.get() {
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(false));
        }
    }
    pub fn claim_single_instance() -> bool {
        true
    }
    pub fn start_timer(_ms: u32, _f: impl Fn() + 'static) {}
}

pub use imp::{claim_single_instance, hide_main, show_main, start_timer};
