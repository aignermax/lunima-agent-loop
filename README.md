# lunima-agent-loop

Autonomous issue → PR loop for the [Lunima](https://github.com/aignermax/Lunima) photonics IDE.
A small .NET console app that runs on a schedule (e.g. while the maintainer is on vacation),
picks up GitHub issues labelled `agent-task`, implements them headlessly with the
[Kimi Code CLI](https://www.kimi.com/code/docs/en/), and opens PRs into a dedicated integration
branch (`dev-ki`) — never into `main`. A Product-Owner pass reviews and merges those PRs
and keeps the backlog aligned with `docs/ROADMAP.md`.

## How it works

```
Windows Task Scheduler (hourly, windowless, runs on lock screen)
        │
        ▼
lunima-agent-loop run                      ← stateless; state lives in GitHub + one JSON file
        │
        ├─ Product-Owner pass (kimi, owner model) — due every `ownerIntervalMinutes`,
        │  but skipped entirely when there is nothing to do (no open agent PRs and a
        │  healthy backlog) → idle hours cost no API calls
        │     review open agent-pr PRs → squash-merge into dev-ki when green
        │     groom + split + seed agent-task issues from docs/ROADMAP.md
        │     status report on the "Agent loop — status" tracking issue (only on changes)
        │
        └─ up to N/day: worker pass (kimi, worker model, one run per issue)
              branch agent/issue-<n>-<ts> from dev-ki
              .agent-loop/task-<n>.md = the work contract (prompts/worker.md)
              kimi -p "...execute task file..." --output-format stream-json
              agent implements, runs the test suite, pushes, opens PR → dev-ki
```

Guardrails: daily task cap, never push to `main`, no force-push/`--admin` by the worker,
full test suite must be green before a PR, confidential data (customer PDKs) must never be
committed. The real budget cap lives on the Kimi account (monthly quota); the daily caps here
only pace the spend. Every run is logged to `logs/` and `state/state.json`.

## Setup (Windows)

```powershell
dotnet build                                   # or let the register script publish for you
copy agent-loop.example.json agent-loop.json   # then edit: clonePath, caps, models
.\publish\lunima-agent-loop.exe init           # clones Lunima → clonePath, creates dev-ki on origin
scripts\Register-AgentLoop.ps1                 # scheduled task, hourly (first run after ~15 min)
```

Useful commands:

```powershell
.\publish\lunima-agent-loop.exe status   # config, today's counters, recent runs
.\publish\lunima-agent-loop.exe run      # one full cycle right now (owner pass if due + tasks)
.\publish\lunima-agent-loop.exe work     # only worker passes
.\publish\lunima-agent-loop.exe own      # force a Product-Owner pass now (ignores the interval)
```

Pause / stop:

```powershell
# pause:   set "enabled": false in agent-loop.json  (task stays, does nothing)
# disable: Disable-ScheduledTask -TaskName LunimaAgentLoop
# remove:  scripts\Unregister-AgentLoop.ps1
```

## Setup (Linux)

Self-contained single-file publish — no .NET runtime needed on the target machine:

```bash
dotnet publish -c Release -r linux-x64 --self-contained true -p:PublishSingleFile=true -o publish/linux
./publish/linux/lunima-agent-loop init
```

Schedule with a systemd user timer or cron instead of the Windows-only register script, e.g.
`7 * * * * /opt/lunima-agent-loop/lunima-agent-loop run >> /var/log/agent-loop.log 2>&1`.

## Configuration (`agent-loop.json`)

| key | default | meaning |
|---|---|---|
| `githubRepo` | `aignermax/Lunima` | target repo |
| `clonePath` | — | dedicated local clone the agent works in (kept separate from your dev checkout) |
| `integrationBranch` | `dev-ki` | branch all PRs target; `main` is never touched |
| `maxTasksPerDay` | `2` | worker-run budget per day |
| `ownerIntervalMinutes` | `60` | min minutes between Product-Owner passes; idle passes are skipped |
| `workerModel` / `ownerModel` | `kimi-k2.7-code` / `kimi-k3` | model aliases (`kimi provider list`) |
| `workerTimeoutMinutes` | `120` | hard kill per worker run |
| `taskLabel` / `prLabel` / `blockedLabel` | `agent-task` / `agent-pr` / `needs-human` | GitHub labels that drive the loop |
| `enabled` | `true` | master switch |

## Requirements

- .NET 10 SDK (build machine only — published binaries are self-contained)
- [Kimi Code CLI](https://www.kimi.com/code/docs/en/) on `PATH`, logged in
- [GitHub CLI](https://cli.github.com/) on `PATH`, authenticated (`gh auth login`)
- Windows for the Task-Scheduler register script (Linux: cron/systemd, see above)

## Notes & limitations

- **Headless verification only.** UI changes are verified via integration tests and the existing
  screenshot-test infrastructure; real clicking is impossible on a locked machine. Issues that
  need human eyes are labelled `needs-human` / documented in the PR body for after the vacation.
- The worker contract and the Product-Owner contract live in `prompts/worker.md` and
  `prompts/owner.md` — edit those to tune behavior; no recompile needed.
