//! One claimed issue: worktree, Claude sessions (continuations after max turns), then
//! `finish`. Failures are counted on GitHub (marker comments), so a retry on the other
//! machine knows how many attempts were already made.

use super::finish;
use super::pickup::{self, Release};
use super::prompt;
use crate::ctx::{Ctx, RoleError, RoleResult};
use crate::directives;
use crate::llm::{Job, LlmError};
use crate::pr_text::thousands;
use crate::session::{SessionState, Store};
use crate::workspace::RepoSetup;
use std::path::PathBuf;
use team_core::detail;
use team_core::gh::Issue;
use team_core::git::commit_and_push;

const FAILED_MARK: &str = "<!-- team-agent:attempt-failed -->";
const GAVE_UP_MARK: &str = "<!-- team-agent:gave-up -->";

pub struct IssueRun<'a> {
    pub ctx: &'a Ctx,
    pub setup: &'a RepoSetup,
    pub store: &'a Store,
}

pub fn label(repo: &str, kind: &str, n: u64) -> String {
    format!("coder_{}_{kind}-{n}", repo.replace('/', "_"))
}

impl IssueRun<'_> {
    /// Works on a claimed issue until it is done, handed back or the usage limit hits.
    pub fn run(&self, issue: Issue, state: Option<SessionState>) -> RoleResult<()> {
        let (ctx, setup) = (self.ctx, self.setup);
        let n = issue.number;
        let branch = pickup::branch_for(setup.forge.as_ref(), &setup.clone, &issue, state.as_ref());
        let mut state = state.unwrap_or_else(|| SessionState::new(&setup.repo, n, &branch));
        state.branch_name = branch.clone();
        if !state.in_review && matches!(pickup::agent_pr(setup.forge.as_ref(), n), Ok(Some(_))) {
            // the PR exists already (e.g. made by the other machine): continue with the review
            state.in_review = true;
        }
        self.store.save(&state);
        ctx.heartbeat.report("working", detail!["repo" => setup.repo, "issue" => n, "title" => issue.title]);
        crate::log::info(&format!("{}#{n} „{}“ auf {branch}", setup.repo, issue.title));
        let (worktree, base) = match self.prepare(&issue, &branch) {
            Ok(Some(x)) => x,
            Ok(None) => return Ok(()),
            Err(e) => return self.fail(&issue, &mut state, &format!("worktree setup failed: {e}")),
        };
        if !state.in_review {
            match self.sessions(&issue, &mut state, &worktree)? {
                Flow::Continue => {}
                Flow::Stop => return Ok(()),
                Flow::Fail(e) => return self.fail(&issue, &mut state, &e),
                Flow::Human(reason) => {
                    self.hand_over(&issue, &mut state, &reason);
                    return Ok(());
                }
            }
        }
        match finish::finish(ctx, setup, self.store, &issue, &mut state, &worktree, &base) {
            Ok(()) => Ok(()),
            Err(RoleError::UsageLimit(t)) => {
                self.store.save(&state);
                Err(RoleError::UsageLimit(t))
            }
            Err(RoleError::Failed(e)) => self.fail(&issue, &mut state, &e),
        }
    }

    /// Team branch (created if missing) and the worktree. `Ok(None)` = handed to a human.
    fn prepare(&self, issue: &Issue, branch: &str) -> Result<Option<(PathBuf, String)>, String> {
        let setup = self.setup;
        setup.clone.fetch()?;
        let team = directives::team_branch(issue.body.as_deref());
        if let Some(tb) = &team {
            if !self.ensure_team_branch(issue, tb)? {
                return Ok(None);
            }
        }
        let base = team.unwrap_or_else(|| setup.working_branch.clone());
        let wt = setup.clone.worktree(&self.ctx.paths.worktree(&setup.repo, branch), branch, &base)?;
        Ok(Some((wt, base)))
    }

    fn ensure_team_branch(&self, issue: &Issue, tb: &str) -> Result<bool, String> {
        let (setup, forge) = (self.setup, self.setup.forge.as_ref());
        if setup.clone.remote_branch_exists(tb) {
            return Ok(true);
        }
        let working = &setup.working_branch;
        match setup.clone.git_net(&["push", "origin", &format!("origin/{working}:refs/heads/{tb}")]) {
            Ok(_) => {
                setup.clone.fetch()?;
                let _ = forge.comment(issue.number, &format!("🌱 Team branch `{tb}` did not exist yet — created it from `{working}`. If the name is a typo, delete the branch and fix the issue field."));
                Ok(true)
            }
            Err(e) => {
                let l = &self.ctx.cfg.labels;
                let short: String = e.trim().chars().take(200).collect();
                let _ = forge.comment(issue.number, &format!("⚠️ This issue declares team branch `{tb}`, which does not exist, and creating it from `{working}` failed (`{short}`). Please create the branch manually, then remove `{}` and re-add the `{}` label.", l.needs_human, l.task));
                pickup::release(self.ctx, forge, issue.number, Release::Human);
                Ok(false)
            }
        }
    }

    fn sessions(&self, issue: &Issue, state: &mut SessionState, worktree: &std::path::Path) -> RoleResult<Flow> {
        let (ctx, setup) = (self.ctx, self.setup);
        let forge = setup.forge.as_ref();
        let n = issue.number;
        let complex = prompt::is_complex(ctx, issue);
        let b = &ctx.cfg.budgets;
        let (max_turns, max_tokens) = if complex { (b.max_turns_complex, b.max_tokens_complex) } else { (b.max_turns_regular, b.max_tokens_regular) };
        loop {
            let current = match forge.issue(n) {
                Ok(i) => i,
                Err(e) => return Ok(Flow::Fail(e)),
            };
            if current.state != "open" {
                crate::log::info(&format!("#{n} wurde geschlossen — Arbeit beendet"));
                self.store.delete(&setup.repo, n);
                pickup::release(ctx, forge, n, Release::Done);
                return Ok(Flow::Stop);
            }
            if state.session_count >= b.max_sessions {
                return Ok(Flow::Human(format!("{} Claude sessions did not finish this issue", state.session_count)));
            }
            let labels = crate::forge::label_names(&current.labels);
            let choice = match team_core::provider::choose(&ctx.cfg, &setup.repo, &labels) {
                Ok(c) => c,
                Err(e) => return Ok(Flow::Fail(e)),
            };
            let text = prompt::session(ctx, &setup.repo, &current, state)?;
            let job = Job { prompt: text, cwd: worktree.to_path_buf(), model: choice.model, max_turns, env: choice.env, label: label(&setup.repo, "issue", n) };
            crate::log::info(&format!("#{n}: Sitzung {} ({})", state.session_count + 1, choice.why));
            let out = match ctx.llm.run(&job) {
                Ok(o) => o,
                Err(LlmError::UsageLimit(t)) => {
                    self.store.save(state);
                    return Err(RoleError::UsageLimit(t));
                }
                Err(LlmError::Failed(e)) => return Ok(Flow::Fail(e)),
            };
            state.add_session(out.num_turns, out.usage.total(), out.estimated_cost());
            state.last_output = out.text.chars().take(5000).collect();
            if state.total_tokens > max_tokens {
                self.budget_stop(n, state, max_tokens);
                return Ok(Flow::Stop);
            }
            if !out.reached_max_turns {
                self.store.save(state);
                return Ok(Flow::Continue);
            }
            self.continuation(n, state, worktree);
        }
    }

    /// Max turns reached: save progress on origin (survives a crash, visible to the other
    /// machine) and go on with a continuation session.
    fn continuation(&self, n: u64, state: &mut SessionState, worktree: &std::path::Path) {
        let k = state.session_count;
        state.note(&format!("Session {k}: Reached max turns, needs continuation"));
        self.store.save(state);
        if let Err(e) = commit_and_push(worktree, &state.branch_name, &format!("Agent: work in progress on #{n} (session {k})")) {
            crate::log::warn(&format!("#{n}: Zwischenstand nicht gepusht: {e}"));
        }
        let body = format!("[AGENT] **Session {k} completed**\n\n- Turns used: {}\n- Tokens: {}\n- Cost so far: ${:.4}\n\n_Continuing work in next session..._", state.total_turns_used, thousands(state.total_tokens), state.total_cost_usd);
        let _ = self.setup.forge.comment(n, &body);
    }

    fn budget_stop(&self, n: u64, state: &mut SessionState, limit: u64) {
        let body = format!("⚠️ **Token budget exceeded**\n\nThis issue has consumed {} tokens (limit: {}).\n\nCost so far: ${:.2}\n\nThe agent is stopping to prevent excessive costs. Please review the issue complexity or increase the budget limit if needed.", thousands(state.total_tokens), thousands(limit), state.total_cost_usd);
        let _ = self.setup.forge.comment(n, &body);
        state.note(&format!("Stopped: Token budget exceeded ({} tokens)", thousands(state.total_tokens)));
        state.handed_to_human = true;
        self.store.save(state);
        pickup::release(self.ctx, self.setup.forge.as_ref(), n, Release::Human);
    }

    /// Not an error but nothing more the agent can do: a human takes over.
    fn hand_over(&self, issue: &Issue, state: &mut SessionState, reason: &str) {
        let n = issue.number;
        let _ = self.setup.forge.comment(n, &format!("{GAVE_UP_MARK}\n🙋 Handing this issue to a human: {reason}. Work so far is on branch `{}`. Re-add the `{}` label (and remove `{}`) to let the agent continue with a fresh budget.", state.branch_name, self.ctx.cfg.labels.task, self.ctx.cfg.labels.needs_human));
        state.note(&format!("Handed to a human: {reason}"));
        state.handed_to_human = true;
        self.store.save(state);
        self.release_pr(state);
        pickup::release(self.ctx, self.setup.forge.as_ref(), n, Release::Human);
    }

    /// Drops `agent-running` from the issue's PR (review / fix ended).
    fn release_pr(&self, state: &SessionState) {
        if let Ok(Some(pr)) = self.setup.forge.pr_by_branch(&state.branch_name) {
            let _ = self.setup.forge.remove_label(pr.number, &self.ctx.cfg.labels.running);
            crate::qa::claims::remove_from(&self.ctx.paths.state(), crate::qa::claims::CODER_FILE, &self.setup.repo, pr.number);
        }
    }

    /// An attempt failed: re-queue (any machine may retry) or give up after `max_attempts`.
    fn fail(&self, issue: &Issue, state: &mut SessionState, err: &str) -> RoleResult<()> {
        let (ctx, forge) = (self.ctx, self.setup.forge.as_ref());
        let n = issue.number;
        crate::log::error(&format!("{}#{n}: {err}", self.setup.repo));
        if state.in_review {
            // the PR exists: re-coding from scratch elsewhere would duplicate it
            self.hand_over(issue, state, &format!("the review step failed after the PR was opened (`{}`)", err.chars().take(300).collect::<String>()));
            return Ok(());
        }
        state.attempts += 1;
        state.note(&format!("Error: {}", err.chars().take(200).collect::<String>()));
        self.store.save(state);
        let attempts = (attempts_on_github(forge, n) + 1).max(state.attempts);
        let short: String = err.chars().take(300).collect();
        if attempts < ctx.cfg.budgets.max_attempts {
            let _ = forge.comment(n, &format!("{FAILED_MARK}\n⚠️ Attempt {attempts} on `{}` failed: `{short}` — the issue goes back into the queue.", ctx.machine));
            pickup::release(ctx, forge, n, Release::Requeue);
            return Ok(());
        }
        let _ = forge.comment(n, &format!("{FAILED_MARK}\n{GAVE_UP_MARK}\n❌ Agent `{}` failed to complete this issue after {attempts} attempts.\n\nLast error: `{short}`\n\nPlease investigate manually.", ctx.machine));
        finish::history(ctx, issue, state, false, None);
        pickup::release(ctx, forge, n, Release::Human);
        state.handed_to_human = true;
        self.store.save(state);
        Ok(())
    }
}

/// Failed attempts since the last give-up (a human re-queue starts a fresh count).
fn attempts_on_github(forge: &dyn crate::forge::Forge, n: u64) -> u32 {
    let comments = forge.comments(n).unwrap_or_default();
    let start = comments.iter().rposition(|c| c.body.contains(GAVE_UP_MARK)).map(|i| i + 1).unwrap_or(0);
    comments[start..].iter().filter(|c| c.body.contains(FAILED_MARK)).count() as u32
}

pub enum Flow {
    Continue,
    Stop,
    Fail(String),
    /// stop and hand to a human (not counted as a failed attempt)
    Human(String),
}
