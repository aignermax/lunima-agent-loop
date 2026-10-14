//! Is a pid a running `lunima-team-agent`? (pid files survive crashes, pids get reused)

/// The process exists and its executable is the team agent.
pub fn is_agent(pid: u32) -> bool {
    image_name(pid).is_some_and(|n| n.to_lowercase().contains("lunima-team-agent"))
}

#[cfg(windows)]
fn image_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let named = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        (alive && named).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(not(windows))]
fn image_name(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv0 = cmdline.split(|b| *b == 0).next()?;
    Some(String::from_utf8_lossy(argv0).into_owned())
}

/// Ends the agent process (Linux stop without systemd).
pub fn terminate(pid: u32) {
    if !is_agent(pid) {
        return;
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    }
    #[cfg(windows)]
    {
        let _ = team_core::proc::command("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_process_is_not_the_agent() {
        assert!(image_name(std::process::id()).is_some());
        assert!(!is_agent(std::process::id()));
        assert!(!is_agent(4_294_967));
    }
}
