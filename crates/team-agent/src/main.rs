//! lunima-team-agent — the Lunima agent team (Coder, QA, PR feedback, Product Owner) as one
//! native process on Windows and Linux.
//!
//!   lunima-team-agent init [--from-env <.env>] [--from-loop <agent-loop.json>]
//!   lunima-team-agent status
//!
//! The roles themselves (`run`) arrive in phase 2 — see docs/TEAM-AGENT.md.

use std::path::PathBuf;
use team_core::config::TeamConfig;
use team_core::paths::Paths;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("init") => init(&args[1..]),
        Some("status") => status(),
        Some("version" | "--version" | "-V") => {
            println!("lunima-team-agent {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!("Verwendung: lunima-team-agent <init|status|version>");
            2
        }
    };
    std::process::exit(code);
}

fn paths() -> Option<Paths> {
    Paths::default_root().map(Paths::new)
}

fn arg(args: &[String], name: &str) -> Option<PathBuf> {
    args.windows(2).find(|w| w[0] == name).map(|w| PathBuf::from(&w[1]))
}

/// Legacy files on this machine (Python agent .env, PO loop agent-loop.json).
fn legacy_candidates() -> (Option<PathBuf>, Option<PathBuf>) {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let env = home.as_ref().map(|h| h.join("autonomous-issue-agent").join(".env")).filter(|p| p.is_file());
    let lp = home.as_ref().map(|h| h.join("lunima-agent-loop").join("agent-loop.json")).filter(|p| p.is_file());
    (env, lp)
}

fn init(args: &[String]) -> i32 {
    let Some(p) = paths() else {
        eprintln!("Kein Datenordner bestimmbar (LOCALAPPDATA/HOME fehlt) — LUNIMA_TEAM_HOME setzen.");
        return 1;
    };
    if p.config().exists() {
        println!("{} existiert bereits — nichts geändert.", p.config().display());
        return 0;
    }
    let (auto_env, auto_loop) = legacy_candidates();
    let env = arg(args, "--from-env").or(auto_env);
    let lp = arg(args, "--from-loop").or(auto_loop);
    let cfg = team_core::import::from_legacy(env.as_deref(), lp.as_deref());
    if let Err(e) = p.ensure().map_err(|e| e.to_string()).and_then(|_| cfg.save(&p.config())) {
        eprintln!("Konnte {} nicht schreiben: {e}", p.config().display());
        return 1;
    }
    for (what, src) in [(".env", &env), ("agent-loop.json", &lp)] {
        match src {
            Some(path) => println!("übernommen aus {what}: {}", path.display()),
            None => println!("kein {what} gefunden — Standardwerte"),
        }
    }
    println!("Konfiguration geschrieben: {}", p.config().display());
    println!("{} Repos, Rechner „{}“. Geheimnisse wurden nicht übernommen (Claude: Abo-Login, GitHub: gh-Login).", cfg.repos.len(), cfg.machine);
    0
}

fn check(ok: bool, what: &str, detail: &str) {
    println!("  [{}] {what}: {detail}", if ok { "ok" } else { "!!" });
}

fn status() -> i32 {
    let Some(p) = paths() else {
        eprintln!("Kein Datenordner bestimmbar.");
        return 1;
    };
    println!("Datenordner: {}", p.root.display());
    let cfg = match TeamConfig::load(&p.config()) {
        Ok(c) => {
            check(true, "Konfiguration", &format!("{} Repos, Rechner „{}“", c.repos.len(), c.machine));
            Some(c)
        }
        Err(e) => {
            check(false, "Konfiguration", &format!("{e} → `lunima-team-agent init`"));
            None
        }
    };
    match team_core::claude::find_cli() {
        Some(cli) => check(true, "Claude-CLI", &cli.display().to_string()),
        None => check(false, "Claude-CLI", "nicht gefunden"),
    }
    match team_core::gh::current_user() {
        Ok(login) => check(true, "GitHub (gh)", &format!("angemeldet als {login}")),
        Err(e) => check(false, "GitHub (gh)", &format!("{e} → `gh auth login`")),
    }
    if let Some(c) = cfg {
        for r in &c.repos {
            println!("  Repo {} · Coder {} · {:?}", r.name, if r.coder { "an" } else { "aus" }, r.routing);
        }
    }
    0
}
