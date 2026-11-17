//! lunima-team-agent — the Lunima agent team (Coder, QA, PR feedback, Product Owner) as one
//! native process on Windows and Linux.
//!
//!   lunima-team-agent init [--from-env <.env>] [--from-loop <agent-loop.json>]
//!   lunima-team-agent status
//!   lunima-team-agent run                 all roles, forever (one instance per machine)
//!   lunima-team-agent once <role>         one cycle of coder | qa | pr-feedback | product-owner | ux-review | product-discovery
//!   lunima-team-agent experiment publish <repo clone> <slug> --approved-by <name> --contact <address> [--goatcounter <code>]
//!                                         the maintainer's step that makes an experiment page public (writes files only)
//!   lunima-team-agent experiment preview <repo clone> <out dir>
//!   lunima-team-agent chat say|note|ask|remember|read …   team chat (used by the roles' sessions)
//!   lunima-team-agent config set <path> <value> | add-repo | remove-repo | show

use std::path::PathBuf;
use team_core::config::TeamConfig;
use team_core::paths::Paths;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--home <dir>` anywhere = data folder (the Control Center passes it for non-default homes)
    if let Some(i) = args.iter().position(|a| a == "--home") {
        if let Some(dir) = args.get(i + 1).cloned() {
            std::env::set_var(team_core::paths::ENV_HOME, dir);
            args.drain(i..=i + 1);
        }
    }
    let code = match args.first().map(String::as_str) {
        Some("init") => init(&args[1..]),
        Some("status") => status(),
        Some("run") => run(),
        Some("once") => once(args.get(1).map(String::as_str).unwrap_or("")),
        Some("experiment") => experiment(&args[1..]),
        Some("chat") => team_cmd(|p| team_agent::chat::cli(p, &args[1..])),
        Some("config") => team_cmd(|p| team_agent::chat::config_cli(&p.config(), &args[1..])),
        Some("version" | "--version" | "-V") => {
            println!("lunima-team-agent {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!("Verwendung: lunima-team-agent <init|status|run|once <rolle>|chat …|config …|experiment publish|preview …|version>");
            2
        }
    };
    std::process::exit(code);
}

/// Runs a team command (`chat`, `config`): the result on stdout, an error on stderr.
fn team_cmd(f: impl FnOnce(&Paths) -> Result<String, String>) -> i32 {
    let Some(p) = paths() else {
        eprintln!("Kein Datenordner bestimmbar (LOCALAPPDATA/HOME fehlt) — LUNIMA_TEAM_HOME setzen.");
        return 1;
    };
    match f(&p) {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn paths() -> Option<Paths> {
    Paths::default_root().map(Paths::new)
}

fn arg(args: &[String], name: &str) -> Option<PathBuf> {
    args.windows(2).find(|w| w[0] == name).map(|w| PathBuf::from(&w[1]))
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
    let (auto_env, auto_loop) = team_core::import::legacy_candidates();
    let env = arg(args, "--from-env").or(auto_env);
    let lp = arg(args, "--from-loop").or(auto_loop);
    let cfg = match team_core::import::from_legacy(env.as_deref(), lp.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Übernahme fehlgeschlagen: {e}
Nichts geschrieben — Pfad prüfen oder ohne --from-… starten.");
            return 1;
        }
    };
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
    let backend = cfg.as_ref().map(|c| c.runner.backend).unwrap_or_default();
    let cli = match backend {
        team_core::config::Backend::Claude | team_core::config::Backend::Openrouter | team_core::config::Backend::Eco => team_core::claude::find_cli(),
        team_core::config::Backend::Codex | team_core::config::Backend::OpenAi => team_core::codex::find_cli(),
    };
    let title = format!("{}-CLI", backend.label());
    match cli {
        Some(cli) => {
            check(true, &title, &cli.display().to_string());
            if backend == team_core::config::Backend::Codex {
                let mut cmd = team_core::proc::command(cli);
                cmd.args(["login", "status"]);
                team_core::provider::ChildEnv::codex_subscription().apply(&mut cmd);
                match team_core::proc::run(cmd, None, std::time::Duration::from_secs(20)) {
                    Ok(out) => {
                        let chatgpt = out.ok() && format!("{} {}", out.stdout, out.stderr).to_ascii_lowercase().contains("chatgpt");
                        check(chatgpt, "Codex-Anmeldung", if chatgpt { "ChatGPT" } else { "ChatGPT-Login erforderlich: `codex login`" });
                    }
                    Err(e) => check(false, "Codex-Anmeldung", &e.to_string()),
                }
            }
        }
        None => check(false, &title, "nicht gefunden"),
    }
    match team_core::gh::current_user() {
        Ok(login) => check(true, "GitHub (gh)", &format!("angemeldet als {login}")),
        Err(e) => check(false, "GitHub (gh)", &format!("{e} → `gh auth login`")),
    }
    if let Some(c) = cfg {
        if matches!(backend, team_core::config::Backend::Openrouter | team_core::config::Backend::Eco | team_core::config::Backend::OpenAi) {
            match team_core::provider::runner_choice(&c.runner, &c.providers) {
                Ok(Some(choice)) => check(true, "API-Provider", &format!("{} / {} (Key konfiguriert)", backend.label(), choice.model)),
                Err(e) => check(false, "API-Provider", &e),
                _ => {}
            }
        }
        for r in &c.repos {
            let routing = if backend == team_core::config::Backend::Claude { format!("{:?}", r.routing) } else { format!("{} (global)", backend.label()) };
            println!("  Repo {} · Coder {} · {}", r.name, if r.coder { "an" } else { "aus" }, routing);
        }
    }
    0
}

/// Holds `state/agent.lock` for the life of the process (OS file lock: released on crash).
fn single_instance(p: &Paths) -> Option<std::fs::File> {
    let _ = p.ensure();
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.state().join("agent.lock")).ok()?;
    file.try_lock().ok()?;
    // the lock file itself is unreadable for others on Windows: the pid goes next to it
    let _ = std::fs::write(p.state().join("agent.pid"), std::process::id().to_string());
    Some(file)
}

fn ready_paths() -> Option<Paths> {
    let Some(p) = paths() else {
        eprintln!("Kein Datenordner bestimmbar — LUNIMA_TEAM_HOME setzen.");
        return None;
    };
    if let Err(e) = TeamConfig::load(&p.config()) {
        eprintln!("{e}\n→ zuerst `lunima-team-agent init`");
        return None;
    }
    Some(p)
}

fn run() -> i32 {
    let Some(p) = ready_paths() else { return 1 };
    let Some(_lock) = single_instance(&p) else {
        eprintln!("Der Team-Agent läuft auf diesem Rechner bereits.");
        return 3;
    };
    team_agent::daemon::run(p)
}

fn once(role: &str) -> i32 {
    let Some(p) = ready_paths() else { return 1 };
    let Some(_lock) = single_instance(&p) else {
        eprintln!("Der Team-Agent läuft bereits — `once` würde mit ihm konkurrieren.");
        return 3;
    };
    let Some(mut r) = team_agent::daemon::all_roles().into_iter().find(|r| r.name() == role) else {
        eprintln!("Unbekannte Rolle „{role}“ (coder | qa | pr-feedback | product-owner | ux-review | product-discovery)");
        return 2;
    };
    team_agent::log::init(&p.logs());
    team_agent::log::set_role(role);
    let cfg = TeamConfig::load(&p.config()).expect("checked above");
    let login = match team_core::gh::current_user() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("GitHub: {e} → `gh auth login`");
            return 1;
        }
    };
    let hb = std::sync::Arc::new(team_core::state::Heartbeat::new(&p.state(), role));
    let ctx = team_agent::daemon::context(cfg, &p, &login, hb.clone(), role);
    r.recover(&ctx);
    let result = r.cycle(&ctx);
    hb.idle();
    match result {
        Ok(true) => println!("{role}: Arbeit erledigt"),
        Ok(false) => println!("{role}: nichts zu tun"),
        Err(e) => {
            eprintln!("{role}: {e}");
            return 1;
        }
    }
    0
}

/// `experiment publish|preview`: runs on the maintainer's machine, never inside the daemon.
fn experiment(args: &[String]) -> i32 {
    use team_agent::product_discovery::page;
    let flag = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    match (args.first().map(String::as_str), args.get(1), args.get(2)) {
        (Some("publish"), Some(repo), Some(slug)) => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            match page::publish(std::path::Path::new(repo), slug, &flag("--approved-by").unwrap_or_default(), &flag("--contact").unwrap_or_default(), flag("--goatcounter"), &today) {
                Ok(out) => {
                    println!("Geschrieben: {}\nPrüfen, committen und selbst nach main mergen — erst dann ist die Seite öffentlich.", out.display());
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        (Some("preview"), Some(repo), Some(out)) => {
            let done = page::render_previews(std::path::Path::new(repo), std::path::Path::new(out));
            println!("{} Vorschau(en): {}", done.len(), done.join(", "));
            0
        }
        _ => {
            eprintln!("Verwendung: lunima-team-agent experiment publish <klon> <slug> --approved-by <name> --contact <adresse> [--goatcounter <code>]\n            lunima-team-agent experiment preview <klon> <ausgabeordner>");
            2
        }
    }
}
