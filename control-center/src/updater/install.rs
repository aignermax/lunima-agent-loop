//! Detached package installation after the caller has safely drained the daemon.

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub fn supported() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad nicht lesbar: {e}"))?;
    let exe = text_path(&exe)?;
    #[cfg(windows)]
    {
        let pf = std::env::var("ProgramW6432")
            .or_else(|_| std::env::var("ProgramFiles"))
            .map_err(|_| "Programme-Ordner nicht gefunden")?;
        if !windows_eligible(exe, &pf, true) {
            return Err("Updates nur für die MSI-Installation unter Programme/Lunima Team; nicht für Cargo-Builds".into());
        }
        let out = command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", MSI_REGISTERED])
            .output()
            .map_err(|e| format!("MSI-Registrierung nicht prüfbar: {e}"))?;
        if !out.status.success()
            || !windows_eligible(
                exe,
                &pf,
                String::from_utf8_lossy(&out.stdout).trim() == "registered",
            )
        {
            return Err("Lunima Team ist nicht als MSI installiert".into());
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        if !linux_eligible(exe, true) {
            return Err("Updates nur für die .deb-Installation unter /usr/bin/lunima-po; nicht für Cargo-Builds".into());
        }
        if !Path::new("/usr/bin/pkexec").is_file() {
            return Err("Für Updates fehlt pkexec. Bitte das Paket pkexec installieren".into());
        }
        if !Path::new("/usr/bin/flock").is_file() {
            return Err("Für Updates fehlt flock (Paket util-linux)".into());
        }
        let status = command("/usr/bin/dpkg-query")
            .args(["--show", "--showformat=${Status}", "lunima-team"])
            .output()
            .map_err(|e| format!("Debian-Paketstatus nicht prüfbar: {e}"))?;
        let owner = command("/usr/bin/dpkg-query")
            .args(["--search", "/usr/bin/lunima-po"])
            .output()
            .map_err(|e| format!("Debian-Paketzuordnung nicht prüfbar: {e}"))?;
        let owned = String::from_utf8_lossy(&owner.stdout).lines().any(|line| {
            line.split_once(": ").is_some_and(|(pkg, file)| {
                pkg.split(':').next() == Some("lunima-team") && file == "/usr/bin/lunima-po"
            })
        });
        if status.status.success()
            && owner.status.success()
            && owned
            && linux_eligible(
                exe,
                String::from_utf8_lossy(&status.stdout).trim() == "install ok installed",
            )
        {
            return Ok(());
        }
        return Err(
            "/usr/bin/lunima-po gehört nicht zum installierten .deb-Paket lunima-team".into(),
        );
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = exe;
        Err("Paket-Updates nur unter Windows und Debian-Linux verfügbar".into())
    }
}

/// The caller must finish draining before calling, then quit the UI on `Ok`.
/// Only the package manager elevates; the detached supervisor retains this user's
/// identity, environment, session bus and credentials for all recovery/relaunches.
pub fn launch(package: &Path, home: &Path, token: &str, restart_agent: bool) -> Result<(), String> {
    let result = prepare_and_spawn(package, home, token, restart_agent);
    if let Err(error) = &result {
        // The UI remains alive when launch fails, so do not spawn a second UI.
        let updates = home.join("updates");
        let _ = fs::create_dir_all(&updates);
        let message = format!("{token}\n{error}\n");
        let _ = fs::write(updates.join("install-failed"), &message);
        if let Ok(mut log) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(updates.join("install.log"))
        {
            let _ = log.write_all(message.as_bytes());
        }
        for file in ["update-drain.json", "update-drained.json"] {
            let _ = fs::remove_file(home.join("state").join(file));
        }
        if restart_agent {
            let _ = recover_daemon(home);
        }
    }
    result
}

fn prepare_and_spawn(
    package: &Path,
    home: &Path,
    token: &str,
    restart_agent: bool,
) -> Result<(), String> {
    supported()?;
    if token.is_empty() || token.contains('\0') {
        return Err("Invalid update token".into());
    }
    let package = package
        .canonicalize()
        .map_err(|e| format!("Update package: {e}"))?;
    let ext = if cfg!(windows) { "msi" } else { "deb" };
    if !package.is_file() || package.extension().and_then(|e| e.to_str()) != Some(ext) {
        return Err(format!("Expected an existing .{ext} update package"));
    }
    let home = home
        .canonicalize()
        .map_err(|e| format!("Update home: {e}"))?;
    let ui = std::env::current_exe().map_err(|e| e.to_string())?;
    let daemon = ui.with_file_name(if cfg!(windows) {
        "lunima-team-agent.exe"
    } else {
        "lunima-team-agent"
    });
    let updates = home.join("updates");
    fs::create_dir_all(&updates).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let handoff = format!("install-helper-{}-{nonce}", std::process::id());
    let install = Install {
        package: text_path(&package)?,
        home: text_path(&home)?,
        token,
        ui: text_path(&ui)?,
        daemon: text_path(&daemon)?,
        ui_pid: std::process::id(),
        restart_agent,
        handoff: &handoff,
    };
    let script = updates.join(format!(
        "{handoff}.{}",
        if cfg!(windows) { "ps1" } else { "sh" }
    ));
    let ready = updates.join(format!("{handoff}.ready"));
    let abort = updates.join(format!("{handoff}.abort"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&script).map_err(|e| e.to_string())?;
    let contents = if cfg!(windows) {
        format!("\u{feff}{}", windows_script(&install))
    } else {
        linux_script(&install)
    };
    file.write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    let mut helper = if cfg!(windows) {
        let mut cmd = command("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script);
        cmd
    } else {
        let mut cmd = command("/bin/sh");
        cmd.arg(&script);
        cmd
    };
    detach(&mut helper);
    helper
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = helper
        .spawn()
        .map_err(|e| format!("Installer-Helfer konnte nicht gestartet werden: {e}"))?;
    wait_ready(
        &mut child,
        &ready,
        &abort,
        token,
        std::time::Duration::from_secs(15),
    )
}

fn wait_ready(
    child: &mut std::process::Child,
    ready: &Path,
    abort: &Path,
    token: &str,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let failure = match child.try_wait() {
            Ok(Some(status)) => Some(format!("Installer-Helfer vor Übergabe beendet: {status}")),
            Err(e) => Some(format!("Installer-Helfer nicht erreichbar: {e}")),
            Ok(None) if std::time::Instant::now() >= deadline => {
                Some("Installer-Helfer nicht bereit; Update bleibt vorbereitet".into())
            }
            Ok(None) => None,
        };
        if let Some(error) = failure {
            // Cooperative cancellation: a late helper must never install after
            // the UI has decided to stay open. Never terminate user processes.
            let _ = fs::write(abort, token);
            return Err(error);
        }
        if fs::read_to_string(ready).is_ok_and(|value| value == token) {
            let _ = fs::remove_file(ready);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn text_path(path: &Path) -> Result<&str, String> {
    let path = path
        .to_str()
        .ok_or("Update-Pfade müssen gültiges Unicode enthalten")?;
    if path.contains('\0') {
        return Err("Update-Pfade dürfen kein NUL enthalten".into());
    }
    #[cfg(windows)]
    let path = path
        .strip_prefix(r"\\?\")
        .filter(|p| p.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(path);
    Ok(path)
}

fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(0x0800_0000);
        command
    }
    #[cfg(not(windows))]
    command
}

fn detach(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Break away from a UI job object as well as its console. Failure to
        // break away is surfaced to the caller, never silently attached to it.
        command.creation_flags(0x0800_0000 | 0x0000_0200 | 0x0100_0000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        unsafe extern "C" {
            fn setsid() -> i32;
        }
        // setsid is async-signal-safe; no allocation or locks after fork.
        unsafe {
            command.pre_exec(|| {
                if setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
}

fn recover_daemon(home: &Path) -> Result<(), String> {
    let ui = std::env::current_exe().map_err(|e| e.to_string())?;
    // On a failed handoff the UI still owns recovery. Start directly so a
    // task/unit belonging to another data folder is never resumed.
    let mut daemon = command(ui.with_file_name(if cfg!(windows) {
        "lunima-team-agent.exe"
    } else {
        "lunima-team-agent"
    }));
    daemon
        .args(["run", "--home"])
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut daemon);
    daemon.spawn().map(|_| ()).map_err(|e| e.to_string())
}

// Product registration is tied to the installer's stable UpgradeCode, rather
// than an unrelated/untrusted ARP display name. ProductState 5 = installed.
#[cfg(windows)]
const MSI_REGISTERED: &str = r#"$ErrorActionPreference = 'Stop'
$i = New-Object -ComObject WindowsInstaller.Installer
$get = [System.Reflection.BindingFlags]::GetProperty
$products = $i.GetType().InvokeMember('RelatedProducts', $get, $null, $i, @('{5B151487-3B93-4F4E-A46C-3C0B2D0549AE}'))
foreach ($p in $products) {
    $state = $i.GetType().InvokeMember('ProductState', $get, $null, $i, @($p))
    if ($state -eq 5) { 'registered'; exit 0 }
}
exit 1
"#;

fn windows_eligible(exe: &str, program_files: &str, registered: bool) -> bool {
    let normalize = |s: &str| {
        s.strip_prefix(r"\\?\")
            .unwrap_or(s)
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    registered
        && normalize(exe)
            == format!(
                "{}\\lunima team\\lunima-po.exe",
                normalize(program_files).trim_end_matches('\\')
            )
}
fn linux_eligible(exe: &str, registered: bool) -> bool {
    registered && exe == "/usr/bin/lunima-po"
}
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Start-Process joins ArgumentList without escaping: provide a complete native
/// command line, with each argument encoded using Windows CRT quoting rules.
fn windows_arg(s: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in s.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        out.extend(std::iter::repeat_n(
            '\\',
            if ch == '"' { slashes * 2 + 1 } else { slashes },
        ));
        slashes = 0;
        out.push(ch);
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

struct Install<'a> {
    package: &'a str,
    home: &'a str,
    token: &'a str,
    ui: &'a str,
    daemon: &'a str,
    ui_pid: u32,
    restart_agent: bool,
    handoff: &'a str,
}

fn windows_script(i: &Install<'_>) -> String {
    let log = Path::new(i.home).join("updates").join("install.log");
    let log = log.to_string_lossy();
    let msi_args = format!(
        "/i {} /passive /norestart /L*+v {}",
        windows_arg(i.package),
        windows_arg(&log)
    );
    let ui_args = format!("--home {} --updated-relaunch", windows_arg(i.home));
    let agent_args = format!("run --home {}", windows_arg(i.home));
    format!(
        r#"$ErrorActionPreference = 'Stop'
$homeDir = {home}
$token = {token}
$log = Join-Path $homeDir 'updates/install.log'
$failed = Join-Path $homeDir 'updates/install-failed'
$pending = Join-Path $homeDir 'updates/pending.json'
$ready = Join-Path $homeDir {ready}
$abort = Join-Path $homeDir {abort}
$guard = $null
$uiExited = $false
$ok = $false
function Log([string]$message) {{
    try {{ Add-Content -LiteralPath $log -Value ((Get-Date -Format o) + ' ' + $message) -Encoding UTF8 }} catch {{}}
}}
function Clear-Drain {{
    foreach ($name in @('update-drain.json', 'update-drained.json')) {{
        $path = Join-Path $homeDir ('state/' + $name)
        if (Test-Path -LiteralPath $path) {{ Remove-Item -LiteralPath $path -ErrorAction Stop }}
    }}
}}
try {{
    $guard = [IO.File]::Open((Join-Path $homeDir 'state/agent.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
    $lockDeadline = (Get-Date).AddSeconds(10)
    while ($true) {{
        if (Test-Path -LiteralPath $abort) {{ throw 'Installer handoff cancelled' }}
        try {{ $guard.Lock(0, 1); break }} catch [IO.IOException] {{
            if ((Get-Date) -ge $lockDeadline) {{ throw 'Agent lock is still held; installation was not started' }}
            Start-Sleep -Milliseconds 50
        }}
    }}
    # This byte-range overlaps Rust File::try_lock's whole-file LockFileEx.
    # Keep the handle alive through MSI and all drain/pending cleanup.
    [IO.File]::WriteAllText($ready, $token, (New-Object Text.UTF8Encoding($false)))
    Log ('Waiting for Control Center; update ' + $token)
    while ($true) {{
        if (Test-Path -LiteralPath $abort) {{ throw 'Installer handoff cancelled' }}
        try {{ Wait-Process -Id {pid} -Timeout 1 -ErrorAction Stop; break }} catch {{
            if ($_.FullyQualifiedErrorId -like 'NoProcessFoundForGivenId,*') {{ break }}
            if ($_.FullyQualifiedErrorId -like 'ProcessNotTerminated,*') {{ continue }}
            throw
        }}
    }}
    $uiExited = $true
    if (Test-Path -LiteralPath $abort) {{ throw 'Installer handoff cancelled' }}
    Log 'Starting elevated MSI installer'
    $installer = Start-Process -FilePath 'msiexec.exe' -Verb RunAs -ArgumentList {msi_args} -Wait -PassThru -ErrorAction Stop
    if ($installer.ExitCode -notin @(0, 1641, 3010)) {{ throw ('MSI failed or cancelled: exit ' + $installer.ExitCode) }}
    Clear-Drain
    if (Test-Path -LiteralPath $failed) {{ Remove-Item -LiteralPath $failed -ErrorAction Stop }}
    # Commit success before any relaunch, so startup cannot retry this package.
    if (Test-Path -LiteralPath $pending) {{ Remove-Item -LiteralPath $pending -ErrorAction Stop }}
    $ok = $true
    Log 'Installation succeeded'
}} catch {{
    $failure = 'Installation failed/cancelled: ' + $_.Exception.Message
    Log $failure
    try {{ Set-Content -LiteralPath $failed -Value ($token + [Environment]::NewLine + $failure) -Encoding UTF8 }} catch {{ Log ('Could not write failure marker: ' + $_) }}
    try {{ Clear-Drain }} catch {{ Log ('Could not clear drain state: ' + $_) }}
}} finally {{
    if ($guard) {{ $guard.Dispose() }}
}}
if ({restart}) {{
    $started = $false
    try {{
        $task = Get-ScheduledTask -TaskName 'LunimaTeamAgent' -ErrorAction SilentlyContinue
        if ($task -and $task.State -ne 'Disabled' -and @($task.Actions).Count -eq 1) {{
            $user = [Security.Principal.WindowsIdentity]::GetCurrent()
            if ($task.Principal.UserId -notin @($user.Name, $user.User.Value, $env:USERNAME)) {{ throw 'Task belongs to a different user' }}
            $action = $task.Actions[0]
            $same = [string]::Equals($action.Execute, {daemon}, [StringComparison]::OrdinalIgnoreCase) -and $action.Arguments -ceq {agent_args}
            $conhost = $action.Execute -in @('conhost.exe', (Join-Path $env:SystemRoot 'System32/conhost.exe'))
            $same = $same -or ($conhost -and $action.Arguments -ceq {conhost_args})
            if (-not $same) {{ throw 'Task executable/home differs; using direct restart' }}
            Start-ScheduledTask -TaskName 'LunimaTeamAgent' -ErrorAction Stop
            $started = $true
        }}
    }} catch {{ Log ('Task restart unavailable: ' + $_) }}
    if (-not $started) {{
        try {{ Start-Process -FilePath {daemon} -ArgumentList {agent_args} -WindowStyle Hidden -ErrorAction Stop }} catch {{ Log ('Daemon restart failed: ' + $_) }}
    }}
}}
if ($uiExited) {{
    try {{ Start-Process -FilePath {ui} -ArgumentList {ui_args} -WindowStyle Hidden -ErrorAction Stop }} catch {{ Log ('Control Center relaunch failed: ' + $_) }}
}}
if (-not $ok) {{ exit 1 }}
"#,
        home = ps_quote(i.home),
        token = ps_quote(i.token),
        pid = i.ui_pid,
        ready = ps_quote(&format!("updates/{}.ready", i.handoff)),
        abort = ps_quote(&format!("updates/{}.abort", i.handoff)),
        msi_args = ps_quote(&msi_args),
        restart = if i.restart_agent { "$true" } else { "$false" },
        daemon = ps_quote(i.daemon),
        agent_args = ps_quote(&agent_args),
        conhost_args = ps_quote(&format!(
            "--headless {} {agent_args}",
            windows_arg(i.daemon)
        )),
        ui = ps_quote(i.ui),
        ui_args = ps_quote(&ui_args)
    )
}

fn linux_script(i: &Install<'_>) -> String {
    format!(
        r#"#!/bin/sh
# This supervisor stays in the original user's session. Only dpkg elevates.
home={home}
package={package}
token={token}
ui={ui}
daemon={daemon}
ui_pid={pid}
restart={restart}
ready="$home/updates/"{ready}
abort="$home/updates/"{abort}
ui_exited=0
ok=0
exec >> "$home/updates/install.log" 2>&1
log() {{ printf '%s %s\n' "$(date -Iseconds)" "$*"; }}
clear_drain() {{ rm -f -- "$home/state/update-drain.json" "$home/state/update-drained.json"; }}
failed() {{
    log "Installation failed/cancelled: $1"
    printf '%s\n%s\n' "$token" "$1" > "$home/updates/install-failed"
    clear_drain
}}
perform_install() {{
    exec 9>> "$home/state/agent.lock" || return 1
    # Rust File::try_lock uses flock on Linux. Hold the same inode/lock,
    # even if the drain request's freshness window expires during dpkg.
    flock -x -w 10 9 || return 1
    [ ! -e "$abort" ] || return 1
    printf '%s' "$token" > "$ready" || return 1
    log "Waiting for Control Center; update $token"
    while kill -0 "$ui_pid" 2>/dev/null; do
        [ ! -e "$abort" ] || return 1
        sleep 0.2
    done
    ui_exited=1
    [ ! -e "$abort" ] || return 1
    log 'Starting elevated Debian installer'
    pkexec /usr/bin/dpkg --install "$package" || return $?
    clear_drain && rm -f -- "$home/updates/install-failed" && rm -f -- "$home/updates/pending.json"
}}
if perform_install; then
    ok=1
    log 'Installation succeeded'
else
    status=$?
    failed "dpkg/pkexec exit $status"
fi
# Release before either the old or the new daemon can acquire agent.lock.
flock -u 9 2>/dev/null || :
exec 9>&-
unit_matches() {{
    # Accept the shipped default-home action only when the user manager's
    # environment agrees. Unusual/custom units fall back to explicit argv.
    action=$(systemctl --user show -p ExecStart --value lunima-team-agent.service 2>/dev/null) || return 1
    case "$action" in
        "{{ path=$daemon ; argv[]=$daemon run ; ignore_errors=no ; "*) ;;
        *) return 1;;
    esac
    environment=$(systemctl --user show -p Environment --value lunima-team-agent.service 2>/dev/null) || return 1
    case "$environment" in *LUNIMA_TEAM_HOME=*|*XDG_DATA_HOME=*|*HOME=*) return 1;; esac
    files=$(systemctl --user show -p EnvironmentFiles --value lunima-team-agent.service 2>/dev/null) || return 1
    [ -z "$files" ] || return 1
    manager=$(systemctl --user show-environment 2>/dev/null) || return 1
    manager_home= manager_xdg= manager_lunima=
    # Do not eval systemd's quoted environment output. A quoted/special path
    # cannot match and therefore safely takes the direct-start route.
    while IFS= read -r line; do
        case "$line" in
            HOME=*) manager_home=${{line#HOME=}};;
            XDG_DATA_HOME=*) manager_xdg=${{line#XDG_DATA_HOME=}};;
            LUNIMA_TEAM_HOME=*) manager_lunima=${{line#LUNIMA_TEAM_HOME=}};;
        esac
    done <<EOF
$manager
EOF
    expected=${{manager_lunima:-${{manager_xdg:-$manager_home/.local/share}}/lunima-team}}
    [ -n "$manager_home" ] && [ "$expected" = "$home" ]
}}
if [ "$restart" = 1 ]; then
    started=0
    if unit_matches; then
        if systemctl --user start lunima-team-agent.service; then started=1; fi
    fi
    if [ "$started" = 0 ]; then
        "$daemon" run --home "$home" </dev/null >> "$home/updates/install.log" 2>&1 &
    fi
fi
if [ "$ui_exited" = 1 ]; then
    "$ui" --home "$home" --updated-relaunch </dev/null >> "$home/updates/install.log" 2>&1 &
fi
[ "$ok" = 1 ]
"#,
        home = sh_quote(i.home),
        package = sh_quote(i.package),
        token = sh_quote(i.token),
        ui = sh_quote(i.ui),
        daemon = sh_quote(i.daemon),
        pid = i.ui_pid,
        restart = u8::from(i.restart_agent),
        ready = sh_quote(&format!("{}.ready", i.handoff)),
        abort = sh_quote(&format!("{}.abort", i.handoff))
    )
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
