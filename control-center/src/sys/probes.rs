//! The real answers for the setup assistant, and a terminal for interactive logins.

use super::{hidden, run_output_timeout};
use crate::core::setup::Probes;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

pub struct System;

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string()
}

impl Probes for System {
    fn version(&self, tool: &str) -> Result<String, String> {
        let path = team_core::proc::which(tool).ok_or_else(|| format!("{tool} nicht im PATH"))?;
        let mut cmd = hidden(path);
        cmd.arg("--version");
        let out = run_output_timeout(cmd, TIMEOUT)?;
        if out.status.success() { Ok(first_line(&out.stdout)) } else { Err(first_line(&out.stderr)) }
    }

    fn gh_login(&self) -> Result<(), String> {
        super::github::auth_status()
    }

    fn claude_cli(&self) -> Option<String> {
        team_core::claude::find_cli().map(|p| p.display().to_string())
    }

    /// `claude auth status` prints JSON with `loggedIn`.
    fn claude_login(&self) -> Result<bool, String> {
        let cli = team_core::claude::find_cli().ok_or("claude nicht gefunden")?;
        let mut cmd = hidden(cli);
        cmd.args(["auth", "status"]);
        for var in team_core::provider::CLAUDE_AUTH_VARS {
            cmd.env_remove(var); // the agent runs on the subscription, so ask about that
        }
        let out = run_output_timeout(cmd, TIMEOUT)?;
        parse_auth_status(&String::from_utf8_lossy(&out.stdout))
    }
}

pub fn parse_auth_status(text: &str) -> Result<bool, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| "Antwort von `claude auth status` unverständlich — CLI aktualisieren".to_string())?;
    v.get("loggedIn").and_then(|b| b.as_bool()).ok_or_else(|| "loggedIn fehlt in `claude auth status`".into())
}

/// Opens a visible terminal that runs `command` and stays open afterwards.
pub fn open_terminal(command: &str) -> Result<(), String> {
    if cfg!(windows) {
        // `start` opens a new console window; /k keeps it open for the user
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "Lunima Team Einrichtung", "cmd", "/k", command]);
        return cmd.spawn().map(|_| ()).map_err(|e| e.to_string());
    }
    let script = format!("{command}; echo; echo 'Fertig — Fenster kann geschlossen werden.'; exec \"${{SHELL:-sh}}\"");
    let terminals: [(&str, &[&str]); 4] = [("x-terminal-emulator", &["-e"]), ("gnome-terminal", &["--"]), ("konsole", &["-e"]), ("xterm", &["-e"])];
    for (term, args) in terminals {
        if team_core::proc::which(term).is_some() {
            let mut cmd = std::process::Command::new(term);
            cmd.args(args).args(["sh", "-c", &script]);
            return cmd.spawn().map(|_| ()).map_err(|e| e.to_string());
        }
    }
    Err(format!("Kein Terminal gefunden — bitte selbst ausführen: {command}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_status_json() {
        assert_eq!(parse_auth_status(r#"{"loggedIn": true, "authMethod": "claude.ai"}"#), Ok(true));
        assert_eq!(parse_auth_status(r#"{"loggedIn": false}"#), Ok(false));
        assert!(parse_auth_status("Usage: claude [options]").is_err());
    }
}
