//! Agent sessions on a hidden Windows desktop.
//!
//! A session runs the repo's tests, and GUI tests (Tk, WPF, Avalonia …) open real windows:
//! on Windows they appeared on the user's screen and took the focus. CI runs them under an
//! invisible display (xvfb); this is the Windows counterpart. A desktop of its own in the
//! user's window station is never switched to, so its windows are never shown, while every
//! window and GDI call works as usual.
//!
//! The standard library cannot pick a desktop (`STARTUPINFO.lpDesktop`), so the agent binary
//! is its own launcher, in two stages:
//!
//! 1. `lunima-team-agent __on-hidden-desktop <program> <args …>` creates or opens the desktop
//!    and starts itself again on it with the very same command line (`CreateProcessW`, the
//!    standard handles passed on).
//! 2. The second stage runs `<program> <args …>` through `std::process::Command`, exactly as
//!    the agent would have, so `.cmd` CLIs and argument quoting behave the same. Everything it
//!    starts inherits the hidden desktop.
//!
//! Both stages are descendants of the process the agent spawned, so `proc::kill_tree` still
//! reaches the whole tree. Where the launcher is missing (tests, the Control Center without
//! the agent binary next to it), or off (`resources.show_session_windows`), or not on
//! Windows, the program starts directly as before.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// First argument that makes the agent binary the launcher (arguments passed on as such).
pub const ARG: &str = "__on-hidden-desktop";
/// The same for a program whose arguments follow as one verbatim string (`raw_arg`).
pub const ARG_RAW: &str = "__on-hidden-desktop-raw";
/// Prints `<window station>\<desktop>` of the running process (tests and diagnosis).
pub const ARG_NAME: &str = "__desktop-name";
/// The hidden desktop's name in the user's window station.
pub const DESKTOP: &str = "LunimaAgentSessions";
/// Marks the second stage, which already runs on the hidden desktop.
const STAGE_ENV: &str = "LUNIMA_HIDDEN_DESKTOP_STAGE";

static HIDDEN: AtomicBool = AtomicBool::new(false);

/// Switches the hidden desktop on or off for sessions started from now on (the agent sets it
/// from `resources.show_session_windows`; off by default, e.g. in the Control Center).
pub fn set_hidden(on: bool) {
    HIDDEN.store(on, Ordering::Relaxed);
}

pub fn hidden() -> bool {
    HIDDEN.load(Ordering::Relaxed)
}

/// `lunima-team-agent` next to the running program.
fn launcher() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(if cfg!(windows) { "lunima-team-agent.exe" } else { "lunima-team-agent" });
    candidate.is_file().then_some(candidate)
}

/// Like `proc::command(program)`, but started on the hidden desktop when that is on.
/// Arguments are added with `.arg`/`.args` as usual.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    wrap(active_launcher().as_deref(), program.as_ref(), ARG)
}

/// `command` for a program whose arguments are added with one `raw_arg` call (`cmd /S /C …`).
pub fn command_raw(program: impl AsRef<OsStr>) -> Command {
    wrap(active_launcher().as_deref(), program.as_ref(), ARG_RAW)
}

fn active_launcher() -> Option<PathBuf> {
    if cfg!(windows) && hidden() {
        launcher()
    } else {
        None
    }
}

fn wrap(launcher: Option<&Path>, program: &OsStr, marker: &str) -> Command {
    match launcher {
        Some(l) => {
            let mut c = crate::proc::command(l);
            c.arg(marker).arg(program);
            c
        }
        None => crate::proc::command(program),
    }
}

/// The launcher's entry point, called first by the agent's `main`: `Some(exit code)` when it
/// was started as the launcher. Reads the raw arguments: a passed-on argument need not be UTF-8.
pub fn launch_main() -> Option<i32> {
    dispatch(std::env::args_os().nth(1).as_deref().and_then(OsStr::to_str))
}

fn dispatch(first: Option<&str>) -> Option<i32> {
    match first {
        Some(ARG_NAME) => {
            println!("{}", current_name().unwrap_or_default());
            Some(0)
        }
        Some(a) if a == ARG || a == ARG_RAW => Some(launch(a == ARG_RAW)),
        _ => None,
    }
}

fn launch(raw: bool) -> i32 {
    #[cfg(windows)]
    if std::env::var_os(STAGE_ENV).is_none() {
        match win::relaunch_on_hidden_desktop() {
            Ok(code) => return code,
            // the work matters more than the window: run it visibly, as before
            Err(e) => eprintln!("lunima-team-agent: versteckter Desktop nicht verfügbar ({e}) — läuft sichtbar"),
        }
    }
    run_program(raw)
}

/// The second stage: the program itself, its output on the inherited standard handles.
fn run_program(raw: bool) -> i32 {
    let args: Vec<OsString> = std::env::args_os().skip(2).collect();
    let Some(program) = args.first() else {
        eprintln!("lunima-team-agent: {ARG} ohne Programm");
        return 2;
    };
    let mut cmd = Command::new(program);
    cmd.env_remove(STAGE_ENV);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        if raw {
            match win::command_line_tail(3) {
                Some(tail) => {
                    cmd.raw_arg(tail);
                }
                None => {
                    eprintln!("lunima-team-agent: Befehlszeile nicht lesbar");
                    return 2;
                }
            }
        } else {
            cmd.args(&args[1..]);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = raw;
        cmd.args(&args[1..]);
    }
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("{}: {e}", program.to_string_lossy());
            127
        }
    }
}

/// `<window station>\<desktop>` of this process; `None` off Windows.
pub fn current_name() -> Option<String> {
    #[cfg(windows)]
    return win::current_name();
    #[cfg(not(windows))]
    None
}

/// What follows the first `n` arguments of a Windows command line, leading blanks removed.
/// Only for the launcher's own arguments (its path, the marker, the program path): none of
/// them contains a quote, so an argument is either `"…"` or runs up to the next blank.
fn skip_args(line: &[u16], n: usize) -> Option<&[u16]> {
    let blank = |c: &u16| *c == b' ' as u16 || *c == b'\t' as u16;
    let quote = b'"' as u16;
    let mut rest = line;
    for _ in 0..n {
        rest = &rest[rest.iter().position(|c| !blank(c))?..];
        let end = if rest[0] == quote { rest[1..].iter().position(|c| *c == quote)? + 2 } else { rest.iter().position(blank).unwrap_or(rest.len()) };
        rest = &rest[end..];
    }
    Some(&rest[rest.iter().position(|c| !blank(c)).unwrap_or(rest.len())..])
}

#[cfg(windows)]
mod win {
    use super::{skip_args, DESKTOP, STAGE_ENV};
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, SetHandleInformation, GENERIC_ALL, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    use windows_sys::Win32::System::Environment::GetCommandLineW;
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, CreateDesktopW, GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW, OpenDesktopW, HDESK, UOI_NAME};
    use windows_sys::Win32::System::Threading::{CreateProcessW, GetCurrentThreadId, GetExitCodeProcess, WaitForSingleObject, CREATE_NO_WINDOW, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn object_name(h: HANDLE) -> Option<String> {
        let mut buf = [0u16; 256];
        let mut needed = 0u32;
        // SAFETY: the buffer and its byte length match; `needed` is a valid out pointer
        let ok = unsafe { GetUserObjectInformationW(h, UOI_NAME, buf.as_mut_ptr().cast(), std::mem::size_of_val(&buf) as u32, &mut needed) };
        if ok == 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn current_name() -> Option<String> {
        // SAFETY: plain queries; neither handle is owned by us, so neither is closed
        let (station, desk) = unsafe { (GetProcessWindowStation(), GetThreadDesktop(GetCurrentThreadId())) };
        Some(format!("{}\\{}", object_name(station as HANDLE)?, object_name(desk as HANDLE)?))
    }

    /// This process's command line, without the terminating NUL.
    fn command_line() -> Vec<u16> {
        // SAFETY: GetCommandLineW returns a NUL-terminated string that lives as long as the process
        unsafe {
            let p = GetCommandLineW();
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            std::slice::from_raw_parts(p, len).to_vec()
        }
    }

    pub fn command_line_tail(n: usize) -> Option<OsString> {
        let line = command_line();
        skip_args(&line, n).map(OsString::from_wide)
    }

    struct Desk(HDESK);
    impl Drop for Desk {
        fn drop(&mut self) {
            // SAFETY: a handle we opened; the desktop lives on while processes use it
            unsafe { CloseDesktop(self.0) };
        }
    }

    /// Starts this process again with the same command line on the hidden desktop and
    /// returns its exit code once it ends.
    pub fn relaunch_on_hidden_desktop() -> Result<i32, String> {
        // SAFETY: plain query of this process's window station (not owned, not closed)
        let station = object_name(unsafe { GetProcessWindowStation() } as HANDLE).ok_or("Fensterstation unbekannt")?;
        let name = wide(DESKTOP);
        // SAFETY: NUL-terminated name; null device/mode/security = defaults (the user's own access)
        let mut desk = unsafe { CreateDesktopW(name.as_ptr(), std::ptr::null(), std::ptr::null(), 0, GENERIC_ALL, std::ptr::null()) };
        if desk.is_null() {
            // SAFETY: as above
            desk = unsafe { OpenDesktopW(name.as_ptr(), 0, 0, GENERIC_ALL) };
        }
        if desk.is_null() {
            return Err(format!("CreateDesktop: {}", std::io::Error::last_os_error()));
        }
        let _desk = Desk(desk);
        let full = wide(&format!("{station}\\{DESKTOP}"));
        let mut line = command_line();
        line.push(0);
        // SAFETY: an all-zero STARTUPINFOW is its documented empty state
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = full.as_ptr() as *mut u16;
        si.dwFlags = STARTF_USESTDHANDLES;
        for (slot, which) in [(&mut si.hStdInput, STD_INPUT_HANDLE), (&mut si.hStdOutput, STD_OUTPUT_HANDLE), (&mut si.hStdError, STD_ERROR_HANDLE)] {
            // SAFETY: plain query of this process's standard handle
            let h = unsafe { GetStdHandle(which) };
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                // SAFETY: a valid handle of this process; only its inherit flag changes
                unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            }
            *slot = h;
        }
        // the second stage inherits the environment, marker included (this process only waits)
        std::env::set_var(STAGE_ENV, "1");
        // SAFETY: an all-zero PROCESS_INFORMATION is a valid out value
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `line` is a mutable NUL-terminated buffer, `si` and `full` outlive the call
        let ok = unsafe { CreateProcessW(std::ptr::null(), line.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 1, CREATE_NO_WINDOW, std::ptr::null(), std::ptr::null(), &si, &mut pi) };
        if ok == 0 {
            let e = std::io::Error::last_os_error();
            std::env::remove_var(STAGE_ENV);
            return Err(format!("CreateProcess: {e}"));
        }
        let mut code = 1u32;
        // SAFETY: handles returned by CreateProcessW, each closed once
        unsafe {
            CloseHandle(pi.hThread);
            WaitForSingleObject(pi.hProcess, INFINITE);
            GetExitCodeProcess(pi.hProcess, &mut code);
            CloseHandle(pi.hProcess);
        }
        Ok(code as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn skip_args_keeps_the_raw_tail_verbatim() {
        let line = w(r#""C:\Program Files\a\lunima-team-agent.exe" __on-hidden-desktop-raw cmd /S /C "echo "a b"> "x y.txt"""#);
        assert_eq!(String::from_utf16(skip_args(&line, 3).unwrap()).unwrap(), r#"/S /C "echo "a b"> "x y.txt"""#);
        let quoted_program = w(r#"agent.exe __on-hidden-desktop-raw "C:\Windows\system32\cmd.exe"	 /C exit 3"#);
        assert_eq!(String::from_utf16(skip_args(&quoted_program, 3).unwrap()).unwrap(), "/C exit 3");
        assert_eq!(skip_args(&w("agent.exe __on-hidden-desktop-raw cmd"), 3).unwrap(), &[] as &[u16]);
        assert!(skip_args(&w("agent.exe __on-hidden-desktop-raw"), 3).is_none(), "program missing");
        assert!(skip_args(&w(r#""unterminated a b"#), 1).is_none());
    }

    #[test]
    fn wraps_only_with_a_launcher() {
        let direct = wrap(None, OsStr::new("claude.cmd"), ARG);
        assert_eq!(direct.get_program(), "claude.cmd");
        assert_eq!(direct.get_args().count(), 0);
        let mut wrapped = wrap(Some(Path::new("lunima-team-agent.exe")), OsStr::new("claude.cmd"), ARG);
        wrapped.arg("-p");
        assert_eq!(wrapped.get_program(), "lunima-team-agent.exe");
        assert_eq!(wrapped.get_args().collect::<Vec<_>>(), [ARG, "claude.cmd", "-p"]);
    }

    #[test]
    fn off_by_default() {
        // the Control Center and the tests never set it: their programs start directly
        assert!(!hidden());
        assert_eq!(command("git").get_program(), "git");
    }

    #[test]
    fn ordinary_commands_are_not_the_launcher() {
        assert_eq!(dispatch(Some("run")), None);
        assert_eq!(dispatch(None), None);
    }
}
