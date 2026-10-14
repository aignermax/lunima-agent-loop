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
state/history.jsonl  run history (issue, PR, cost, duration)
state/sessions/      resumable coder sessions
prompts/<name>.md    prompt overrides (built-ins are compiled in)
logs/                agent log (size-rotated) + per-session stream-json logs
work/<owner>_<repo>/ one clone per repo + worktrees per issue
```

## Processes

- `lunima-team-agent run` — the daemon; Windows: started at logon (scheduled task, runs on
  the lock screen), Linux: systemd `--user` unit. Runs as the user, so the user's Claude
  and gh logins apply.
- `lunima-team-agent once --role coder [--issue N]` — one cycle, for testing.
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
