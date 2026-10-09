//! "Start with Windows": a per-user Run key pointing at this executable (no admin needed).

use super::{hidden, run};

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "LunimaPOCenter";

pub fn is_enabled() -> bool {
    if !cfg!(windows) {
        return false;
    }
    let mut cmd = hidden("reg.exe");
    cmd.args(["query", RUN_KEY, "/v", VALUE]);
    run(cmd).is_ok()
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let mut cmd = hidden("reg.exe");
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let data = format!("\"{}\" --tray", exe.display());
        cmd.args(["add", RUN_KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &data, "/f"]);
    } else {
        cmd.args(["delete", RUN_KEY, "/v", VALUE, "/f"]);
    }
    run(cmd).map(|_| ())
}
