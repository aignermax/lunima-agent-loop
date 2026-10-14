//! Starting and stopping the team agent daemon (`lunima-team-agent run`).
//!
//! Windows: scheduled task `LunimaTeamAgent` — at logon of this user, interactive (so the
//! user's Claude and gh logins work), console hidden via `conhost --headless`, restart on
//! failure, no time limit. Linux: systemd user unit `lunima-team-agent.service` (written to
//! `~/.config/systemd/user` if the package didn't ship one). Without autostart the daemon is
//! simply spawned in the background.

use super::{hidden, run};
use std::path::{Path, PathBuf};

pub const TASK_NAME: &str = "LunimaTeamAgent";
pub const UNIT: &str = "lunima-team-agent";
pub const EXE: &str = if cfg!(windows) { "lunima-team-agent.exe" } else { "lunima-team-agent" };

/// A non-default data folder (`lunima-po --home`): the agent must use the same one.
static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_home(dir: PathBuf) {
    let _ = HOME.set(dir);
}

/// `run` plus `--home "<dir>"` when the Control Center runs on a non-default folder.
fn run_args() -> String {
    match HOME.get() {
        Some(h) => format!("run --home \"{}\"", h.display()),
        None => "run".into(),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServiceInfo {
    /// starts with the user's session (task exists / unit enabled)
    pub autostart: bool,
    /// task state (Ready/Running/Disabled) or unit state (active/inactive/failed)
    pub state: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verb {
    Start,
    Stop,
    Restart,
}

/// The daemon next to this app (installed side by side), else on PATH.
pub fn find_exe() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(EXE)));
    beside.filter(|p| p.is_file()).or_else(|| team_core::proc::which("lunima-team-agent"))
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// PowerShell that (re)registers the at-logon task for the current user.
pub fn register_script(exe: &Path) -> String {
    let args = format!("--headless \"{}\" {}", exe.display(), run_args());
    format!(
        "$u = \"$env:USERDOMAIN\\$env:USERNAME\"\n\
         $a = New-ScheduledTaskAction -Execute 'conhost.exe' -Argument {args} -WorkingDirectory {dir}\n\
         $t = New-ScheduledTaskTrigger -AtLogOn -User $u\n\
         $s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\n\
         $p = New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Limited\n\
         Register-ScheduledTask -TaskName '{TASK_NAME}' -Action $a -Trigger $t -Settings $s -Principal $p -Description 'Lunima Team Agent (Coder, QA, PR-Feedback, Product Owner)' -Force | Out-Null",
        args = ps_quote(&args),
        dir = ps_quote(&exe.parent().map(|d| d.display().to_string()).unwrap_or_default()),
    )
}

/// The systemd user unit for `exe`.
pub fn unit_file(exe: &Path) -> String {
    format!(
        "[Unit]\nDescription=Lunima Team Agent (Coder, QA, PR-Feedback, Product Owner)\nAfter=network-online.target\n\n[Service]\nEnvironment=PATH=%h/.local/bin:%h/.npm-global/bin:%h/bin:/usr/local/bin:/usr/bin:/bin\nExecStart=\"{}\" {}\nRestart=on-failure\nRestartSec=60\n\n[Install]\nWantedBy=default.target\n",
        exe.display(),
        run_args()
    )
}

pub fn systemctl_args(verb: &'static str) -> [&'static str; 3] {
    ["--user", verb, UNIT]
}

const QUERY: &str = "$t = Get-ScheduledTask -TaskName 'LunimaTeamAgent' -ErrorAction SilentlyContinue\nif ($t) { [string]$t.State } else { 'missing' }";

pub fn query() -> Result<ServiceInfo, String> {
    if cfg!(windows) {
        let state = super::powershell(QUERY)?.trim().to_string();
        return Ok(ServiceInfo { autostart: state != "missing" && state != "Disabled", state });
    }
    let systemctl = |verb: &'static str| {
        let mut c = hidden("systemctl");
        c.args(systemctl_args(verb));
        run(c).unwrap_or_else(|e| e).trim().to_string()
    };
    Ok(ServiceInfo { autostart: systemctl("is-enabled") == "enabled", state: systemctl("is-active") })
}

/// Autostart on/off. On: registers (Windows) / enables and starts (Linux).
pub fn set_autostart(on: bool, exe: &Path) -> Result<(), String> {
    if cfg!(windows) {
        let script = if on { register_script(exe) } else { format!("Unregister-ScheduledTask -TaskName '{TASK_NAME}' -Confirm:$false -ErrorAction SilentlyContinue") };
        return super::powershell(&script).map(|_| ());
    }
    if on {
        ensure_unit(exe)?;
    }
    let mut c = hidden("systemctl");
    c.args(["--user", if on { "enable" } else { "disable" }, "--now", UNIT]);
    run(c).map(|_| ())
}

/// Writes the user unit if neither the package nor the user has one.
fn ensure_unit(exe: &Path) -> Result<(), String> {
    let shipped = ["/usr/lib/systemd/user", "/lib/systemd/user"].iter().any(|d| Path::new(d).join(format!("{UNIT}.service")).is_file());
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("HOME fehlt")?;
    let own = home.join(".config").join("systemd").join("user").join(format!("{UNIT}.service"));
    if shipped || own.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(own.parent().expect("has parent")).map_err(|e| e.to_string())?;
    std::fs::write(&own, unit_file(exe)).map_err(|e| e.to_string())?;
    let mut c = hidden("systemctl");
    c.args(["--user", "daemon-reload"]);
    run(c).map(|_| ())
}

/// Start / stop / restart, through the task or unit when autostart is set up.
pub fn control(verb: Verb, exe: &Path, info: Option<&ServiceInfo>, state: &Path) -> Result<(), String> {
    let managed = info.is_some_and(|i| i.autostart);
    if matches!(verb, Verb::Stop | Verb::Restart) {
        stop(managed, state)?;
    }
    if verb == Verb::Stop {
        return Ok(());
    }
    if managed && cfg!(windows) {
        return super::powershell(&format!("Start-ScheduledTask -TaskName '{TASK_NAME}'")).map(|_| ());
    }
    if managed {
        let mut c = hidden("systemctl");
        c.args(systemctl_args("start"));
        return run(c).map(|_| ());
    }
    spawn(exe)
}

fn stop(managed: bool, state: &Path) -> Result<(), String> {
    if managed && !cfg!(windows) {
        let mut c = hidden("systemctl");
        c.args(systemctl_args("stop"));
        return run(c).map(|_| ());
    }
    if managed {
        let _ = super::powershell(&format!("Stop-ScheduledTask -TaskName '{TASK_NAME}' -ErrorAction SilentlyContinue"));
    }
    // the task's conhost may leave the daemon behind; a spawned daemon has no task at all
    if let Some(pid) = crate::core::agent_files::agent_pid(state) {
        super::process::terminate(pid);
    }
    if cfg!(windows) {
        let mut c = hidden("taskkill");
        c.args(["/IM", EXE, "/T", "/F"]);
        let _ = run(c); // "no such process" is fine
    }
    Ok(())
}

/// Background daemon without console window (survives this app).
fn spawn(exe: &Path) -> Result<(), String> {
    let mut cmd = hidden(exe);
    cmd.arg("run");
    if let Some(h) = HOME.get() {
        cmd.arg("--home").arg(h);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("{}: {e}", exe.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_runs_the_daemon_hidden_at_logon_and_restarts() {
        let s = register_script(Path::new(r"C:\Program Files\Lunima Team\lunima-team-agent.exe"));
        assert!(s.contains("-Execute 'conhost.exe' -Argument '--headless \"C:\\Program Files\\Lunima Team\\lunima-team-agent.exe\" run'"));
        assert!(s.contains("-AtLogOn") && s.contains("-LogonType Interactive"));
        assert!(s.contains("-RestartCount 3") && s.contains("[TimeSpan]::Zero"));
        assert!(s.contains("-TaskName 'LunimaTeamAgent'"));
        assert!(register_script(Path::new("C:/it's/x.exe")).contains("it''s"), "quotes are escaped");
    }

    #[test]
    fn unit_runs_the_daemon_and_restarts_on_failure() {
        let u = unit_file(Path::new("/usr/bin/lunima-team-agent"));
        assert!(u.contains("ExecStart=\"/usr/bin/lunima-team-agent\" run"), "{u}");
        assert!(u.contains("Restart=on-failure") && u.contains("RestartSec=60") && u.contains("WantedBy=default.target"));
        assert_eq!(systemctl_args("is-active"), ["--user", "is-active", "lunima-team-agent"]);
    }
}
