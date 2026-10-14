//! The PR comment thread as the role's shared state. Several machines run this role with
//! the same GitHub account, so progress is recorded in hidden markers inside the agent's
//! own comments instead of a local file:
//!
//! * `<!-- pr-feedback:claim <id> machine=<name> -->` — work on trigger comment `<id>` started
//! * `<!-- pr-feedback:failed <id> -->` — that attempt failed, the trigger may be retried
//! * `<!-- pr-feedback:done <id> -->` — final reply (success, decline, give-up)
//! * `<!-- pr-feedback:cap -->` — the round-cap notice was posted

use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;
use team_core::gh::Comment;

/// Every comment of this role starts with this line; it is never a trigger.
pub const REPLY_MARKER: &str = "<!-- pr-feedback-agent -->";
/// Attempts per trigger comment before giving up.
pub const MAX_ATTEMPTS_PER_COMMENT: usize = 2;
/// A claim without result older than this belongs to a crashed or stuck machine.
pub fn claim_ttl() -> Duration {
    Duration::hours(3)
}

/// What the markers in a PR's comments say.
#[derive(Debug, Default)]
pub struct Thread {
    pub done: BTreeSet<u64>,
    claims: HashMap<u64, Vec<DateTime<Utc>>>,
    failed: HashMap<u64, usize>,
    pub cap_notified: bool,
}

fn marker_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<!-- pr-feedback:(claim|failed|done) (\d+)").expect("regex"))
}

/// `created_at` of a comment; unreadable → long ago (an unreadable claim never blocks).
pub fn created(c: &Comment) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&c.created_at).map(|t| t.with_timezone(&Utc)).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

impl Thread {
    pub fn parse(comments: &[Comment]) -> Self {
        let mut t = Self::default();
        for c in comments.iter().filter(|c| c.body.contains(REPLY_MARKER)) {
            if c.body.contains("<!-- pr-feedback:cap -->") {
                t.cap_notified = true;
            }
            for m in marker_re().captures_iter(&c.body) {
                let Ok(id) = m[2].parse::<u64>() else { continue };
                match &m[1] {
                    "claim" => t.claims.entry(id).or_default().push(created(c)),
                    "failed" => *t.failed.entry(id).or_default() += 1,
                    _ => {
                        t.done.insert(id);
                    }
                }
            }
        }
        t
    }

    /// Feedback rounds used on this PR (every finished trigger counts).
    pub fn rounds(&self) -> usize {
        self.done.len()
    }

    /// Claims made for a trigger so far (abandoned ones included).
    pub fn attempts(&self, id: u64) -> usize {
        self.claims.get(&id).map_or(0, Vec::len)
    }

    /// Someone is working on `id` right now: an unanswered claim younger than the TTL.
    pub fn live_claim(&self, id: u64, now: DateTime<Utc>) -> bool {
        let Some(claims) = self.claims.get(&id) else { return false };
        let failed = self.failed.get(&id).copied().unwrap_or(0);
        claims.len() > failed && claims.iter().max().is_some_and(|t| now - *t < claim_ttl())
    }
}

/// A human comment asking the agent for something: contains the marker (plain substring),
/// is not one of the agent's own replies.
pub fn is_trigger(body: &str, marker: &str) -> bool {
    !body.is_empty() && !marker.is_empty() && body.contains(marker) && !body.contains(REPLY_MARKER)
}

/// Trigger comments without a final reply, oldest first.
pub fn pending<'a>(comments: &'a [Comment], thread: &Thread, marker: &str) -> Vec<&'a Comment> {
    let mut out: Vec<&Comment> = comments.iter().filter(|c| is_trigger(&c.body, marker) && !thread.done.contains(&c.id)).collect();
    out.sort_by_key(|c| (created(c), c.id));
    out
}

fn report_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s)===\s*FEEDBACK REPORT\s*===\s*(.*?)\s*===\s*END\s*===").expect("regex"))
}

/// The worker's FEEDBACK REPORT block, else the end of its final message (≤800 chars).
pub fn feedback_report(output: &str) -> String {
    if output.trim().is_empty() {
        return "(worker produced no output)".into();
    }
    if let Some(block) = report_re().captures_iter(output).map(|c| c[1].trim().to_string()).find(|b| !b.is_empty()) {
        return block;
    }
    crate::shell::tail(output.trim(), 800)
}

/// At most 300 characters of an error for a GitHub comment.
pub fn short_error(e: &str) -> String {
    let e = e.trim();
    if e.chars().count() <= 300 {
        return e.to_string();
    }
    format!("{}…", e.chars().take(300).collect::<String>())
}

fn with_marker(text: &str, marker: &str) -> String {
    format!("{REPLY_MARKER}\n{text}\n{marker}")
}

fn done(id: u64) -> String {
    format!("<!-- pr-feedback:done {id} -->")
}

pub fn claim_comment(id: u64, machine: &str) -> String {
    with_marker(&format!("[pr-feedback] On it — working on comment [above](#issuecomment-{id}) (machine `{machine}`)."), &format!("<!-- pr-feedback:claim {id} machine={machine} -->"))
}

pub fn failed_comment(id: u64, attempt: usize, error: &str) -> String {
    with_marker(&format!("[pr-feedback] Attempt {attempt} on comment [above](#issuecomment-{id}) failed (`{}`) — I'll retry.", short_error(error)), &format!("<!-- pr-feedback:failed {id} -->"))
}

pub fn fork_comment(id: u64, head_full_name: &str) -> String {
    with_marker(&format!("[pr-feedback] This PR's branch lives in a fork (`{head_full_name}`), which I can't push to — please apply the request manually or move the branch into this repo."), &done(id))
}

pub fn disabled_comment(id: u64, parse_error: Option<&str>) -> String {
    let mut text = "[pr-feedback] The pr-feedback role is not enabled for this repository (`agents_enabled` in `.agent.toml` on the default branch).".to_string();
    if let Some(e) = parse_error {
        text.push_str(&format!(" The file could not be read: `{}`.", short_error(e)));
    }
    with_marker(&text, &done(id))
}

pub fn give_up_comment(id: u64, attempts: usize, error: &str) -> String {
    with_marker(&format!("[pr-feedback] I couldn't complete this request after {attempts} attempts (last error: `{}`). Leaving it for a human.", short_error(error)), &done(id))
}

pub fn cap_comment(max_rounds: u32) -> String {
    with_marker(&format!("[pr-feedback] Reached the maximum of {max_rounds} feedback rounds on this PR. Further marker comments will be ignored — please handle the rest manually or merge and open a follow-up issue."), "<!-- pr-feedback:cap -->")
}

/// The success reply (spec §2.7 R).
pub fn success_comment(id: u64, report: &str, pushed: bool, walkthrough: &str) -> String {
    let mut parts = vec![
        REPLY_MARKER.to_string(),
        format!("[pr-feedback] Done — addressed the feedback from comment [above](#issuecomment-{id})."),
        String::new(),
        "### What changed".into(),
        report.to_string(),
    ];
    if !pushed {
        parts.push("\n_Note: no new commits were pushed — the request may have required no code change (or the change failed to apply)._".into());
    }
    parts.push(if walkthrough.trim().is_empty() { "\n_No UI screenshots were produced for this round._".into() } else { walkthrough.to_string() });
    parts.push(done(id));
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use team_core::gh::User;

    const REPORT: &str = "Done.
=== FEEDBACK REPORT ===
- renamed the button
=== END ===";

    fn comment(id: u64, body: &str) -> Comment {
        Comment { id, body: body.into(), user: Some(User { login: "x".into() }), created_at: "2026-10-14T10:00:00Z".into() }
    }
    
    #[test]
    fn trigger_rules_and_order() {
        assert!(is_trigger("@agent do", "@agent"));
        assert!(is_trigger("mail x@agent.com", "@agent"));
        assert!(!is_trigger(&format!("{REPLY_MARKER}\n@agent"), "@agent"));
        assert!(!is_trigger("", "@agent"));
        let list = vec![comment(3, "@agent t3"), comment(1, "@agent t1"), comment(2, "plain"), comment(4, &format!("{REPLY_MARKER} @agent")), comment(5, "@agent t5"), comment(9, &format!("{REPLY_MARKER}\nx\n<!-- pr-feedback:done 1 -->"))];
        let th = Thread::parse(&list);
        assert_eq!(pending(&list, &th, "@agent").iter().map(|c| c.id).collect::<Vec<_>>(), vec![3, 5]);
        assert_eq!(th.rounds(), 1);
    }
    
    #[test]
    fn claims_expire_and_failures_release_them() {
        let now = Utc::now();
        let at = |h: i64| (now - chrono::Duration::hours(h)).to_rfc3339();
        let mut c = comment(10, &claim_comment(3, "A"));
        c.created_at = at(1);
        let th = Thread::parse(std::slice::from_ref(&c));
        assert!(th.live_claim(3, now) && th.attempts(3) == 1);
        let mut old = c.clone();
        old.created_at = at(4);
        assert!(!Thread::parse(&[old]).live_claim(3, now), "abandoned after 3h");
        let failed = comment(11, &failed_comment(3, 1, "boom"));
        assert!(!Thread::parse(&[c, failed]).live_claim(3, now));
    }
    
    #[test]
    fn report_extraction() {
        assert_eq!(feedback_report(""), "(worker produced no output)");
        assert_eq!(feedback_report(REPORT), "- renamed the button");
        let long = "z".repeat(2000);
        let r = feedback_report(&long);
        assert!(r.starts_with("...\n") && r.chars().count() <= 810);
        assert_eq!(short_error(&"e".repeat(400)).chars().count(), 301);
    }
}
