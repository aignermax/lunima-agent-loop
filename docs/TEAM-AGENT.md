# Lunima Team Agent — architecture (Rust)

The whole agent team — Product Owner, Coder (+ test gate, reviewer, UX walkthrough), QA,
PR feedback — runs as **one native Rust program on Windows and Linux**. It replaces the
Python *autonomous-issue-agent* (which ran in WSL only because it drove the Claude CLI
through a Unix pseudo-terminal) and, last, the C# PO loop. The Lunima Team Control Center
is its user interface.

## Decisions (2026-10-14)

| Topic | Decision |
|---|---|
| Language / platforms | Rust, native Windows + Linux (no WSL), x64 + arm64 |
| Claude | Claude Code CLI, **subscription login** of the user (no API key). Kimi via OpenRouter keeps its own key for repos routed there. Usage-limit errors pause the role until the reset time. |
| GitHub | `gh` CLI with the user's login (`gh api …`); no token in any config file |
| Configuration | one `team.toml` in the data folder, edited by the Control Center; existing `.env` + `agent-loop.json` are imported on first start |
| Two machines | every machine works on all repos; a claim on GitHub (assignee + `agent-running` + machine name in the claim comment) prevents double work; stale claims are released |
| PO | becomes a role of the same program, ported last; the C# loop runs until then |
| Migration | role by role: a role runs in Rust *or* Python, never both |

## Layout

```
lunima-agent-loop/
  Cargo.toml                 workspace
  crates/team-core/          config, gh, git/worktrees, claude runner, provider policy,
                             prompts, status/control files, history, discovery
  crates/team-agent/         binary `lunima-team-agent`: one thread per role
  control-center/            binary `lunima-po` (UI, tray) — reads the same files
```

## Data folder

`%LOCALAPPDATA%\lunima-team` (Windows) · `~/.local/share/lunima-team` (Linux):

```
team.toml            configuration (no secrets)
state/control.json   per-role pause            (same contract as the Python agent)
state/status-*.json  per-role heartbeat
state/issue-history.jsonl  run history (issue, PR, cost, duration, machine)
state/sessions/<o_r>/      resumable coder sessions (per repo — no cross-repo collisions)
state/discovered-repos.json, work-hints.json   org discovery (coder writes, all read)
state/qa-claims.json       PRs this machine marked qa-running (cleared on restart)
state/agent.lock           single instance per machine (OS file lock)
prompts/<name>.md          prompt overrides (built-ins are compiled in)
logs/agent.log             agent log (size-rotated); logs/<role>/*.jsonl per Claude session
work/<role>/<o_r>/         one clone per role and repo
work/wt/<repo>_<branch>_<hash>/  worktree per branch (short: Windows path limits)
```

## Processes

- `lunima-team-agent run` — the daemon; Windows: started at logon (scheduled task, runs on
  the lock screen), Linux: systemd `--user` unit. Runs as the user, so the user's Claude
  and gh logins apply.
- `lunima-team-agent once <coder|qa|pr-feedback>` — one cycle, for testing.
- `lunima-team-agent status` — health summary on the console.

## Phases

1. Foundation: workspace, `team-core` (config + import, gh, git, claude runner incl.
   usage-limit detection, prompts, state files), CI on Windows + Linux.
2. Roles: coder (claim, worktree, session, test gate, reviewer loop, PR, walkthrough,
   QA-fix), QA, PR feedback, discovery.
3. Cut-over per role from Python; remove WSL units, Python agent, old dashboard.
4. PO role (port of the C# loop); remove the C# loop.
5. Installers: Windows MSI (Control Center + agent + autostart), Linux `.deb` + tar.gz
   (systemd user unit, desktop entry, tray); setup assistant in the Control Center.

## Several machines, one GitHub account

Every role may run on every machine. A claim runs inside a GitHub-side mutex: creating the
hidden ref `refs/team-locks/<key>` succeeds for exactly one machine (422 for the rest). Inside
it the item is re-read and marked — issue: `agent-running` + assignee + claim comment with a
`<!-- team-agent:claim machine=X -->` marker; QA: `qa-running`; QA fix: `agent-running` on the
PR; PR feedback: an ack comment with `<!-- pr-feedback:claim <id> machine=X -->`. Locks older
than 10 minutes belong to a crashed machine and are broken.

State the other machine needs lives on GitHub, not in local files: failed attempts
(`<!-- team-agent:attempt-failed -->`), PR-feedback rounds/claims/done markers, the work
branch (`agent/issue-N-*` on origin is reused; continuation sessions push their progress).

## Deliberate differences to the Python agent

- Claude runs on the subscription (no API key; `ANTHROPIC_*` stripped) unless the provider
  policy routes a repo to a third-party endpoint; reviewers always use the subscription.
- Continuation sessions keep the previous session's uncommitted work (Python reset it) and
  push it as a work-in-progress commit.
- Failed attempts are counted per issue on GitHub (Python counted only successful sessions,
  so a crashing Claude looped forever); after `max_attempts` the issue goes to `needs-human`.
- After a restart a machine resumes its own claimed issues (Python left them stuck).
- The usage limit pauses all roles until the reset time instead of failing the issue.
- Only `OK/PASS/APPROVE(D)` verdicts pass a review (Python passed everything but `BLOCKING`).
- Reviewer and QA review diff against the PR's real base (Python: the default branch).
- No PR-URL short-circuit from the final message; `#12` no longer matches `#123`.
- QA: a repo with QA disabled no longer starves newer PRs; a broken `.agent.toml` is a FAILED
  verdict naming the error; failed checkouts release `qa-running`; startup only clears this
  machine's `qa-running` labels.
- QA fix: `max_qa_fix_rounds` fix attempts (Python allowed one fewer); a crashing fix is
  retried once, then escalated (Python retried forever). Usage of retries counts to the budget.
- PR feedback: new commits drop the stale `qa-passed`/`qa-failed` so QA checks the new head.
- Session files per repo; worktrees with short paths; no tokens in remote URLs (git uses
  `gh auth git-credential`).
