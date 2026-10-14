//! "Start the Control Center with the session" (in the tray): a per-user Run key on
//! Windows, an XDG autostart entry on Linux. No admin rights needed.

use super::{hidden, run};
use std::path::PathBuf;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "LunimaPOCenter";

fn desktop_file() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("autostart").join("lunima-po.desktop"))
}

pub fn desktop_entry(exe: &std::path::Path) -> String {
    format!("[Desktop Entry]\nType=Application\nName=Lunima Team Control Center\nExec=\"{}\" --tray\nX-GNOME-Autostart-enabled=true\n", exe.display())
}

pub fn is_enabled() -> bool {
    if cfg!(windows) {
        let mut cmd = hidden("reg.exe");
        cmd.args(["query", RUN_KEY, "/v", VALUE]);
        return run(cmd).is_ok();
    }
    desktop_file().is_some_and(|p| p.is_file())
}

/// Registers `"exe" --tray`.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if !cfg!(windows) {
        let path = desktop_file().ok_or("HOME fehlt")?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(path.parent().expect("has parent")).map_err(|e| e.to_string())?;
        return std::fs::write(path, desktop_entry(&exe)).map_err(|e| e.to_string());
    }
    let mut cmd = hidden("reg.exe");
    if enabled {
        let data = format!("\"{}\" --tray", exe.display());
        cmd.args(["add", RUN_KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &data, "/f"]);
    } else {
        cmd.args(["delete", RUN_KEY, "/v", VALUE, "/f"]);
    }
    run(cmd).map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_entry_starts_in_the_tray() {
        let e = super::desktop_entry(std::path::Path::new("/usr/bin/lunima-po"));
        assert!(e.contains("Exec=\"/usr/bin/lunima-po\" --tray"));
    }
}
