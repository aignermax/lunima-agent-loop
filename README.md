# lunima-agent-loop — the Lunima agent team

Autonomous issue → PR team for the [Lunima](https://github.com/aignermax/Lunima) photonics IDE
and the Akhetonics repositories: one native Rust daemon, **`lunima-team-agent`**, runs every role
on Windows and Linux (no WSL, no Python, Claude on the subscription login, GitHub via the `gh`
login — no tokens in any config):

| role | what it does |
|---|---|
| `coder` | implements `agent-task` issues as PRs (`Agent: …`) into the integration branch (`dev`), repairs `qa-failed` PRs |
| `qa` | builds/tests agent PRs per the repo's `.agent.toml`, posts a verdict, labels `qa-passed` / `qa-failed` |
| `pr-feedback` | implements `@agent …` comments on PRs |
| `product-owner` | one pass per interval: reviews + merges agent PRs, keeps `dev` fresh, grooms and seeds the backlog from `docs/ROADMAP.md` |

Architecture, data folder, multi-machine claims and the differences to the former Python
agent / C# loop: [`docs/TEAM-AGENT.md`](docs/TEAM-AGENT.md).

```powershell
lunima-team-agent init          # writes team.toml (imports the old .env / agent-loop.json if found)
lunima-team-agent status        # config, Claude CLI, gh login, repos
lunima-team-agent once product-owner   # one cycle of a role, for testing
lunima-team-agent run           # all roles, forever (scheduled task / systemd user unit)
```

Configuration is a single `team.toml` in the data folder (`%LOCALAPPDATA%\lunima-team`,
`~/.local/share/lunima-team`); the Control Center edits it. The Product Owner's settings live in
`[product_owner]` (`repo`, `integration_branch`, `base_branch`, `interval_minutes`,
`timeout_minutes`, `max_runs_per_day`, `backlog_min`, `max_turns`, `customer_command`) and the
role is switched on with `[roles] product_owner = true`. Pauses are set per role in the Control
Center (`state/control.json`).

## Control Center (tray app)

`lunima-po.exe` (Rust, `control-center/`) shows whether the loop works and what it did — and
lets you fix it without a terminal. It reads the same folder as the CLI (`agent-loop.json`,
`state/`, `logs/`, `prompts/`) and drives the loop through the CLI itself.

| Page | What you see / do |
|---|---|
| **Übersicht** | Traffic light + headline, today's counters, next scheduled run, **health checks** with one-click fixes, recent runs |
| **Aktivität** | Every PO pass: issues created, PRs merged, labels, comments (with links), final report, duration, cost |
| **GitHub** | Newest issues (flags new ones **without** the task label), open PRs (flags wrong base branch) |
| **Einstellungen** | Form over `agent-loop.json` (unknown keys preserved), schedule on/off, "start with Windows" |
| **Regeln** | Edit `prompts/owner.md` / `worker.md` (disk overrides the built-in prompts), restore defaults |
| **Protokolle** | Raw logs incl. the loop's console mirror `logs/loop-<date>.log` (real error behind a failed run) |

Health checks — each one once stopped the loop unnoticed: config/CLI present, `enabled`, pause
(→ *Fortsetzen*), scheduled task missing/disabled (→ *Einschalten*), last PO pass failed (expired
Claude login is called out), worker failure streak, **dirty clone** blocking every worker
(→ *Als WIP sichern*: commits the changes to a local `wip/control-center-*` branch), `gh` login.

The tray icon has the same colour as the traffic light; its menu offers open / pause-resume /
run PO now / quit. Closing the window keeps the app in the tray.

```powershell
lunima-po.exe                          # root: cwd → exe folder → %LOCALAPPDATA%\lunima-agent-loop
lunima-po.exe --root C:\path\to\loop    # explicit loop folder
lunima-po.exe --tray                   # start hidden (what "start with Windows" uses)
lunima-po.exe --page activity          # open a page: overview|activity|github|settings|rules|logs
```

Build: `cargo build --release --manifest-path control-center/Cargo.toml` (tests: `cargo test`).
Rendering uses wgpu (DirectX 12 on Windows) — Windows-on-ARM GPUs have no OpenGL 2+.

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

The desktop tester runs before each Product-Owner pass when `[product_owner]
customer_command` is set (e.g. `python tools/ux-tester/customer_cycle.py --repo aignermax/Lunima
--base dev --feedback "%LUNIMA_CUSTOMER_FEEDBACK%"`); the PO reads the evidence file. The customer role uses Claude's [computer-use toolset](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool)
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
python -m venv .customer-venv
.customer-venv\Scripts\pip install -r tools/ux-tester/requirements.txt
lunima-team-agent once product-owner   # customer evidence, then the PO
```

`customer_command` is any shell command; it runs in the PO's clone with
`LUNIMA_CUSTOMER_FEEDBACK` pointing at the evidence file the PO reads
(`state/customer/feedback.md` in the data folder). The tool's own options (`--model`,
`--project`, `--max-reviews`, `--max-age`, `--max-steps`, `--max-turns`, `--timeout`) go on that
command line; see `python tools/ux-tester/customer_cycle.py --help`.

Pausing the product-owner role also pauses the customer. State caches
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
python -m unittest discover -s tests -p "test_customer*.py" -v
```

The tests drive the real report loop with scripted API/desktop adapters. They do not
substitute for a live customer run after setup.

## Requirements

- `git`, the [GitHub CLI](https://cli.github.com/) (`gh auth login`) and the
  [Claude Code CLI](https://code.claude.com/) (logged in with the subscription) on `PATH`
- whatever the target repos build with (Lunima: .NET 10 SDK)
- Rust (stable) to build from source: `cargo build --release --workspace`

## Notes & limitations

- **Desktop availability.** Mechanical tests can run headlessly. The optional customer role
  requires an unlocked test desktop; locked sessions are explicitly `BLOCKED`, never UX-passed.
- Prompts are compiled in (`crates/team-core/prompts/`); a file of the same name in the data
  folder's `prompts/` overrides one (the Control Center's rules editor writes those).
- A crashed worker can leave a stale `agent-running` claim behind; the Product-Owner pass
  removes claims older than ~6 h so the issue is retried.
