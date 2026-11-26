//! Signs that a human took an issue over or wants the agent to stop — checked before every
//! coder session and every few minutes while one runs, so a stopped or duplicated issue
//! costs minutes instead of a whole session.

use crate::forge::Forge;
use chrono::{DateTime, Utc};
use regex::Regex;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use team_core::gh::Issue;

/// How often a running session asks GitHub (three API calls per check).
pub const CHECK_INTERVAL: Duration = Duration::from_secs(180);

/// GitHub stamps comments to the second; a little slack keeps one written in the very
/// second the run started.
const SINCE_SLACK_SECS: i64 = 2;

pub struct Watch<'a> {
    forge: &'a dyn Forge,
    issue: u64,
    running_label: String,
    login: String,
    own_branch: String,
    since: DateTime<Utc>,
    assignees_at_start: Vec<String>,
    last: Mutex<Option<Instant>>,
}

impl<'a> Watch<'a> {
    /// Starts watching `issue` as claimed now; assignees it already has are not news.
    pub fn new(forge: &'a dyn Forge, issue: &Issue, running_label: &str, login: &str, own_branch: &str) -> Self {
        Self {
            forge,
            issue: issue.number,
            running_label: running_label.into(),
            login: login.into(),
            own_branch: own_branch.into(),
            since: Utc::now() - chrono::Duration::seconds(SINCE_SLACK_SECS),
            assignees_at_start: issue.assignees.iter().map(|u| u.login.to_lowercase()).collect(),
            last: Mutex::new(None),
        }
    }

    /// Checks GitHub now. A failed call counts as "keep going": a network hiccup must not
    /// end a session.
    pub fn check_now(&self) -> Option<String> {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        self.reason()
    }

    /// `check_now`, at most every `CHECK_INTERVAL` (the session loop asks every second).
    pub fn throttled(&self) -> Option<String> {
        let due = self.last.lock().unwrap_or_else(|p| p.into_inner()).is_none_or(|t| t.elapsed() >= CHECK_INTERVAL);
        if due {
            self.check_now()
        } else {
            None
        }
    }

    fn reason(&self) -> Option<String> {
        let n = self.issue;
        let issue = self.forge.issue(n).ok()?;
        if issue.state != "open" {
            return Some("the issue was closed".into());
        }
        if !issue.has_label(&self.running_label) {
            return Some(format!("the `{}` label was removed", self.running_label));
        }
        let assigned = issue.assignees.iter().map(|u| u.login.to_lowercase()).find(|l| *l != self.login.to_lowercase() && !self.assignees_at_start.contains(l));
        if let Some(who) = assigned {
            return Some(format!("the issue was assigned to `{who}`"));
        }
        if let Ok(comments) = self.forge.comments(n) {
            let asked = comments.iter().find(|c| is_stop_command(&c.body) && DateTime::parse_from_rfc3339(&c.created_at).is_ok_and(|t| t >= self.since));
            if let Some(c) = asked {
                let who = c.user.as_ref().map(|u| u.login.as_str()).unwrap_or("someone");
                return Some(format!("`{who}` asked the agent to stop"));
            }
        }
        if let Ok(prs) = self.forge.open_prs() {
            let other = prs.iter().find(|p| p.head.name != self.own_branch && fixes_issue(&format!("{}\n{}", p.title, p.body.as_deref().unwrap_or("")), n));
            if let Some(p) = other {
                return Some(format!("PR #{} already fixes this issue", p.number));
            }
        }
        None
    }
}

/// A line `/agent stop` or `agent stop` (also `agent-stop`, German `stopp`). Only this
/// command counts: free text such as "Stop all" is what TestConsole issues are about.
pub fn is_stop_command(body: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?im)^\s*/?agent[\s-]+stopp?\b").expect("static regex")).is_match(body)
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
        let issue = f.issue(n).unwrap();
        Watch::new(f, &issue, "agent-running", "bot", "agent/issue-7-1")
    }

    #[test]
    fn the_stop_command_is_explicit() {
        for yes in ["/agent stop", "agent stop", "Agent-Stop please", "thanks\n/agent stopp\nNik takes it"] {
            assert!(is_stop_command(yes), "{yes}");
        }
        for no in ["Stop all leaves PS_Logic on", "please stop", "the agent stops at 20 M tokens", "agent stopper"] {
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
        assert_eq!(w.check_now().as_deref(), Some("the issue was closed"));
    }

    #[test]
    fn a_human_pr_that_fixes_the_issue_ends_it_but_the_agents_own_does_not() {
        let mut own = fake::pr(50, "Agent: Quick connect", "agent/issue-7-1", &[]);
        own.body = Some("Fixes #7".into());
        let f = running(7).with_pr(own);
        assert_eq!(watch(&f, 7).check_now(), None);
        let mut theirs = fake::pr(51, "Lab fixes batch 5", "fix/lab-batch-5", &[]);
        theirs.body = Some("Fixes #7".into());
        let f = running(7).with_pr(theirs);
        assert_eq!(watch(&f, 7).check_now().as_deref(), Some("PR #51 already fixes this issue"));
    }

    #[test]
    fn old_stop_comments_and_existing_assignees_are_not_news() {
        let f = running(7);
        let id = f.add_comment(7, "agent stop");
        f.set_comment_time(7, id, "2026-11-01T08:00:00Z");
        f.s.lock().unwrap().issues.get_mut(&7).unwrap().assignees.push(team_core::gh::User { login: "GiulioTerrasanta".into() });
        assert_eq!(watch(&f, 7).check_now(), None);
    }

    #[test]
    fn throttled_checks_at_once_then_waits() {
        let f = running(7);
        let w = watch(&f, 7);
        assert_eq!(w.throttled(), None);
        f.add_comment(7, "/agent stop");
        assert_eq!(w.throttled(), None, "the next GitHub check is minutes away");
        assert!(w.check_now().is_some());
    }
}
