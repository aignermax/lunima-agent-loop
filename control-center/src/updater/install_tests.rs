use super::*;
use std::{fs, path::PathBuf, process::Command};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("lunima-install-test-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn eligibility_requires_registered_package_at_exact_installed_location() {
    let pf = r"C:\Program Files";
    assert!(windows_eligible(
        r"C:\Program Files\Lunima Team\lunima-po.exe",
        pf,
        true
    ));
    assert!(windows_eligible(
        "c:/program files/LUNIMA TEAM/lunima-po.exe",
        pf,
        true
    ));
    for path in [
        r"C:\dev\target\release\lunima-po.exe",
        r"C:\Program Files\Lunima Team\target\release\lunima-po.exe",
        r"C:\Program Files\Lunima Team Extra\lunima-po.exe",
        r"C:\Program Files\Lunima Team\..\lunima-po.exe",
    ] {
        assert!(!windows_eligible(path, pf, true), "{path}");
    }
    assert!(!windows_eligible(
        r"C:\Program Files\Lunima Team\lunima-po.exe",
        pf,
        false
    ));
    assert!(linux_eligible("/usr/bin/lunima-po", true));
    for path in [
        "/usr/bin/lunima-po",
        "/usr/local/bin/lunima-po",
        "/work/target/release/lunima-po",
        "/usr/bin/../bin/lunima-po",
    ] {
        assert!(
            !linux_eligible(path, path != "/usr/bin/lunima-po"),
            "{path}"
        );
    }
}

#[test]
fn native_windows_arguments_escape_embedded_quotes_and_trailing_backslashes() {
    assert_eq!(windows_arg("a b"), "\"a b\"");
    assert_eq!(windows_arg("a\"b"), "\"a\\\"b\"");
    assert_eq!(windows_arg("C:\\a b\\"), "\"C:\\a b\\\\\"");
    assert_eq!(windows_arg("line\nbreak"), "\"line\nbreak\"");
}

#[cfg(windows)]
#[test]
fn canonical_drive_paths_use_native_spelling_for_msi_and_task_matching() {
    assert_eq!(
        text_path(Path::new(r"\\?\C:\Program Files\Lunima Team\lunima-po.exe")).unwrap(),
        r"C:\Program Files\Lunima Team\lunima-po.exe"
    );
}

#[test]
fn shell_literals_do_not_interpolate_quotes_newlines_or_commands() {
    assert_eq!(ps_quote("a'b\n$()\""), "'a''b\n$()\"'");
    assert_eq!(sh_quote("a'b\n$()\""), "'a'\"'\"'b\n$()\"'");
}

fn idle_child() -> std::process::Child {
    if cfg!(windows) {
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Milliseconds 500",
            ])
            .spawn()
            .unwrap()
    } else {
        Command::new("/bin/sh")
            .args(["-c", "sleep 0.5"])
            .spawn()
            .unwrap()
    }
}

#[test]
fn handoff_requires_matching_ready_token_and_consumes_it() {
    let dir = Temp::new();
    let ready = dir.0.join("ready");
    let abort = dir.0.join("abort");
    let mut child = idle_child();
    fs::write(&ready, "unique-'\ntoken").unwrap();
    assert!(wait_ready(
        &mut child,
        &ready,
        &abort,
        "unique-'\ntoken",
        std::time::Duration::from_secs(1)
    )
    .is_ok());
    assert!(!ready.exists() && !abort.exists());
    child.wait().unwrap();
}

#[test]
fn handoff_timeout_aborts_late_helper_instead_of_quitting_ui() {
    let dir = Temp::new();
    let ready = dir.0.join("ready");
    let abort = dir.0.join("abort");
    let mut child = idle_child();
    fs::write(&ready, "wrong-token").unwrap();
    assert!(wait_ready(
        &mut child,
        &ready,
        &abort,
        "current-token",
        std::time::Duration::from_millis(75)
    )
    .is_err());
    assert_eq!(fs::read_to_string(abort).unwrap(), "current-token");
    child.wait().unwrap();
}

#[test]
fn handoff_rejects_helper_that_exits_before_readiness() {
    let dir = Temp::new();
    let mut child = idle_child();
    child.wait().unwrap();
    assert!(wait_ready(
        &mut child,
        &dir.0.join("ready"),
        &dir.0.join("abort"),
        "token",
        std::time::Duration::from_secs(1)
    )
    .is_err());
    assert!(dir.0.join("abort").exists());
}

fn config<'a>(home: &'a str, restart_agent: bool) -> Install<'a> {
    Install {
        package: "C:\\download dir\\new 'package.msi",
        home,
        token: "token-'\n$(throw 'injected')",
        ui: "C:\\Program Files\\Lunima Team\\lunima-po.exe",
        daemon: "C:\\Program Files\\Lunima Team\\lunima-team-agent.exe",
        ui_pid: 424242,
        restart_agent,
        handoff: "helper",
    }
}

// Only the OS boundaries are replaced: no installer, daemon, UI, or task runs.
#[cfg(windows)]
fn run_windows(code: i32, managed: bool, task_fails: bool, restart: bool) -> (Temp, String) {
    run_windows_case(code, managed, task_fails, restart, "none")
}

#[cfg(windows)]
fn run_windows_case(
    code: i32,
    managed: bool,
    task_fails: bool,
    restart: bool,
    fault: &str,
) -> (Temp, String) {
    let dir = Temp::new();
    fs::create_dir_all(dir.0.join("updates")).unwrap();
    fs::create_dir_all(dir.0.join("state")).unwrap();
    for path in [
        "updates/pending.json",
        "state/update-drain.json",
        "state/update-drained.json",
    ] {
        fs::write(dir.0.join(path), "pending").unwrap();
    }
    let home = dir.0.to_str().unwrap();
    let agent = ps_quote(config(home, restart).daemon);
    let action_args = ps_quote(&format!("run --home {}", windows_arg(home)));
    let prelude = format!(
        r#"
$ErrorActionPreference = 'Stop'
function Wait-Process {{ param($Id, $Timeout, $ErrorAction) if ({fault} -eq 'wait') {{ throw 'wait denied' }}; 'wait' | Add-Content -LiteralPath {events} }}
function Get-ScheduledTask {{ param($TaskName, $ErrorAction) if ({managed}) {{
    $args = {action_args}; if ({fault} -eq 'wrong-home') {{ $args = 'run --home "C:\other"' }}
    [pscustomobject]@{{ State = 'Ready'; Principal = [pscustomobject]@{{ UserId = [Security.Principal.WindowsIdentity]::GetCurrent().Name }}; Actions = @([pscustomobject]@{{ Execute = {agent}; Arguments = $args }}) }}
}} }}
function Start-ScheduledTask {{ param($TaskName, $ErrorAction) if ({task_fails}) {{ throw 'task unavailable' }}; 'task' | Add-Content -LiteralPath {events} }}
function Start-Process {{
    param($FilePath, $ArgumentList, $Verb, [switch]$Wait, [switch]$PassThru, $WindowStyle, $WorkingDirectory, $ErrorAction)
    if ($FilePath -like '*msiexec*') {{
        if ({fault} -eq 'uac') {{ throw 'User cancelled elevation' }}
        if ($Verb -ne 'RunAs' -or -not $Wait -or -not $PassThru) {{ throw 'installer must elevate and wait' }}
        if ($ArgumentList -notlike '*/i * /passive /norestart*') {{ throw 'incorrect MSI arguments' }}
        'install' | Add-Content -LiteralPath {events}
        if ({fault} -eq 'lock') {{
            [IO.File]::WriteAllText({entered}, 'entered')
            while (-not (Test-Path -LiteralPath {release})) {{ Start-Sleep -Milliseconds 25 }}
        }}
        [pscustomobject]@{{ ExitCode = {code} }}
    }} else {{
        if ($Verb) {{ throw 'restart elevated' }}
        if ($ArgumentList -notlike '*--home*') {{ throw 'restart lost home' }}
        if ($FilePath -like '*lunima-team-agent*' -or $FilePath -like '*conhost*') {{ 'daemon' | Add-Content -LiteralPath {events} }}
        else {{
            if ($ArgumentList -notlike '*--updated-relaunch*') {{ throw 'missing relaunch flag' }}
            if ({code} -eq 0 -and {fault} -notin @('uac','wait') -and (Test-Path -LiteralPath {pending})) {{ throw 'UI started before pending was cleared' }}
            'ui' | Add-Content -LiteralPath {events}
        }}
    }}
}}
"#,
        events = ps_quote(dir.0.join("events").to_str().unwrap()),
        managed = if managed { "$true" } else { "$false" },
        task_fails = if task_fails { "$true" } else { "$false" },
        fault = ps_quote(fault),
        entered = ps_quote(dir.0.join("entered").to_str().unwrap()),
        release = ps_quote(dir.0.join("release").to_str().unwrap()),
        pending = ps_quote(dir.0.join("updates/pending.json").to_str().unwrap()),
    );
    let script = dir.0.join("helper.ps1");
    // BOM is required for non-ASCII source in Windows PowerShell 5.1.
    fs::write(
        &script,
        format!(
            "\u{feff}{prelude}{}",
            windows_script(&config(home, restart))
        ),
    )
    .unwrap();
    let child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    if fault == "lock" {
        wait_for_file(&dir.0.join("entered"));
        let locked = lock_is_held(&dir.0.join("state/agent.lock"));
        fs::write(dir.0.join("release"), "release").unwrap();
        assert!(locked, "installer did not exclude Rust File::try_lock");
        assert!(
            dir.0.join("updates/helper.ready").is_file(),
            "helper did not acknowledge readiness"
        );
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.success(),
        matches!(code, 0 | 1641 | 3010) && !matches!(fault, "uac" | "wait"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = fs::read_to_string(dir.0.join("events"))
        .unwrap_or_default()
        .replace('\r', "");
    if fault == "lock" {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.0.join("state/agent.lock"))
            .unwrap();
        file.try_lock()
            .expect("helper retained the lock after installation");
    }
    (dir, events)
}

fn wait_for_file(path: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !path.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(path.exists(), "helper never reached {}", path.display());
}

fn lock_is_held(path: &Path) -> bool {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

#[cfg(windows)]
#[test]
fn windows_helper_holds_rust_compatible_lock_through_install_and_releases_it() {
    let (_, events) = run_windows_case(0, false, false, true, "lock");
    assert_eq!(events, "wait\ninstall\ndaemon\nui\n");
}

#[cfg(windows)]
#[test]
fn wait_failure_cannot_start_installer_and_uac_cancellation_is_failure() {
    for fault in ["wait", "uac"] {
        let (dir, events) = run_windows_case(0, false, false, true, fault);
        assert!(!events.contains("install"));
        assert!(dir.0.join("updates/install-failed").is_file());
        assert!(dir.0.join("updates/pending.json").is_file());
    }
}

#[cfg(windows)]
#[test]
fn task_with_different_home_is_never_started() {
    let (_, events) = run_windows_case(0, true, false, true, "wrong-home");
    assert_eq!(events, "wait\ninstall\ndaemon\nui\n");
}

#[cfg(windows)]
#[test]
fn entire_windows_helper_parses_even_with_hostile_literals() {
    let dir = Temp::new();
    let script = dir.0.join("parse.ps1");
    let mut i = config("C:/a 'user/line\nbreak", true);
    i.package = "C:/new \"package\"/'\n$(throw 'injected').msi";
    let helper = windows_script(&i);
    let parse = format!("$tokens=$null; $errors=$null; [void][System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$tokens, [ref]$errors); if ($errors.Count) {{ $errors | Out-String | Write-Error; exit 1 }}", ps_quote(&helper));
    fs::write(&script, format!("\u{feff}{parse}")).unwrap();
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(script)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
fn run_linux(code: i32, hold: bool) -> (Temp, String) {
    let dir = Temp::new();
    fs::create_dir_all(dir.0.join("updates")).unwrap();
    fs::create_dir_all(dir.0.join("state")).unwrap();
    for path in [
        "updates/pending.json",
        "state/update-drain.json",
        "state/update-drained.json",
    ] {
        fs::write(dir.0.join(path), "pending").unwrap();
    }
    use std::os::unix::fs::PermissionsExt;
    for (name, event) in [("ui", "ui"), ("daemon", "daemon")] {
        let path = dir.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n[ \"$1\" = run ] && shift\n[ \"$1\" = --home ] && [ \"$2\" = \"$EXPECTED_HOME\" ] || exit 8\nflock -s -n \"$2/state/agent.lock\" true || exit 11\nif [ {event} = ui ] && [ \"$EXPECT_SUCCESS\" = 1 ] && [ -e \"$2/updates/pending.json\" ]; then exit 12; fi\nprintf '%s\\n' {event} >> \"$EVENTS\"\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let home = dir.0.to_str().unwrap();
    let ui = dir.0.join("ui");
    let daemon = dir.0.join("daemon");
    let mut i = config(home, true);
    i.ui = ui.to_str().unwrap();
    i.daemon = daemon.to_str().unwrap();
    i.ui_pid = 2147483647;
    i.package = "/tmp/package 'with\n$(touch injected).deb";
    let prelude = format!(
        r#"
export EVENTS={events} EXPECTED_HOME={home} EXPECT_SUCCESS={success}
pkexec() {{
    [ "$1" = /usr/bin/dpkg ] && [ "$2" = --install ] || return 9
    printf '%s' "$3" > {package_seen}
    echo install >> "$EVENTS"
    if [ {hold} = 1 ]; then
        touch {entered}
        while [ ! -e {release} ]; do sleep 0.025; done
    fi
    return {code}
}}
systemctl() {{
    case "$*" in
        *LoadState*) echo loaded;;
        *ExecStart*) printf '%s\n' '{{ path=/wrong/daemon ; argv[]=/wrong/daemon run --home /wrong/home ; ignore_errors=no ; }}';;
        *show-environment*) printf '%s\n' 'HOME=/wrong/home';;
        *start*) echo wrong-unit >> "$EVENTS"; return 0;;
    esac
}}
"#,
        events = sh_quote(dir.0.join("events").to_str().unwrap()),
        home = sh_quote(home),
        package_seen = sh_quote(dir.0.join("package-seen").to_str().unwrap()),
        entered = sh_quote(dir.0.join("entered").to_str().unwrap()),
        release = sh_quote(dir.0.join("release").to_str().unwrap()),
        hold = u8::from(hold),
        success = u8::from(code == 0)
    );
    let script = dir.0.join("helper.sh");
    fs::write(&script, format!("{prelude}{}", linux_script(&i))).unwrap();
    let child = Command::new("/bin/sh").arg(&script).spawn().unwrap();
    if hold {
        wait_for_file(&dir.0.join("entered"));
        let locked = lock_is_held(&dir.0.join("state/agent.lock"));
        fs::write(dir.0.join("release"), "release").unwrap();
        assert!(locked, "installer did not exclude Rust File::try_lock");
        assert!(dir.0.join("updates/helper.ready").exists());
    }
    assert_eq!(
        child.wait_with_output().unwrap().status.success(),
        code == 0
    );
    wait_for_file(&dir.0.join("events"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let events = loop {
        let events = fs::read_to_string(dir.0.join("events")).unwrap();
        if (events.contains("ui") && events.contains("daemon"))
            || std::time::Instant::now() > deadline
        {
            break events;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(
        fs::read_to_string(dir.0.join("package-seen")).unwrap(),
        i.package
    );
    if hold {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.0.join("state/agent.lock"))
            .unwrap()
            .try_lock()
            .expect("lock not released");
    }
    (dir, events)
}

#[cfg(target_os = "linux")]
#[test]
fn linux_helper_holds_rust_compatible_flock_and_clears_pending_before_relaunch() {
    let (dir, events) = run_linux(0, true);
    assert!(
        events.contains("daemon") && events.contains("ui"),
        "{events}"
    );
    assert!(!events.contains("wrong-unit"), "{events}");
    assert!(!dir.0.join("updates/pending.json").exists());
    assert!(!dir.0.join("updates/install-failed").exists());
    assert!(!dir.0.join("state/update-drain.json").exists());
    assert!(!dir.0.join("state/update-drained.json").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_cancel_preserves_pending_and_clears_drain_without_starting_wrong_unit() {
    let (dir, events) = run_linux(126, false);
    assert!(!events.contains("wrong-unit"), "{events}");
    assert!(dir.0.join("updates/pending.json").exists());
    assert!(fs::read_to_string(dir.0.join("updates/install-failed"))
        .unwrap()
        .contains("126"));
    assert!(!dir.0.join("state/update-drain.json").exists());
    assert!(!dir.0.join("state/update-drained.json").exists());
}

#[cfg(windows)]
#[test]
fn msi_success_waits_then_cleans_pending_and_drain_and_restarts_as_user() {
    let (dir, events) = run_windows(0, true, false, true);
    assert_eq!(events, "wait\ninstall\ntask\nui\n");
    for path in [
        "updates/pending.json",
        "updates/install-failed",
        "state/update-drain.json",
        "state/update-drained.json",
    ] {
        assert!(!dir.0.join(path).exists(), "{path}");
    }
    assert!(dir.0.join("updates/install.log").is_file());
}

#[cfg(windows)]
#[test]
fn msi_cancel_keeps_pending_marks_failure_clears_drain_and_recovers_old_daemon() {
    let (dir, events) = run_windows(1602, false, false, true);
    assert_eq!(events, "wait\ninstall\ndaemon\nui\n");
    assert!(dir.0.join("updates/pending.json").is_file());
    assert!(fs::read_to_string(dir.0.join("updates/install-failed"))
        .unwrap()
        .contains("1602"));
    assert!(!dir.0.join("state/update-drain.json").exists());
    assert!(!dir.0.join("state/update-drained.json").exists());
}

#[cfg(windows)]
#[test]
fn reboot_required_is_success_and_stopped_daemon_stays_stopped() {
    let (dir, events) = run_windows(3010, false, false, false);
    assert_eq!(events, "wait\ninstall\nui\n");
    assert!(!dir.0.join("updates/pending.json").exists());
}

#[cfg(windows)]
#[test]
fn registered_task_failure_falls_back_to_direct_start() {
    let (_, events) = run_windows(0, true, true, true);
    assert_eq!(events, "wait\ninstall\ndaemon\nui\n");
}

#[test]
fn generated_linux_helper_waits_elevates_only_dpkg_and_handles_both_outcomes() {
    let script = linux_script(&config("/home/a 'user/line\nbreak", true));
    assert!(script.find("kill -0").unwrap() < script.find("pkexec").unwrap());
    assert!(script.contains("pkexec /usr/bin/dpkg --install \"$package\""));
    assert!(script.contains("systemctl --user"));
    assert!(script.contains("install-failed") && script.contains("pending.json"));
    assert!(script.contains("update-drain.json") && script.contains("update-drained.json"));
    assert!(!script.contains("sudo") && !script.contains("kill -9"));
}
