//! Memory guard. A full RAM freezes some Linux desktops completely (swap thrashing long
//! before the OOM killer acts), and the agent's builds and test runs are the usual cause.
//! So roles wait while little memory is free, and a running job — with every process it
//! started — is killed once free memory drops below the floor, before the machine stalls.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mem {
    pub total_mb: u64,
    pub available_mb: u64,
}

/// `/proc/meminfo` → total and available (MemAvailable counts reclaimable cache too).
pub fn parse_meminfo(text: &str) -> Option<Mem> {
    let kb = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok());
    Some(Mem { total_mb: kb("MemTotal:")? / 1024, available_mb: kb("MemAvailable:")? / 1024 })
}

#[cfg(target_os = "linux")]
pub fn current() -> Option<Mem> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(windows)]
pub fn current() -> Option<Mem> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page: u64,
        avail_page: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_ext_virtual: u64,
    }
    const MB: u64 = 1024 * 1024;
    extern "system" {
        fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
    }
    let mut s = MemoryStatusEx { length: std::mem::size_of::<MemoryStatusEx>() as u32, load: 0, total_phys: 0, avail_phys: 0, total_page: 0, avail_page: 0, total_virtual: 0, avail_virtual: 0, avail_ext_virtual: 0 };
    // SAFETY: plain out-parameter struct with its size set, as the API requires
    (unsafe { GlobalMemoryStatusEx(&mut s) } != 0).then(|| Mem { total_mb: s.total_phys / MB, available_mb: s.avail_phys / MB })
}

/// Unknown platform (macOS …): no guard rather than a guess.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn current() -> Option<Mem> {
    None
}

/// Kill floor when not configured: 10 % of RAM, at least 1 GB, at most 4 GB.
pub fn auto_floor_mb(total_mb: u64) -> u64 {
    (total_mb / 10).clamp(1024, 4096)
}

/// Configured floors (0 = automatic); set by the daemon from `[resources]` every cycle.
static KILL_FLOOR_MB: AtomicU64 = AtomicU64::new(0);
static START_FLOOR_MB: AtomicU64 = AtomicU64::new(0);

pub fn configure(kill_floor_mb: u64, start_floor_mb: u64) {
    KILL_FLOOR_MB.store(kill_floor_mb, Ordering::Relaxed);
    START_FLOOR_MB.store(start_floor_mb, Ordering::Relaxed);
}

/// Below this, a running job is killed.
pub fn kill_floor_mb(m: &Mem) -> u64 {
    match KILL_FLOOR_MB.load(Ordering::Relaxed) {
        0 => auto_floor_mb(m.total_mb),
        v => v,
    }
}

/// Below this, no new work starts (default twice the kill floor: a test run needs room).
pub fn start_floor_mb(m: &Mem) -> u64 {
    match START_FLOOR_MB.load(Ordering::Relaxed) {
        0 => kill_floor_mb(m) * 2,
        v => v,
    }
}

/// `Some(reason)` when free memory is below `floor` (unknown memory = no objection).
pub fn below(floor: impl Fn(&Mem) -> u64) -> Option<String> {
    let m = current()?;
    let f = floor(&m);
    (m.available_mb < f).then(|| format!("Arbeitsspeicher knapp: {} MB frei von {} MB (Grenze {f} MB)", m.available_mb, m.total_mb))
}

/// Why a new job must not start now.
pub fn start_blocked() -> Option<String> {
    below(start_floor_mb)
}

/// Why a running job must be stopped now.
pub fn must_kill() -> Option<String> {
    below(kill_floor_mb)
}

/// Environment for builds and tests that keeps their memory use modest: workstation GC
/// (server GC reserves heaps per core) and no MSBuild nodes lingering after the build.
pub const LEAN_ENV: &[(&str, &str)] = &[("DOTNET_gcServer", "0"), ("MSBUILDDISABLENODEREUSE", "1"), ("DOTNET_CLI_TELEMETRY_OPTOUT", "1")];

/// Applies `LEAN_ENV` without overriding what the user set.
pub fn apply_lean_env(cmd: &mut std::process::Command) {
    for (k, v) in LEAN_ENV {
        if std::env::var_os(k).is_none() {
            cmd.env(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_meminfo() {
        let m = parse_meminfo("MemTotal:       32654420 kB\nMemFree:  1000 kB\nMemAvailable:   20480000 kB\n").unwrap();
        assert_eq!(m, Mem { total_mb: 31889, available_mb: 20000 });
        assert!(parse_meminfo("MemTotal: 1 kB\n").is_none());
    }

    #[test]
    fn automatic_floor_scales_with_ram() {
        assert_eq!(auto_floor_mb(8 * 1024), 1024);
        assert_eq!(auto_floor_mb(32 * 1024), 3276);
        assert_eq!(auto_floor_mb(128 * 1024), 4096);
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn this_machine_reports_memory() {
        let m = current().expect("memory readable");
        assert!(m.total_mb > 0 && m.available_mb <= m.total_mb);
        assert!(below(|_| 0).is_none() && below(|_| u64::MAX).unwrap().contains("Arbeitsspeicher knapp"));
    }
}
