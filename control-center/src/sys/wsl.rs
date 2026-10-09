//! The issue agent's daemons are systemd user units inside WSL; reached via `wsl.exe`.

use super::{hidden, run};
use std::collections::BTreeMap;

/// (role, systemd unit) of the issue agent.
pub const UNITS: &[(&str, &str)] = &[("coder", "aia-coder"), ("qa", "aia-qa"), ("pr-feedback", "aia-prfeedback")];

pub fn unit_of(role: &str) -> Option<&'static str> {
    UNITS.iter().find(|(r, _)| *r == role).map(|(_, u)| *u)
}

fn wsl(args: &[&str]) -> Result<String, String> {
    let mut cmd = hidden("wsl.exe");
    cmd.arg("-e").args(args);
    run(cmd)
}

/// `systemctl --user is-active` for every unit: role → active/inactive/failed/…
pub fn unit_states() -> Result<BTreeMap<String, String>, String> {
    let mut args = vec!["systemctl", "--user", "is-active"];
    args.extend(UNITS.iter().map(|(_, u)| *u));
    // is-active exits non-zero when any unit is inactive — the output is still valid
    let out = match wsl(&args) {
        Ok(o) => o,
        Err(e) if e.lines().count() == UNITS.len() => e,
        Err(e) => return Err(e),
    };
    Ok(parse_states(&out))
}

pub fn parse_states(out: &str) -> BTreeMap<String, String> {
    UNITS.iter().zip(out.lines()).map(|((role, _), state)| (role.to_string(), state.trim().to_string())).collect()
}

/// start / stop / restart one unit.
pub fn unit_action(verb: &str, unit: &str) -> Result<(), String> {
    wsl(&["systemctl", "--user", verb, unit]).map(|_| ())
}

/// Version of the Claude CLI the daemons use (their PATH puts ~/.npm-global/bin first).
pub fn claude_version() -> Result<String, String> {
    let out = wsl(&["bash", "-lc", "(~/.npm-global/bin/claude --version || claude --version) 2>/dev/null | head -1"])?;
    let v = out.split_whitespace().next().unwrap_or("").to_string();
    if v.is_empty() { Err("claude nicht gefunden".into()) } else { Ok(v) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_is_active_lines_to_roles() {
        let s = parse_states("active\ninactive\nfailed\n");
        assert_eq!(s["coder"], "active");
        assert_eq!(s["qa"], "inactive");
        assert_eq!(s["pr-feedback"], "failed");
    }

    #[test]
    fn unit_lookup() {
        assert_eq!(unit_of("qa"), Some("aia-qa"));
        assert_eq!(unit_of("po"), None);
    }
}
