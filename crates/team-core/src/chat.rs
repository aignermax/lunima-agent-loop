//! Team chat and memory. Max talks to every role like a boss to his team; roles answer,
//! ask him questions, leave each other notes and keep standing instructions.
//!
//!   <root>/team/chat/<role>.jsonl   one message per line, append-only. Several processes
//!                                   write (Control Center, daemon, `chat` CLI); single-line
//!                                   appends keep that safe without a lock.
//!   <root>/team/chat/<role>.busy    exists while the role is composing an answer
//!   <root>/team/memory/<role>.md    what the role was told to keep in mind; it is part of
//!                                   every prompt the role gets, work and chat alike

use crate::paths::Paths;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// The human (sender/recipient name in messages).
pub const MAX: &str = "max";

/// Role keys in pipeline order, with the German names the Control Center shows.
pub const ROLES: [(&str, &str); 6] = [("product-owner", "Product Owner"), ("coder", "Coder"), ("qa", "QA-Tester"), ("pr-feedback", "PR-Feedback"), ("ux-review", "UX-Review"), ("product-discovery", "Product Discovery")];

pub fn is_role(name: &str) -> bool {
    ROLES.iter().any(|(r, _)| *r == name)
}

pub fn label(name: &str) -> &'static str {
    if name == MAX {
        return "Max";
    }
    ROLES.iter().find(|(r, _)| *r == name).map(|(_, l)| *l).unwrap_or("Rolle")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// conversation with Max
    #[default]
    Chat,
    /// a role needs a decision or information from Max
    Question,
    /// a role informs a teammate (read in the teammate's next session)
    Note,
    /// one role asks another and waits for the answer (`chat ask`)
    Ask,
    /// the role could not answer (Claude failed, usage limit …)
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    /// RFC 3339, local time
    pub at: String,
    pub from: String,
    pub to: String,
    pub text: String,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

static SEQ: AtomicU32 = AtomicU32::new(0);

impl Message {
    pub fn new(from: &str, to: &str, kind: Kind, text: &str) -> Self {
        let now = Local::now();
        let id = format!("{}-{}-{}", now.format("%Y%m%d%H%M%S%3f"), std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
        Self { id, at: now.to_rfc3339(), from: from.into(), to: to.into(), text: text.trim().into(), kind, reply_to: None }
    }

    pub fn reply(mut self, to: &Message) -> Self {
        self.reply_to = Some(to.id.clone());
        self
    }

    pub fn time(&self) -> Option<DateTime<Local>> {
        DateTime::parse_from_rfc3339(&self.at).ok().map(|t| t.with_timezone(&Local))
    }
}

pub fn dir(paths: &Paths) -> PathBuf {
    paths.root.join("team")
}

pub fn chat_file(paths: &Paths, role: &str) -> PathBuf {
    dir(paths).join("chat").join(format!("{role}.jsonl"))
}

pub fn busy_file(paths: &Paths, role: &str) -> PathBuf {
    dir(paths).join("chat").join(format!("{role}.busy"))
}

pub fn memory_file(paths: &Paths, role: &str) -> PathBuf {
    dir(paths).join("memory").join(format!("{role}.md"))
}

fn append_line(path: &PathBuf, line: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(format!("{line}\n").as_bytes()).map_err(|e| e.to_string())
}

/// Appends the message to the chat of every role taking part (Max has no file of his own:
/// his conversations live in the roles' chats).
pub fn post(paths: &Paths, msg: &Message) -> Result<(), String> {
    let line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    let mut targets: Vec<&str> = [msg.from.as_str(), msg.to.as_str()].into_iter().filter(|n| is_role(n)).collect();
    targets.dedup();
    if targets.is_empty() {
        return Err(format!("unbekannte Rolle: {} → {}", msg.from, msg.to));
    }
    for role in targets {
        append_line(&chat_file(paths, role), &line)?;
    }
    Ok(())
}

/// The whole chat of a role, oldest first (broken lines are skipped).
pub fn read(paths: &Paths, role: &str) -> Vec<Message> {
    std::fs::read_to_string(chat_file(paths, role)).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

/// The last `n` messages.
pub fn tail(paths: &Paths, role: &str, n: usize) -> Vec<Message> {
    let all = read(paths, role);
    all[all.len().saturating_sub(n)..].to_vec()
}

/// Max's latest message to `role` when the role has not answered it yet. Only Max's
/// messages get automatic answers: notes between roles never start a conversation loop.
pub fn pending(messages: &[Message], role: &str) -> Option<Message> {
    let last_from_max = messages.iter().rposition(|m| m.from == MAX && m.to == role)?;
    let answered = messages[last_from_max + 1..].iter().any(|m| m.from == role && m.to == MAX);
    (!answered).then(|| messages[last_from_max].clone())
}

/// Questions and answers of the role that Max has not reacted to yet (badge count).
pub fn unread_for_max(messages: &[Message], role: &str) -> usize {
    let since = messages.iter().rposition(|m| m.from == MAX).map_or(0, |i| i + 1);
    messages[since..].iter().filter(|m| m.from == role && m.to == MAX && m.kind == Kind::Question).count()
}

pub fn is_busy(paths: &Paths, role: &str) -> bool {
    busy_file(paths, role).exists()
}

pub fn set_busy(paths: &Paths, role: &str, busy: bool) {
    let f = busy_file(paths, role);
    if busy {
        let _ = f.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(&f, Local::now().to_rfc3339());
    } else {
        let _ = std::fs::remove_file(&f);
    }
}

pub fn memory(paths: &Paths, role: &str) -> String {
    std::fs::read_to_string(memory_file(paths, role)).unwrap_or_default()
}

/// Adds one standing instruction to the role's memory.
pub fn remember(paths: &Paths, role: &str, text: &str) -> Result<(), String> {
    if !is_role(role) {
        return Err(format!("unbekannte Rolle: {role}"));
    }
    let path = memory_file(paths, role);
    if !path.exists() {
        append_line(&path, &format!("# Gedächtnis: {}\n", label(role)))?;
    }
    append_line(&path, &format!("- ({}) {}", Local::now().format("%Y-%m-%d"), text.trim().replace('\n', " ")))
}

/// Notes teammates left for `role` within the last `days`, oldest first.
pub fn notes_for(paths: &Paths, role: &str, days: i64) -> Vec<Message> {
    let since = Local::now() - chrono::Duration::days(days);
    read(paths, role).into_iter().filter(|m| m.to == role && is_role(&m.from) && m.from != role && m.time().is_some_and(|t| t >= since)).collect()
}

/// How a role talks to the team, for its prompts. `exe` is the agent binary.
pub fn howto(exe: &str, role: &str) -> String {
    format!(
        "Team communication (you are `{role}`; run these commands with the shell tool):\n\
- Ask a teammate and wait for the answer: \"{exe}\" chat ask --from {role} --to <role> \"<question>\"\n\
- Leave a teammate a note for their next session: \"{exe}\" chat note --from {role} --to <role> \"<text>\"\n\
- Ask Max (the human boss) a question; he answers in the Control Center, not right away: \"{exe}\" chat say --from {role} --question \"<question>\"\n\
- Keep a standing instruction in your own memory: \"{exe}\" chat remember --role {role} \"<text>\"\n\
Roles: product-owner (tactical lead: backlog, review, merge), product-discovery (strategy, direction), coder, qa, pr-feedback, ux-review.\n\
Ask your product owner first; only ask Max when the product owner cannot decide or is switched off. Don't wait for Max: carry on with the rest of your work."
    )
}

/// The block appended to every prompt of `role`: its memory, recent notes from teammates
/// and how to reach the team. `exe` = the agent binary; `None` for a session without a
/// shell (sandboxed), which gets memory and notes but no commands it couldn't run.
pub fn prompt_section(paths: &Paths, role: &str, exe: Option<&str>) -> String {
    let mut out = String::from("\n\n---\n## Team\n");
    let mem = memory(paths, role);
    if !mem.trim().is_empty() {
        out.push_str("Standing instructions you were given (follow them unless the task above explicitly overrides them):\n");
        out.push_str(mem.trim());
        out.push_str("\n\n");
    }
    let notes = notes_for(paths, role, 14);
    if !notes.is_empty() {
        out.push_str("Recent notes from teammates:\n");
        for m in notes.iter().rev().take(10).rev() {
            out.push_str(&format!("- {} from {}: {}\n", m.at.get(..16).unwrap_or(&m.at), m.from, m.text.replace('\n', " ")));
        }
        out.push('\n');
    }
    if let Some(exe) = exe {
        out.push_str(&howto(exe, role));
    } else if mem.trim().is_empty() && notes.is_empty() {
        return String::new();
    }
    out
}

/// The role a prompt template belongs to (`None`: a prompt that isn't one role's).
pub fn role_of_prompt(name: &str) -> Option<&'static str> {
    Some(match name {
        n if n.starts_with("coder-") || n == "reviewer" || n == "qa-fix" => "coder",
        n if n.starts_with("qa-") => "qa",
        "pr-feedback" => "pr-feedback",
        "po-owner" => "product-owner",
        n if n.starts_with("discovery-") => "product-discovery",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, Paths) {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::new(t.path());
        (t, p)
    }

    #[test]
    fn messages_land_in_every_participating_role() {
        let (_t, p) = home();
        post(&p, &Message::new(MAX, "coder", Kind::Chat, "Warum Digital Fab?")).unwrap();
        post(&p, &Message::new("coder", "product-owner", Kind::Note, "Frage zur Fab")).unwrap();
        assert_eq!(read(&p, "coder").len(), 2);
        assert_eq!(read(&p, "product-owner").len(), 1);
        assert!(post(&p, &Message::new(MAX, "nobody", Kind::Chat, "x")).is_err());
    }

    #[test]
    fn only_max_messages_wait_for_an_answer() {
        let (_t, p) = home();
        let q = Message::new(MAX, "coder", Kind::Chat, "Bist du aktiv?");
        post(&p, &q).unwrap();
        assert_eq!(pending(&read(&p, "coder"), "coder").unwrap().id, q.id);
        post(&p, &Message::new("coder", MAX, Kind::Chat, "Ja").reply(&q)).unwrap();
        assert!(pending(&read(&p, "coder"), "coder").is_none());
        post(&p, &Message::new("qa", "coder", Kind::Note, "Build rot")).unwrap();
        assert!(pending(&read(&p, "coder"), "coder").is_none(), "a teammate's note starts no conversation");
    }

    #[test]
    fn questions_count_until_max_writes() {
        let (_t, p) = home();
        post(&p, &Message::new("coder", MAX, Kind::Question, "Rust oder C#?")).unwrap();
        assert_eq!(unread_for_max(&read(&p, "coder"), "coder"), 1);
        post(&p, &Message::new(MAX, "coder", Kind::Chat, "Rust")).unwrap();
        assert_eq!(unread_for_max(&read(&p, "coder"), "coder"), 0);
    }

    #[test]
    fn memory_and_notes_reach_the_prompt() {
        let (_t, p) = home();
        assert!(prompt_section(&p, "coder", Some("agent")).contains("chat ask --from coder"));
        remember(&p, "coder", "Neue Module in Rust schreiben").unwrap();
        post(&p, &Message::new("product-discovery", "coder", Kind::Note, "Richtung: eigener Chip")).unwrap();
        let s = prompt_section(&p, "coder", Some("agent"));
        assert!(!prompt_section(&p, "coder", None).contains("chat ask"), "no commands without a shell");
        assert!(s.contains("Neue Module in Rust schreiben") && s.contains("eigener Chip"));
        assert!(remember(&p, "nobody", "x").is_err());
    }

    #[test]
    fn prompts_map_to_roles() {
        assert_eq!(role_of_prompt("coder-initial"), Some("coder"));
        assert_eq!(role_of_prompt("reviewer"), Some("coder"));
        assert_eq!(role_of_prompt("qa-review"), Some("qa"));
        assert_eq!(role_of_prompt("po-owner"), Some("product-owner"));
        assert_eq!(role_of_prompt("discovery-scout"), Some("product-discovery"));
        assert_eq!(role_of_prompt("unknown"), None);
    }
}
