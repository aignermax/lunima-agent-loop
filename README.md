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
              claim the issue (agent-running label) so no other machine starts it too
              branch agent/issue-<n>-<ts> from dev-ki
              .agent-loop/task-<n>.md = the work contract (prompts/worker.md)
              kimi -p "...execute task file..." --output-format stream-json
              agent implements, runs the test suite, pushes, opens PR → dev-ki
```

Guardrails: daily task cap, never push to `main`, no force-push/`--admin` by the worker,
full test suite must be green before a PR, confidential data (customer PDKs) must never be
committed. Issues being worked on carry the `agent-running` claim label — safe to run the loop
on more than one machine without duplicate work (but all machines share the same Kimi account
quota, so two machines burn the budget roughly twice as fast). The real budget cap lives on the
Kimi account (monthly quota); the daily caps here only pace the spend. Every run is logged to
`logs/` and `state/state.json`.

## Setup (Windows)

```powershell
git clone https://github.com/aignermax/lunima-agent-loop; cd lunima-agent-loop
scripts\Setup-Machine.ps1    # checks deps, creates config (edit + re-run), builds, inits, registers
```

`Setup-Machine.ps1` verifies the prerequisites (.NET 10 SDK, `kimi` CLI logged in, `gh` CLI
authenticated), creates `agent-loop.json` from the example on first run, then builds, runs
`init` (clones Lunima → `clonePath`, creates `dev-ki` on origin if missing) and registers the
hourly scheduled task. Use `-NoRegister` to skip the scheduler.

Manual equivalent:

```powershell
dotnet build
copy agent-loop.example.json agent-loop.json   # then edit: clonePath, caps, models
.\bin\Release\net10.0\lunima-agent-loop.exe init
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

## Install from a release (any PC, no SDK)

Every `v*` tag builds a **NativeAOT** single executable (~4 MB, no .NET runtime needed) for
`win-x64`, `win-arm64`, `linux-x64`, `linux-arm64` and `osx-arm64` — see
[Releases](https://github.com/aignermax/lunima-agent-loop/releases). Prompts and the example
config are compiled in; files in a `prompts/` folder next to the config override them.

**Windows: MSI** (`lunima-agent-loop-<version>-x64.msi` / `-arm64.msi`) installs the exe to
`C:\Program Files\Lunima Agent Loop` and adds it to the system PATH. Config, state and logs
then live in `%LOCALAPPDATA%\lunima-agent-loop` (untouched by upgrades and uninstall):

```powershell
lunima-agent-loop init    # 1st run: writes %LOCALAPPDATA%\lunima-agent-loop\agent-loop.json — edit clonePath/models, set "enabled": true
lunima-agent-loop init    # 2nd run: clones the repo, ensures the integration branch
```

**Zip / tar.gz:** unzip anywhere and run the same two `init` calls in that folder — an
`agent-loop.json` in the current folder (or next to the exe) takes precedence over the data folder.
Without one, Linux uses `~/.local/share/lunima-agent-loop`, macOS `~/Library/Application Support/lunima-agent-loop`.
A freshly created config ships with `"enabled": false`, so a scheduler firing early does nothing.

The target machine still needs the tools the loop drives: `git`, `gh` (logged in), `kimi`
and — for `ownerRunner: "claude"` — the `claude` CLI, plus the .NET SDK the *Lunima* build needs.
To cut a release: `git tag v0.2.0 && git push origin v0.2.0`.

## Building locally

`dotnet publish` produces the NativeAOT binary (`-r win-arm64`, `-r linux-x64`, … for other
targets). It needs the platform C/C++ toolchain: on Windows *Visual Studio / Build Tools with
"Desktop development with C++"* (`Register-AgentLoop.ps1` puts `vswhere.exe` on PATH for the link
step); on Linux `clang` + `zlib1g-dev`. ARM64 Windows is fully supported natively.

## Setup (Linux)

Use a release binary (or `dotnet publish -c Release -r linux-x64`), then run `init` as above.

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
| `taskLabel` / `prLabel` / `blockedLabel` / `runningLabel` | `agent-task` / `agent-pr` / `needs-human` / `agent-running` | GitHub labels that drive the loop; `agent-running` is the cross-machine claim |
| `enabled` | `true` | master switch |

A pause (see below) is stored separately in `state/state.json`, not here — so pausing never
edits your config, and resuming can't accidentally re-enable a loop you turned off on purpose.

## UX tester — Claude clicks through the real app

`tools/ux-tester/ux_tester.py` closes the gap the headless pipeline cannot: nobody in the
worker → test-gate → PO chain ever *uses* the app. The tester launches Lunima on an unlocked
desktop session and lets Claude (computer use, `computer_toolset_20260801`) walk the manual
items of `docs/RELEASE-CHECKLIST.md` like a user — clicking, typing, zooming into small text —
while judging like a UX designer: right-sidebar clutter, overlapping components/routes,
walls of help text, missing feedback. After every action it measures whether the window is
hung (Win32 `IsHungAppWindow`) and tells Claude how long, so "the UI hangs" becomes a number.

```
pip install anthropic pyautogui mss pygetwindow pillow
set ANTHROPIC_API_KEY=...
py -3 tools/ux-tester/ux_tester.py ^
   --app "C:\...\Lunima-dev-ki\CAP.Avalonia\bin\Debug\net10.0\CAP.Avalonia.dll" ^
   --checklist "C:\...\Lunima-dev-ki\docs\RELEASE-CHECKLIST.md" --max-steps 150
```

Output: `tools/ux-tester/reports/<stamp>/report.md` (+ `findings.json`, screenshots,
`transcript.jsonl`). Add `--file-issues --lunima-clone <path>` to open one GitHub issue per
finding (label `ux-finding`, `--min-severity major` by default); screenshots are pushed to a
`ux-findings/<stamp>` branch so they render in the issue. `--attach` reuses a running window,
`--scenarios file.txt` adds your own steps, `--limit N` trims the list.

Do not touch mouse or keyboard while it runs; moving the mouse into the top-left corner aborts
(pyautogui fail-safe). Budget: a 150-step run on Fable is roughly 20–40 minutes and a few dollars
in screenshots; prompt caching is on.

## Independent customer role → Product Owner

The existing desktop tester is now part of `run` and `own` when `customerEnabled` is
true. The customer role uses Claude's [computer-use toolset](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool)
to operate the real Windows app. It receives persona goals, not source code or a
developer's click recipe. It assesses task completion **and** visual hierarchy,
consistency, typography, spacing, discoverability, feedback and recovery.

```
scheduled run / own
  → independent customer: new PR head(s) + integration baseline
  → evidence and scenario verdicts in state/customer/feedback.md + .json
  → PO reads customer evidence before merge / prioritization
  → deduplicated, outcome-based issues for the existing Autonomous Issue Agent
  → worker PR / updated commit → new customer review
```

Every review checks out the requested commit into a fresh directory, builds
`CAP.Desktop/CAP.Desktop.csproj`, and launches that output. The working Lunima clone
is never checked out, reset or cleaned by the customer. Drafts, forks and unrelated
PRs are excluded; eligible PRs target the integration branch and carry `agent-pr`
or an `Agent:` title. Reviews use least-recently-attempted order, with new commits
first and the baseline winning ties. A failing baseline or older PR cannot keep
later PRs waiting indefinitely, even with a one-review budget.

The bundled goals in `tools/ux-tester/customer_scenarios.json` cover Ingrid's first
use, Jonas learning and composing logic, Peter editing precisely and inspecting
the manufacturing path, and a design coherence review. The customer reads a brief
derived from issue #537's latest North star and the integration commit's
`docs/ROADMAP.md` and `docs/PERSONAS.md`: education-first **NAND2TETRIS for photonics**,
gates → circuits → systems, visible computation and eventual fabrication.
The complete versioned sources are saved in `state/customer/strategy.json`; their
fingerprint participates in acceptance identity. Unavailable sources block the
cycle, and source changes during a run invalidate its acceptance. The customer
gets product intent and persona definitions, without implementation/rung notes
or prescribed clicks. It restates its understanding in the transcript for the
PO to check. Future roadmap ambitions are not treated as shipped features, and
simulation/DRC-lite/GDS export are never proof of foundry readiness.
Edit the **trusted runner-side** scenarios to add a targeted journey for a new
feature; the six default journeys do not cover every possible feature.
For each PR, the PO must also write `state/customer/goals/pr-<number>.json` with
`sha` set to the current PR head and a nonempty `goals` array using the same
`id/persona/goal/success` schema. The PO receives these instructions automatically.
Missing/stale PR-specific goals block acceptance: generic smoke journeys alone
cannot approve a feature they never exercise. The next scheduled pass tests the
combined goals; no human needs to copy the report between roles. These are simulated
users, not human research or a guarantee that a redesign is good.

### Set up once, after merging

Use a dedicated, unlocked Windows test desktop/account or VM, Python 3.11+, .NET 10,
`git`, authenticated `gh`, and `ANTHROPIC_API_KEY` in the scheduled task's environment.
Do not use the mouse/keyboard while the customer runs. A shared everyday desktop
can contain notifications and user data even when captures are cropped to the app.
The test process supplies fresh profile environment directories to Lunima, but
this is **not an OS sandbox**: Windows known-folder APIs can still resolve the
account's normal folders. Use a dedicated account with only public test fixtures.

```powershell
scripts\Setup-CustomerReview.ps1
publish\lunima-agent-loop.exe customer   # test only; no PO, issues, merges or workers
publish\lunima-agent-loop.exe own        # customer evidence, then the PO
```

The setup script creates `.customer-venv`, installs dependencies, publishes the
loop and updates only the customer fields in `agent-loop.json`. It does not store
an API key, start a test or change Task Scheduler. Keep the existing task's working
directory pointed at this checkout. Python tools must accompany the executable;
deploying only the `.exe` is insufficient. Existing installations stay opted out
until configured; no running service is silently changed by this PR.

Manual configuration:

| key | default | meaning |
|---|---|---|
| `customerEnabled` | `false` | opt in to desktop customer reviews before every due PO pass |
| `customerPython` | `python` | Python executable with `tools/ux-tester/requirements.txt` installed |
| `customerModel` | `claude-fable-5-1` | configurable computer-use model; independent of worker/PO model |
| `customerProject` | `CAP.Desktop/CAP.Desktop.csproj` | project to build in each fresh checkout |
| `customerMaxReviewsPerCycle` | `2` | at most two due targets; cache hits consume no slot |
| `customerMaxAgeHours` | `24` | acceptance expiry, even for an unchanged commit |
| `customerMaxSteps` / `customerMaxTurns` | `80` / `60` | per-review action and model-turn budgets |
| `customerTimeoutMinutes` | `20` | desktop-run limit; checkout/build have separate bounds |

Both pause and the master `enabled` switch also apply to `customer`. State caches
are keyed by repo, PR, commit, goals, strategy sources, model and harness policy. New commits invalidate
old results; a push during a review blocks the result. Infrastructure blocks retry
after an hour; normal verdicts expire at the configured age. A per-cycle lock
prevents two customer sessions sharing the same state directory from using the
desktop concurrently. Use one runner/state directory per test desktop.

### Evidence and acceptance

- `PASSED`: every goal has an interaction, an observation and an existing screenshot;
  no failed goals or major/critical findings; the model ended normally.
- `NEEDS_CHANGES`: observed failed goal or major/critical friction.
- `BLOCKED`: locked desktop, missing key/dependency, wrong build, failed launch,
  interrupted/budget-exhausted run, incomplete coverage or missing evidence.
- `PENDING`: not run yet because the per-cycle budget was reached.

Every meaningful action receives a fresh screenshot. Customer input is bounded to
the launched app's process/window; a focus change ends the session rather than
clicking another app. Reports, model transcripts and screenshots remain local in
`state/customer/runs/`. Nothing is automatically uploaded or filed by this role.
Run directories include builds and can grow large; archive/remove old completed
runs during maintenance, while retaining evidence for any accepted current PR.

The PO is instructed to accept only fresh `PASSED` evidence for the current PR head,
compare the SHA immediately before merging, and use `--match-head-commit`. This is
a **PO workflow rule**, not a GitHub branch-protection check: human/admin merges
outside the loop are unaffected. Baseline evidence never approves a PR. Blocked
test infrastructure is not filed as a product defect. The PO converts actual UX
findings into deduplicated issues for the existing worker, with persona, goal,
reproduction and retest criteria, and may choose to create no new feature tasks.

### Validate the integration without a desktop or API calls

```powershell
dotnet build
dotnet run --project tests/CustomerIntegration
python -m unittest discover -s tests -p "test_customer*.py" -v
```

The tests drive the real report loop with scripted API/desktop adapters and exercise
the C# → Python handoff. They do not substitute for a live customer run after setup.

## Pausing the loop

Going on vacation, or just want the machine to stop thinking for a while? Pause it:

```
lunima-agent-loop pause                       # indefinitely, until you resume
lunima-agent-loop pause 14 vacation           # 14 days, with a reason
lunima-agent-loop pause 2026-09-01 vacation   # through the end of that day
lunima-agent-loop resume
lunima-agent-loop status                       # shows "Paused: …"
```

The pause lives in `state/state.json`, so it **survives reboots, Windows updates and
re-registered scheduled tasks** — the scheduler may fire, but every pass (owner *and*
worker) exits immediately, printing the pause reason. A dated pause lifts itself when
it elapses; an indefinite one waits for `resume`.

For a *hard* stop, combine it with the two other switches — they are independent by design:
`enabled: false` in `agent-loop.json` and `Disable-ScheduledTask -TaskName LunimaAgentLoop`.

## Requirements

- .NET 10 SDK (build machine only — published binaries are self-contained)
- [Kimi Code CLI](https://www.kimi.com/code/docs/en/) on `PATH`, logged in
- [GitHub CLI](https://cli.github.com/) on `PATH`, authenticated (`gh auth login`)
- Windows for the Task-Scheduler register script (Linux: cron/systemd, see above)

## Notes & limitations

- **Desktop availability.** Mechanical tests can run headlessly. The optional customer role
  requires an unlocked test desktop; locked sessions are explicitly `BLOCKED`, never UX-passed.
- The worker contract and the Product-Owner contract live in `prompts/worker.md` and
  `prompts/owner.md` — edit those to tune behavior; no recompile needed.
- A crashed worker can leave a stale `agent-running` claim behind; the Product-Owner pass
  removes claims older than ~6 h so the issue is retried.
