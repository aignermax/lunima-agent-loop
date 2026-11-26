//! Signs that a human took an issue over or wants the agent to stop — checked before and
//! after every coder session and, from a background thread, every few minutes while one
//! runs, so a stopped or duplicated issue costs minutes instead of a whole session.

use crate::forge::Forge;
use crate::llm::StopCheck;
use chrono::{DateTime, Utc};
use regex::Regex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How often a running session asks GitHub (three API calls per check).
pub const CHECK_INTERVAL: Duration = if cfg!(test) { Duration::from_millis(50) } else { Duration::from_secs(180) };

/// The watcher thread sleeps in slices this long, so it ends soon after its session.
const SLICE: Duration = Duration::from_millis(200);

/// GitHub stamps comments to the second; a little slack keeps one written in the very
/// second of the claim.
const SINCE_SLACK_SECS: i64 = 2;

/// Who may steer the agent through comments and PRs (`author_association`): people with
/// access to the repository, not anyone who can comment on a public one.
const STEERING: [&str; 3] = ["OWNER", "MEMBER", "COLLABORATOR"];

pub struct Watch<'a> {
    forge: &'a dyn Forge,
    issue: u64,
    running_label: String,
    login: String,
    own_branch: String,
    since: DateTime<Utc>,
    assignees_at_claim: Vec<String>,
}

impl<'a> Watch<'a> {
    /// Watches `issue` as claimed at `claimed_at` (GitHub's time of the claim comment, so a
    /// stop posted while the agent was paused still counts) with the assignees it had then.
    pub fn new(forge: &'a dyn Forge, issue: u64, running_label: &str, login: &str, own_branch: &str, claimed_at: Option<&str>, assignees_at_claim: &[String]) -> Self {
        let claimed = claimed_at.and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
        Self {
            forge,
            issue,
            running_label: running_label.into(),
            login: login.into(),
            own_branch: own_branch.into(),
            since: claimed - chrono::Duration::seconds(SINCE_SLACK_SECS),
            assignees_at_claim: assignees_at_claim.iter().map(|l| l.to_lowercase()).collect(),
        }
    }

    /// Checks GitHub now. A failed call counts as "keep going": a network hiccup must not
    /// end a session.
    pub fn check_now(&self) -> Option<String> {
        let n = self.issue;
        let issue = self.forge.issue(n).ok()?;
        if issue.state != "open" {
            return Some(CLOSED.into());
        }
        if !issue.has_label(&self.running_label) {
            return Some(format!("the `{}` label was removed", self.running_label));
        }
        let own = self.login.to_lowercase();
        let assigned = issue.assignees.iter().map(|u| u.login.to_lowercase()).find(|l| *l != own && !self.assignees_at_claim.contains(l));
        if let Some(who) = assigned {
            return Some(format!("the issue was assigned to `{who}`"));
        }
        if let Ok(comments) = self.forge.comments(n) {
            let asked = comments.iter().find(|c| may_steer(&c.author_association) && is_stop_command(&c.body) && DateTime::parse_from_rfc3339(&c.created_at).is_ok_and(|t| t >= self.since));
            if let Some(c) = asked {
                let who = c.user.as_ref().map(|u| u.login.as_str()).unwrap_or("someone");
                return Some(format!("`{who}` asked the agent to stop"));
            }
        }
        if let Ok(prs) = self.forge.open_prs() {
            let other = prs.iter().find(|p| p.head.name != self.own_branch && may_steer(&p.author_association) && fixes_issue(&format!("{}\n{}", p.title, p.body.as_deref().unwrap_or("")), n));
            if let Some(p) = other {
                return Some(format!("PR #{} already fixes this issue", p.number));
            }
        }
        None
    }

    /// Runs `session` with a stop check that a background thread keeps current every
    /// `CHECK_INTERVAL`: the session loop asks it every second, and the GitHub calls behind
    /// it (up to a minute each when GitHub hangs) must not stall its memory guard.
    pub fn beside<T>(&self, session: impl FnOnce(StopCheck<'_>) -> T) -> T {
        let done = AtomicBool::new(false);
        let flagged: Mutex<Option<String>> = Mutex::new(None);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !sleep_unless(&done, CHECK_INTERVAL) {
                    if let Some(reason) = self.check_now() {
                        *flagged.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason);
                        return;
                    }
                }
            });
            let stop = || flagged.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let out = session(&stop);
            done.store(true, Ordering::Relaxed);
            out
        })
    }
}

/// The reason `check_now` gives for a closed issue (the coder steps aside silently then).
pub const CLOSED: &str = "the issue was closed";

/// Sleeps up to `d` in short slices; `true` once `done` is set.
fn sleep_unless(done: &AtomicBool, d: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < d {
        if done.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(SLICE.min(d.saturating_sub(start.elapsed())));
    }
    done.load(Ordering::Relaxed)
}

fn may_steer(association: &str) -> bool {
    STEERING.iter().any(|s| association.eq_ignore_ascii_case(s))
}

/// A line `/agent stop` or `agent stop` (also `agent-stop`, German `stopp`). Only this
/// command counts: free text such as "Stop all" is what TestConsole issues are about.
pub fn is_stop_command(body: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?im)^[ \t]*/?agent[ \t-]+stopp?\b").expect("static regex")).is_match(body)
}

/// GitHub's closing keywords (`Fixes #12`, `closes: #12` …); a bare mention does not count —
/// batch PRs list related issues they do not fix.
pub fn fixes_issue(text: &str, n: u64) -> bool {
    Regex::new(&format!(r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s*:?\s+#{n}\b"))
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{self, FakeForge};

    fn running(n: u64) -> FakeForge {
        FakeForge::new("o/r").with_issue(fake::issue(n, "Quick connect", "x", &["agent-running"]))
    }

    fn watch(f: &FakeForge, n: u64) -> Watch<'_> {
        Watch::new(f, n, "agent-running", "bot", "agent/issue-7-1", None, &[])
    }

    #[test]
    fn the_stop_command_is_explicit() {
        for yes in ["/agent stop", "agent stop", "Agent-Stop please", "thanks\n/agent stopp\nNik takes it"] {
            assert!(is_stop_command(yes), "{yes}");
        }
        for no in ["Stop all leaves PS_Logic on", "please stop", "the agent stops at 20 M tokens", "agent stopper", "Agent\nStop all is broken"] {
            assert!(!is_stop_command(no), "{no}");
        }
    }

    #[test]
    fn only_closing_keywords_count_as_fixing() {
        assert!(fixes_issue("Lab fixes batch 2\nFixes #9\nFixes #12", 9));
        assert!(fixes_issue("closes: #40", 40) && fixes_issue("Resolved #40", 40));
        assert!(!fixes_issue("Fixes #90", 9), "another issue");
        assert!(!fixes_issue("Life Support lab fixes (see #8, #10)", 8), "a bare mention");
    }

    #[test]
    fn a_quiet_issue_keeps_the_session_going() {
        let f = running(7);
        f.add_comment(7, "Measured 2.7 min after the change");
        assert_eq!(watch(&f, 7).check_now(), None);
    }

    #[test]
    fn closing_unlabelling_assigning_or_a_stop_command_end_it() {
        let f = running(7);
        let w = watch(&f, 7);
        f.add_comment(7, "/agent stop — Nik takes this one");
        assert_eq!(w.check_now().as_deref(), Some("`someone` asked the agent to stop"));

        let f = running(7);
        let w = watch(&f, 7);
        f.s.lock().unwrap().issues.get_mut(&7).unwrap().assignees.push(team_core::gh::User { login: "MaddNik".into() });
        assert_eq!(w.check_now().as_deref(), Some("the issue was assigned to `maddnik`"));

        let f = running(7);
        let w = watch(&f, 7);
        f.remove_label(7, "agent-running").unwrap();
        assert_eq!(w.check_now().as_deref(), Some("the `agent-running` label was removed"));

        let f = running(7);
        let w = watch(&f, 7);
        f.s.lock().unwrap().issues.get_mut(&7).unwrap().state = "closed".into();
        assert_eq!(w.check_now().as_deref(), Some(CLOSED));
    }

    #[test]
    fn a_member_pr_that_fixes_the_issue_ends_it_but_the_agents_own_and_outsiders_do_not() {
        let mut own = fake::pr(50, "Agent: Quick connect", "agent/issue-7-1", &[]);
        own.body = Some("Fixes #7".into());
        let f = running(7).with_pr(own);
        assert_eq!(watch(&f, 7).check_now(), None);
        let mut fork = fake::pr(52, "drive-by", "patch-1", &[]);
        fork.body = Some("Fixes #7".into());
        fork.author_association = "CONTRIBUTOR".into();
        let f = running(7).with_pr(fork);
        assert_eq!(watch(&f, 7).check_now(), None, "no write access, no say");
        let mut theirs = fake::pr(51, "Lab fixes batch 5", "fix/lab-batch-5", &[]);
        theirs.body = Some("Fixes #7".into());
        let f = running(7).with_pr(theirs);
        assert_eq!(watch(&f, 7).check_now().as_deref(), Some("PR #51 already fixes this issue"));
    }

    #[test]
    fn outsiders_comments_old_comments_and_assignees_from_before_the_claim_are_not_news() {
        let f = running(7);
        let id = f.add_comment(7, "agent stop");
        f.set_comment_time(7, id, "2026-11-01T08:00:00Z");
        f.add_comment(7, "/agent stop");
        f.s.lock().unwrap().comments.get_mut(&7).unwrap().last_mut().unwrap().author_association = "NONE".into();
        f.s.lock().unwrap().issues.get_mut(&7).unwrap().assignees.push(team_core::gh::User { login: "GiulioTerrasanta".into() });
        let w = Watch::new(&f, 7, "agent-running", "bot", "agent/issue-7-1", Some("2026-11-25T10:00:00Z"), &["giulioterrasanta".into()]);
        assert_eq!(w.check_now(), None);
    }

    #[test]
    fn a_stop_while_the_agent_was_paused_counts_from_the_claim() {
        let f = running(7);
        let id = f.add_comment(7, "/agent stop, Nik has it");
        f.set_comment_time(7, id, "2026-11-25T12:00:00Z");
        let w = Watch::new(&f, 7, "agent-running", "bot", "agent/issue-7-1", Some("2026-11-25T10:00:00Z"), &[]);
        assert!(w.check_now().is_some(), "posted after the claim, before this run resumed");
    }

    #[test]
    fn the_background_check_reaches_a_running_session_and_ends_with_it() {
        let f = running(7);
        let w = watch(&f, 7);
        let reason = w.beside(|stop| {
            f.add_comment(7, "/agent stop");
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if let Some(r) = stop() {
                    return Some(r);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        });
        assert_eq!(reason.as_deref(), Some("`someone` asked the agent to stop"));
        let quiet = running(8);
        let started = Instant::now();
        assert_eq!(watch(&quiet, 8).beside(|stop| stop()), None);
        assert!(started.elapsed() < Duration::from_secs(2), "the watcher ends with its session");
    }
}
