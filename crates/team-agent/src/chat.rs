//! The team chat on the agent side: a worker thread answers Max's messages to each role
//! while the roles keep working, and the `chat` / `config` commands let the roles talk to
//! each other, ask Max and change their own settings from any Claude session.
//!
//! An answer is a separate Claude session with the role's identity, its live status, its
//! memory and the conversation so far. It runs next to the role's work, never inside it,
//! so a long build never makes Max wait and a chat never disturbs a build.

use crate::daemon::role_enabled;
use chrono::Local;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use team_core::chat::{self, Kind, Message, MAX};
use team_core::config::TeamConfig;
use team_core::paths::Paths;
use team_core::provider::ChildEnv;

/// Nesting of `chat ask` (a role asking a role that asks a role …) is cut off here.
const DEPTH_ENV: &str = "LUNIMA_CHAT_DEPTH";
const MAX_DEPTH: u32 = 2;
const HISTORY: usize = 30;

/// This binary, quoted into prompts so sessions can call `chat` and `config`.
pub fn agent_exe() -> Option<String> {
    std::env::current_exe().ok().map(|p| p.display().to_string())
}

fn describe(role: &str) -> &'static str {
    match role {
        "product-owner" => "Tactical lead of the delivery team: keeps the backlog of agent-task issues filled, reviews and merges the agents' PRs into the integration branch, plans releases. Takes direction from Product Discovery and Max.",
        "coder" => "Implements agent-task issues as pull requests (worktree per issue, own reviewer pass, test gate), fixes QA findings.",
        "qa" => "Builds and tests every agent PR, reviews it and labels it qa-passed or qa-failed with findings.",
        "pr-feedback" => "Implements `@agent` comments that humans leave on pull requests.",
        "ux-review" => "At night, simulated users (personas) operate the desktop app; findings become ux-finding issues and a report for the product owner.",
        "product-discovery" => "The strategist (like a CEO): weekly evidence-based discovery of where the product should go — scout, strategist, critic — and passes direction to the product owner.",
        _ => "",
    }
}

fn model(cfg: &TeamConfig, role: &str) -> String {
    let m = &cfg.models;
    match role {
        "coder" => m.coder.clone(),
        "qa" | "pr-feedback" => m.reviewer.clone(),
        "product-discovery" if !cfg.product_discovery.model.is_empty() => cfg.product_discovery.model.clone(),
        _ => m.product_owner.clone(),
    }
}

/// What the role is doing right now, in words the model can relay.
fn live_status(paths: &Paths, cfg: &TeamConfig, role: &str) -> String {
    let mut out = Vec::new();
    out.push(if role_enabled(cfg, role) { "switched ON in team.toml".to_string() } else { format!("switched OFF in team.toml (roles.{} = false) — you do no work until it is switched on", role.replace('-', "_")) });
    if let Some(reason) = team_core::state::pause_reason(&paths.state(), role, Local::now()) {
        out.push(format!("paused: {reason}"));
    }
    match std::fs::read_to_string(paths.state().join(format!("status-{role}.json"))) {
        Ok(hb) => out.push(format!("heartbeat: {}", hb.trim())),
        Err(_) => out.push("no heartbeat yet (the team agent may not be running)".into()),
    }
    let tag = format!("[{role}]");
    let log = std::fs::read_to_string(paths.logs().join("agent.log")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().filter(|l| l.contains(&tag)).collect();
    if !lines.is_empty() {
        out.push(format!("your latest log lines:\n{}", lines[lines.len().saturating_sub(15)..].join("\n")));
    }
    out.join("\n")
}

fn transcript(history: &[Message], role: &str) -> String {
    history
        .iter()
        .map(|m| {
            let who = if m.from == role { "You".to_string() } else { chat::label(&m.from).to_string() };
            let to = if m.to == role || m.to == MAX { String::new() } else { format!(" → {}", chat::label(&m.to)) };
            let kind = match m.kind {
                Kind::Question => " (question)",
                Kind::Note => " (note)",
                Kind::Ask => " (asks)",
                Kind::Error => " (error)",
                Kind::Chat => "",
            };
            format!("[{}] {who}{to}{kind}: {}", m.at.get(..16).unwrap_or(&m.at), m.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The prompt for one answer of `role` to `incoming` (from Max or a teammate).
pub fn persona_prompt(paths: &Paths, cfg: &TeamConfig, role: &str, history: &[Message], incoming: &Message, exe: &str) -> String {
    let from_max = incoming.from == MAX;
    let audience = if from_max {
        "Max, your human boss, is writing to you in the Control Center's team chat. Answer him like a capable team member talks to the boss.".to_string()
    } else {
        format!("Your teammate {} ({}) asks you something and waits for your answer. Answer them directly; Max may read it later.", chat::label(&incoming.from), incoming.from)
    };
    format!(
        "You are the {label} (`{role}`) of the Lunima agent team. The team runs in `lunima-team-agent` on the machine \"{machine}\".\n\
{audience}\n\n\
Your job in the team: {job}\n\n\
## Your status right now\n{status}\n\n\
## Files\n\
- team configuration: {config} (change it only with `config set`, which validates)\n\
- your memory (standing instructions, part of every prompt you get): {memory}\n\
- your chat: {chat_file}\n\
- logs: {logs} (agent.log; raw Claude sessions of your work in {logs}{sep}{role})\n\
- state: {state}\n\n\
## Commands (shell tool)\n\
- \"{exe}\" config set <dotted.path> <value>   e.g. `roles.product_owner true`, `product_owner.repo aignermax/Lunima`, `budgets.max_turns_regular 200`\n\
- \"{exe}\" config add-repo <owner/name>   /   config remove-repo <owner/name>   (repositories the coder works on)\n\
- \"{exe}\" status   (configuration, Claude CLI, gh login, repositories)\n\
- gh is logged in as the team's GitHub user; git and the usual tools are available.\n\
{howto}\n\n\
## How to answer\n\
- Briefly and concretely, in the language the message is written in (Max usually writes German). Plain text, short paragraphs, no tables.\n\
- Questions about what you do or why: look it up (status, logs, issues, PRs, code) before you answer. Never guess.\n\
- A lasting instruction (\"from now on …\", a direction, a preference): store it with `chat remember --role {role}` and say so. It then applies to all your future work.\n\
- Settings (switch a role on/off, repositories, models, budgets): change them with `config set` / `add-repo` and confirm what changed. They apply from the next cycle, no restart.\n\
- Something a teammate should know or decide: `chat note` or `chat ask` them. Strategy goes from Product Discovery to the product owner; the product owner decides tactics.\n\
- Extra tasks: quick ones (look something up, review a piece of code, search this computer) you do right here. Real development work becomes a GitHub issue with the label `{task}` in the right repository, so the normal pipeline picks it up; tell the product owner with a note. Don't change repository code from the chat.\n\
- Your regular work continues in parallel; this chat is a separate session. Don't start builds or test runs from the chat: the machine's memory belongs to the real work. Reading code, logs and GitHub is fine.\n\
- Never print secrets or keys.\n\n\
## Conversation so far (oldest first)\n{history}\n\n\
## New message\n{who}: {text}\n",
        label = chat::label(role),
        machine = cfg.machine,
        job = describe(role),
        status = live_status(paths, cfg, role),
        config = paths.config().display(),
        memory = chat::memory_file(paths, role).display(),
        chat_file = chat::chat_file(paths, role).display(),
        logs = paths.logs().display(),
        sep = std::path::MAIN_SEPARATOR,
        state = paths.state().display(),
        howto = chat::howto(exe, role),
        task = cfg.labels.task,
        history = if history.is_empty() { "(none)".into() } else { transcript(history, role) },
        who = chat::label(&incoming.from),
        text = incoming.text,
    )
}

/// One answer of `role` to `incoming`; the reply text or why there is none.
pub fn answer(paths: &Paths, role: &str, incoming: &Message) -> Result<String, String> {
    let cfg = TeamConfig::load(&paths.config())?;
    let exe = agent_exe().ok_or("Pfad des Team-Agents unbekannt")?;
    let mut history = chat::tail(paths, role, HISTORY + 1);
    history.retain(|m| m.id != incoming.id);
    let prompt = persona_prompt(paths, &cfg, role, &history, incoming, &exe);
    let depth: u32 = std::env::var(DEPTH_ENV).ok().and_then(|d| d.parse().ok()).unwrap_or(0);
    let mut env = ChildEnv::subscription();
    env.set.insert(DEPTH_ENV.into(), (depth + 1).to_string());
    let logs = paths.logs().join("chat");
    let _ = std::fs::create_dir_all(&logs);
    let session = team_core::claude::Session {
        prompt,
        cwd: paths.root.clone(),
        model: model(&cfg, role),
        max_turns: 60,
        env,
        settings_file: None,
        log_file: Some(logs.join(format!("{}_{role}.jsonl", Local::now().format("%Y-%m-%d_%H%M%S")))),
        inactivity: Duration::from_secs(300),
        deadline: Some(Duration::from_secs(20 * 60)),
        on_tick: None,
        tools: None,
        memory_guard: false,
    };
    let out = team_core::claude::run(&session).map_err(|e| e.to_string())?;
    if let Some(until) = out.usage_limit_until {
        return Err(format!("Claude-Nutzungslimit erreicht — ich antworte ab {} wieder", until.format("%H:%M")));
    }
    let text = out.text.trim();
    if text.is_empty() {
        return Err(format!("keine Antwort erhalten ({})", out.subtype));
    }
    Ok(text.to_string())
}

/// Answers Max's messages, one conversation per role at a time, all roles in parallel.
pub fn spawn_worker(paths: Paths) {
    for (role, _) in chat::ROLES {
        chat::set_busy(&paths, role, false);
    }
    let busy: Arc<Mutex<HashSet<&'static str>>> = Arc::default();
    std::thread::Builder::new()
        .name("chat".into())
        .spawn(move || loop {
            for (role, _) in chat::ROLES {
                if busy.lock().unwrap_or_else(|p| p.into_inner()).contains(role) {
                    continue;
                }
                let Some(msg) = chat::pending(&chat::read(&paths, role), role) else { continue };
                busy.lock().unwrap_or_else(|p| p.into_inner()).insert(role);
                chat::set_busy(&paths, role, true);
                let (paths, busy) = (paths.clone(), busy.clone());
                std::thread::spawn(move || {
                    crate::log::set_role("chat");
                    let reply = match answer(&paths, role, &msg) {
                        Ok(text) => Message::new(role, MAX, Kind::Chat, &text),
                        Err(e) => {
                            crate::log::warn(&format!("Chat {role}: {e}"));
                            Message::new(role, MAX, Kind::Error, &format!("Ich konnte gerade nicht antworten: {e}"))
                        }
                    };
                    if let Err(e) = chat::post(&paths, &reply.reply(&msg)) {
                        crate::log::error(&format!("Chat {role}: Antwort nicht gespeichert ({e})"));
                    }
                    chat::set_busy(&paths, role, false);
                    busy.lock().unwrap_or_else(|p| p.into_inner()).remove(role);
                });
            }
            std::thread::sleep(Duration::from_secs(2));
        })
        .expect("spawn chat worker");
}

/// `--flag value` pairs, `--switch`es and the remaining words (the text).
struct Args {
    flags: Vec<(String, String)>,
    switches: Vec<String>,
    words: Vec<String>,
}

const SWITCHES: &[&str] = &["--question"];

fn parse(args: &[String]) -> Args {
    let (mut flags, mut switches, mut words) = (Vec::new(), Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if SWITCHES.contains(&a.as_str()) {
            switches.push(a.clone());
        } else if let Some(name) = a.strip_prefix("--") {
            flags.push((name.to_string(), it.next().cloned().unwrap_or_default()));
        } else {
            words.push(a.clone());
        }
    }
    Args { flags, switches, words }
}

impl Args {
    fn get(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn text(&self) -> String {
        self.words.join(" ")
    }
}

fn role_arg(a: &Args, name: &str) -> Result<String, String> {
    let r = a.get(name).ok_or_else(|| format!("--{name} <rolle> fehlt"))?;
    if chat::is_role(r) {
        Ok(r.to_string())
    } else {
        Err(format!("--{name} {r}: unbekannte Rolle ({})", chat::ROLES.iter().map(|(r, _)| *r).collect::<Vec<_>>().join(", ")))
    }
}

/// `lunima-team-agent chat …`
pub fn cli(paths: &Paths, args: &[String]) -> Result<String, String> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    let a = parse(args.get(1..).unwrap_or_default());
    let text = a.text();
    let need_text = || if text.trim().is_empty() { Err("Text fehlt".to_string()) } else { Ok(()) };
    match sub {
        "say" => {
            need_text()?;
            let from = role_arg(&a, "from")?;
            let kind = if a.switches.iter().any(|s| s == "--question") { Kind::Question } else { Kind::Chat };
            chat::post(paths, &Message::new(&from, MAX, kind, &text))?;
            Ok("Nachricht an Max abgelegt (er sieht sie im Control Center).".into())
        }
        "note" => {
            need_text()?;
            let (from, to) = (role_arg(&a, "from")?, role_arg(&a, "to")?);
            chat::post(paths, &Message::new(&from, &to, Kind::Note, &text))?;
            Ok(format!("Notiz für {to} abgelegt (steht in seinem nächsten Prompt)."))
        }
        "ask" => {
            need_text()?;
            let (from, to) = (role_arg(&a, "from")?, role_arg(&a, "to")?);
            let depth: u32 = std::env::var(DEPTH_ENV).ok().and_then(|d| d.parse().ok()).unwrap_or(0);
            if depth >= MAX_DEPTH {
                return Err("zu viele verschachtelte Rückfragen — hinterlasse stattdessen eine Notiz (chat note)".into());
            }
            let question = Message::new(&from, &to, Kind::Ask, &text);
            chat::post(paths, &question)?;
            chat::set_busy(paths, &to, true);
            let result = answer(paths, &to, &question);
            chat::set_busy(paths, &to, false);
            let reply = result?;
            chat::post(paths, &Message::new(&to, &from, Kind::Chat, &reply).reply(&question))?;
            Ok(reply)
        }
        "remember" => {
            need_text()?;
            let role = role_arg(&a, "role")?;
            chat::remember(paths, &role, &text)?;
            Ok(format!("Gemerkt in {}", chat::memory_file(paths, &role).display()))
        }
        "read" => {
            let role = role_arg(&a, "role")?;
            let n = a.get("last").and_then(|n| n.parse().ok()).unwrap_or(20);
            Ok(transcript(&chat::tail(paths, &role, n), &role))
        }
        _ => Err("Verwendung: chat say --from <rolle> [--question] <text> | note --from <rolle> --to <rolle> <text> | ask --from <rolle> --to <rolle> <frage> | remember --role <rolle> <text> | read --role <rolle> [--last N]".into()),
    }
}

/// `lunima-team-agent config …` — validated changes to team.toml.
pub fn config_cli(path: &Path, args: &[String]) -> Result<String, String> {
    let cfg = TeamConfig::load(path)?;
    match args {
        [cmd, key, value @ ..] if cmd == "set" && !value.is_empty() => {
            let updated = cfg.with_setting(key, &value.join(" "))?;
            updated.save(path)?;
            Ok(format!("{key} = {} — gilt ab dem nächsten Zyklus", value.join(" ")))
        }
        [cmd, repo] if cmd == "add-repo" => {
            let mut updated = cfg.clone();
            if updated.repo(repo).is_some() {
                return Ok(format!("{repo} ist schon eingetragen"));
            }
            updated.repos.push(team_core::config::RepoConfig { name: repo.trim().to_string(), ..Default::default() });
            updated.validate()?;
            updated.save(path)?;
            Ok(format!("{repo} hinzugefügt"))
        }
        [cmd, repo] if cmd == "remove-repo" => {
            let mut updated = cfg.clone();
            let before = updated.repos.len();
            updated.repos.retain(|r| !r.name.eq_ignore_ascii_case(repo));
            if updated.repos.len() == before {
                return Err(format!("{repo} ist nicht eingetragen"));
            }
            updated.save(path)?;
            Ok(format!("{repo} entfernt"))
        }
        [cmd] if cmd == "show" => std::fs::read_to_string(path).map_err(|e| e.to_string()),
        _ => Err("Verwendung: config set <pfad> <wert> | add-repo <owner/name> | remove-repo <owner/name> | show".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn home() -> (tempfile::TempDir, Paths) {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::new(t.path());
        p.ensure().unwrap();
        TeamConfig::default().save(&p.config()).unwrap();
        (t, p)
    }

    #[test]
    fn roles_leave_notes_questions_and_memories() {
        let (_t, p) = home();
        cli(&p, &s(&["note", "--from", "product-discovery", "--to", "product-owner", "Richtung:", "eigener", "Chip"])).unwrap();
        assert_eq!(chat::read(&p, "product-owner")[0].text, "Richtung: eigener Chip");
        cli(&p, &s(&["say", "--from", "coder", "--question", "Rust oder C#?"])).unwrap();
        assert_eq!(chat::unread_for_max(&chat::read(&p, "coder"), "coder"), 1);
        cli(&p, &s(&["remember", "--role", "coder", "Rust bevorzugen"])).unwrap();
        assert!(chat::memory(&p, "coder").contains("Rust bevorzugen"));
        assert!(cli(&p, &s(&["note", "--from", "coder", "--to", "boss", "x"])).is_err());
        assert!(cli(&p, &s(&["say", "--from", "coder"])).unwrap_err().contains("Text fehlt"));
    }

    #[test]
    fn nested_asks_are_cut_off() {
        let (_t, p) = home();
        std::env::set_var(DEPTH_ENV, MAX_DEPTH.to_string());
        let err = cli(&p, &s(&["ask", "--from", "coder", "--to", "product-owner", "Darf ich?"])).unwrap_err();
        std::env::remove_var(DEPTH_ENV);
        assert!(err.contains("verschachtelt"));
    }

    #[test]
    fn config_commands_validate_and_save() {
        let (_t, p) = home();
        config_cli(&p.config(), &s(&["set", "roles.product_owner", "true"])).unwrap();
        config_cli(&p.config(), &s(&["add-repo", "aignermax/Lunima"])).unwrap();
        let cfg = TeamConfig::load(&p.config()).unwrap();
        assert!(cfg.roles.product_owner && cfg.repo("aignermax/Lunima").is_some());
        assert!(config_cli(&p.config(), &s(&["add-repo", "kein-slash"])).is_err());
        assert!(config_cli(&p.config(), &s(&["set", "poll_seconds", "0"])).is_err());
        config_cli(&p.config(), &s(&["remove-repo", "aignermax/Lunima"])).unwrap();
        assert!(TeamConfig::load(&p.config()).unwrap().repos.is_empty());
    }

    #[test]
    fn persona_prompt_carries_status_history_and_rules() {
        let (_t, p) = home();
        let cfg = TeamConfig::default();
        let q = Message::new(MAX, "product-owner", Kind::Chat, "Bist du aktiv?");
        let history = vec![Message::new("coder", "product-owner", Kind::Note, "PR 12 wartet")];
        let prompt = persona_prompt(&p, &cfg, "product-owner", &history, &q, "agent.exe");
        assert!(prompt.contains("switched OFF"), "the PO is off by default and must know it");
        assert!(prompt.contains("PR 12 wartet") && prompt.contains("Bist du aktiv?"));
        assert!(prompt.contains("config set") && prompt.contains("chat remember --role product-owner"));
    }
}
