//! The issue agent's daemons are systemd user units inside WSL; reached via `wsl.exe`.
//! Every call has a timeout: a cold-booting or hanging WSL must not freeze the app.

use super::{hidden, run_output_timeout, run_timeout};
use chrono::FixedOffset;
use std::collections::BTreeMap;
use std::time::Duration;

/// (role, systemd unit) of the issue agent.
pub const UNITS: &[(&str, &str)] = &[("coder", "aia-coder"), ("qa", "aia-qa"), ("pr-feedback", "aia-prfeedback")];

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);

pub fn unit_of(role: &str) -> Option<&'static str> {
    UNITS.iter().find(|(r, _)| *r == role).map(|(_, u)| *u)
}

fn wsl(args: &[&str]) -> std::process::Command {
    let mut cmd = hidden("wsl.exe");
    cmd.arg("-e").args(args);
    cmd
}

/// `systemctl --user is-active` for every unit: role → active/inactive/failed/…
/// (is-active exits non-zero when any unit is down; its stdout is still the answer.)
pub fn unit_states() -> Result<BTreeMap<String, String>, String> {
    let mut args = vec!["systemctl", "--user", "is-active"];
    args.extend(UNITS.iter().map(|(_, u)| *u));
    let out = run_output_timeout(wsl(&args), PROBE_TIMEOUT)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.lines().count() != UNITS.len() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() { "WSL antwortet nicht wie erwartet".into() } else { stderr });
    }
    Ok(parse_states(&stdout))
}

pub fn parse_states(out: &str) -> BTreeMap<String, String> {
    UNITS.iter().zip(out.lines()).map(|((role, _), state)| (role.to_string(), state.trim().to_string())).collect()
}

/// start / stop / restart one unit.
pub fn unit_action(verb: &str, unit: &str) -> Result<(), String> {
    run_timeout(wsl(&["systemctl", "--user", verb, unit]), ACTION_TIMEOUT).map(|_| ())
}

/// Claude CLI version the daemons use (their PATH puts ~/.npm-global/bin first) and the
/// WSL time-zone offset (agent.log timestamps are WSL local time, which may differ).
pub fn probe() -> Result<(String, Option<FixedOffset>), String> {
    let script = "date +%z; (~/.npm-global/bin/claude --version || claude --version) 2>/dev/null | head -1";
    let out = run_timeout(wsl(&["bash", "-lc", script]), PROBE_TIMEOUT)?;
    parse_probe(&out)
}

pub fn parse_probe(out: &str) -> Result<(String, Option<FixedOffset>), String> {
    let mut lines = out.lines();
    let offset = lines.next().and_then(parse_offset);
    let version = lines.next().and_then(|l| l.split_whitespace().next()).unwrap_or("").to_string();
    if version.is_empty() { Err("claude nicht gefunden".into()) } else { Ok((version, offset)) }
}

/// "+0200" / "-0530" → offset.
fn parse_offset(s: &str) -> Option<FixedOffset> {
    let s = s.trim();
    let sign = match s.get(..1)? {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let h: i32 = s.get(1..3)?.parse().ok()?;
    let m: i32 = s.get(3..5)?.parse().ok()?;
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
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

    #[test]
    fn probe_reads_offset_and_version() {
        let (v, off) = parse_probe("+0000\n2.1.295 (Claude Code)\n").unwrap();
        assert_eq!(v, "2.1.295");
        assert_eq!(off, FixedOffset::east_opt(0));
        assert_eq!(parse_probe("-0530\n1.0.0\n").unwrap().1, FixedOffset::east_opt(-(5 * 3600 + 30 * 60)));
        assert!(parse_probe("+0200\n").is_err());
    }
}
