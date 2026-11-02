# Product discovery — architecture and operation

Status: implemented (role `product-discovery`, off by default), 2026-11-01.

## 1. Purpose

The delivery team (coder, QA, PR feedback, UX review, Product Owner) builds what the backlog
says. Product discovery decides **what is worth building** from evidence. The roadmap then
becomes an output of validated discovery instead of the Product Owner's only input.

The goal is **validated product learning and useful engineering progress per unit of
maintainer attention and cost**. It is not features, issues or agent activity. "Do nothing",
"park it" and "not enough evidence yet" are first-class results.

### Q1 2027 outcome

Identify and externally validate at least one genuinely valuable photonic engineering
workflow for Lunima. Supporting targets:

- 3–5 conversations with real PIC designers
- at least two workflows compared
- at least one practical external validation of an actual Lunima workflow

The weekly report counts these from the repository. It never estimates them, and a target
is never a reason to invent evidence.

## 2. Architecture

```
            maintainer: outcome, budget, interviews, approvals, merging
                                   │
  ┌──────── weekly window, one machine (lock ref discovery-<week>) ────────┐
  │ product-discovery role                                                 │
  │                                                                        │
  │  prepare ─▶ scout ─▶ strategist ─▶ critic ─▶ apply (deterministic)     │
  │  skeleton   evidence  hypotheses,   verdict   gate + caps → issues,    │
  │  DR verdicts          DRs, experi-  per item  draft PR, previews,      │
  │                       ments, pages            private outreach, report │
  │                                                                        │
  │  each LLM stage: sandboxed Claude session → revert writes outside its  │
  │  paths → validate → one repair session → salvage/discard → commit+push │
  └────────────────────────────────┬───────────────────────────────────────┘
                                   │ branch discovery/<week>, draft PR (label `discovery`)
                                   ▼
        maintainer merges (= accepts DRs) · /reject DR-NNNN · labels approved/declined
                                   │
                                   ▼
  Product Owner (existing): "Discovery decisions" prompt section → agent-task issues
  that name their DR; keeps maintenance, bugs, capacity and release readiness
                                   │
                                   ▼
  coder / QA / UX review (existing): spikes on spike/* branches, persona journeys
                                   │
                                   └──▶ issues, metrics, interview summaries feed the next scout
```

The three stages run in **one** role, one thread, one weekly run. Three daemon roles would
need three schedules and locks for what is a single pipeline. The critic runs as its own
session with its own prompt, so it does not argue from inside the strategist's context.

### What was reused

| Existing piece | Use in discovery |
|---|---|
| `Role` trait, daemon threads, pause (`control.json`), heartbeat, usage-limit wait, backoff | The role; it pauses independently of delivery |
| Lock refs (`refs/team-locks/*`), windows à la `ux/plan.rs` | Weekly window and the cross-machine lock `discovery-<week>` |
| Prompt built-ins plus disk overrides | `discovery-scout`, `discovery-strategist`, `discovery-critic`, editable on the *Regeln* page |
| `Llm`/`Job`, Claude `Outcome` (tokens, cost) | Stage sessions and the cost ledger |
| Maintained-issue pattern of `ux/report.rs` | Weekly report issue and the PO section |
| `workspace::setup`, `commit_and_push` | Clone on `discovery/<week>`, progress pushed after every step |
| Coder with `Team branch:` | Feasibility spikes on `spike/<key>` (never merged into `dev`) |
| UX review | Usability evidence, always class `simulated` |
| `FakeForge`/`FakeLlm`/local git origin | End-to-end tests |

### Additions to the shared code

- **`Session/Job.tools`:** a sandboxed session gets `--tools <list> --permission-mode
  acceptEdits` instead of `--dangerously-skip-permissions`. Discovery stages get Read, Glob,
  Grep, Write, Edit, WebSearch and WebFetch. That means no shell, so no `gh`, `git push` or
  `curl`. Edits are allowed only inside the clone: edit tools are deliberately *not*
  pre-approved, because a live check with CLI 2.1.316 showed that a pre-approved Write may
  write anywhere. The environment also points `GH_CONFIG_DIR` at an empty folder and strips
  GitHub tokens.
- **`Forge`:** `issues(label, state)`, `create_draft_pr`; PRs carry `draft`/`merged_at`/`state`.
- **Config:** `[roles] product_discovery` and the `[product_discovery]` section (§8).
- **State:** `DiscoveryState` (`state/discovery/state.json`) and the ledger
  (`state/discovery/ledger.jsonl`), both readable by the Control Center.

## 3. Data model

Everything durable lives in the target repository under **`discovery/`**. That folder is at
the repository root, not under `docs/`, because Lunima's GitHub Pages serves `docs/` of
`main` with Jekyll: Markdown there would become pages on lunima.org.

```
discovery/
  README.md                     how the maintainer decides
  HYPOTHESES.md                 H0 (status quo) … H8, never deleted, only dropped
  evidence/<week>.md            evidence entries (scout)
  decisions/DR-NNNN-<slug>.md   Decision Records (strategist + critic section)
  experiments/X-<week>-<slug>.md  pre-registered experiments, results appended
  campaigns/C-<week>-<slug>.md  paid-campaign plans, authorised budget 0
  pages/<slug>/page.json        landing-page drafts (previews only)
  interviews/I-NNNN.md          anonymised summaries (maintainer)
  pilot/ONBOARDING.md, FEEDBACK.md  pilot workflow
  runs/<week>.json              progress record of the week (resume, PR number, filed items)
```

Workflow items are GitHub issues with stable markers (`<!-- discovery:item key=… -->`):

| Label | Meaning |
|---|---|
| `discovery` | weekly draft PR — never `agent-pr`, never "Agent:" title |
| `discovery-opportunity` | an observed problem (at most 3 open) |
| `discovery-experiment` | a registered experiment (plus approval labels if it reaches people) |
| `discovery-interview` + `needs-human` | interview request for the maintainer |
| `discovery-approval` + `needs-human` | publish page, outreach, campaign, architecture … |
| `discovery-report` | the one maintained weekly report |
| `discovery-parked` | parked opportunity (closed) |
| `approved` / `declined` | set **only by the maintainer** on approval issues |

### Evidence and its levels

The class of an entry caps its level. Internal tests and simulated personas never count as
external demand.

| Class | Max level | Accepted source |
|---|---|---|
| `agent-hypothesis` | E0 | none needed |
| `internal-test` | E0 | URL, issue or repository path |
| `simulated` | E0 | URL, issue or repository path |
| `external` | E1 | URL (public docs, issues, forums, papers, competitor material) |
| `user-feedback` | E2 | interview summary `I-NNNN`, a user's own issue, or `metrics:<page>` |
| `user-commitment` | E3 | same as `user-feedback` |

More rules:

- Every non-E0 entry needs a verbatim quote.
- Duplicates are dropped automatically: same normalised URL (tracking parameters stripped)
  and nearly the same quote, compared across all weeks.

### Validation (code, not prompt)

| Artefact | Enforced |
|---|---|
| Evidence | id format, class/level cap, source rules above, quote required, ≤ 600 characters |
| Hypotheses | no `H<n>` may disappear; status from the fixed set |
| Decision Record | ≥ 2 options incl. "do nothing"; known evidence ids; `pursue` needs a cited E2+ entry; `## Delivery`; after the critic a `## Critic` verdict; new DRs are `proposed`; an existing DR may only become `superseded`; its decision cannot change |
| Experiment | method from the fixed set; external methods are `needs-approval`; success/failure/measurement/timebox; the pre-registration must equal the version on the integration branch (results are appended) |
| Campaign | all plan fields; `## Compliance`; the authorised budget can only be what the maintainer set (default 0) |
| Page | slug = folder; honesty note; `available` claims cite an existing repository path; no performance numbers or superlatives; figures are existing repository images; stays `draft` with no approval stamp |
| Everything public | no e-mail addresses, phone numbers, private paths, tokens or confidentiality markers |

## 4. The run

1. **Due:** from the configured weekday and hour on. A machine that was off catches up later
   in the same week. Every start counts as an attempt (at most `max_attempts`; errors retry
   after 30 minutes).
2. **Lock:** `refs/team-locks/discovery-<week>`, held by one machine. It is kept after a
   finished week as proof and released after an error, pause or usage limit. A lock older
   than three stage timeouts plus 30 minutes belongs to a crashed machine and is broken,
   unless the week's report already exists.
3. **Prepare:** clone on `discovery/<week>` (resumed from origin if it exists), create the
   skeleton if missing, write the maintainer's merge/reject verdicts into DR files.
4. **Stages** (skipped if `runs/<week>.json` says they are done):
   - Before each stage: a pause check, a budget check, and a fresh context folder
     (`.discovery/context/`, excluded from git) with the brief, Lunima issues, discovery
     issues and the maintainer's answers, DRs with effective status, the evidence index, the
     UX report, metrics, last week's critic and local `context_files`.
   - **Scout:** writes only `evidence/<week>.md`. Duplicates are removed.
   - **Strategist:** writes hypotheses, DRs, experiments, campaigns, page drafts and
     `.discovery/out/actions.json` (opportunities, interviews, approvals, outreach, spikes).
   - **Critic:** a `## Critic` section in each new DR and `.discovery/out/critic.json` with a
     verdict (`stands | weaken | reject`) for every item.
   - After each session, writes outside the stage's allowed paths are reverted. If
     validation fails, one repair session gets the exact problems. If problems remain:
     invalid evidence entries are dropped, invalid files restored, unusable actions replaced
     by "nothing", and a DR without a valid critic review is **withdrawn**. Then the stage is
     committed and pushed.
5. **Apply** (no LLM):
   - **Gate.** A critic `reject` is not filed. Items without a verdict are not filed
     (fail-safe). Caps apply: open opportunities, new experiments, spikes, and issues per run.
     Items with a privacy finding are not filed.
   - **Filing.** Issues are created idempotently. The marker and the run record are written
     before labelling, so a crash never files twice.
   - **Opportunities.** Retired ones are parked: a comment, the `discovery-parked` label, and
     the issue is closed.
   - **Private files.** Outreach drafts go to `state/discovery/private/outreach-<week>.md`.
     Page previews go to `state/discovery/previews/<slug>/index.html`.
   - **PR.** A draft PR `Discovery: week <week>` against the integration branch, labelled
     `discovery`.
   - **Report.** The maintained report issue plus `state/discovery/report.md`, and the
     Control Center state.

## 5. Governance

| Autonomous | Needs the maintainer |
|---|---|
| public web research, evidence, hypotheses | publishing a page (`experiment publish`, merge to `main`) |
| issues and draft DRs within caps | any message to a person (outreach drafts are private; the maintainer sends them) |
| experiments without external reach | interviews and pilots (the maintainer runs them) |
| spikes on `spike/*` via the coder | connecting analytics or any third-party service |
| page and campaign drafts, previews | any money: campaigns, tools, incentives (authorised budget is 0) |
| benchmarks and tests in delivery | architecture changes, major dependencies, public API breaks, confidential designs |

Approval requests state the action, rationale, evidence, benefit, risk, cost and rollback. No
answer means not approved; the request stays in the weekly report and is not re-filed.
Independent work goes on while an approval is pending.

**Accepting Decision Records:**

- The maintainer marks the draft PR *Ready for review* and merges it.
- A PR comment `/reject DR-0003 <reason>` before merging rejects one DR.
- `review-minutes: N` in a comment records the review time.
- `decisions::effective` computes accepted/rejected right after the merge; the next run
  writes it into the files.

**Accidental-merge protection:**

1. The PR is a draft, and GitHub refuses to merge drafts, even with admin rights.
2. It carries `discovery`, not `agent-pr`, so the PO's merge list doesn't include it.
3. Its title doesn't start with "Agent:", so QA ignores it.
4. The PO prompt forbids touching `discovery`/`spike` PRs and `discovery/` files.
5. A guard snapshots open discovery PRs before every PO pass. If one is merged or
   un-drafted afterwards, it comments on the PR and files a `needs-human` issue.

Caveat: pr-feedback still acts on an explicit `@agent` comment on any PR, including discovery
PRs. That only happens on the maintainer's request.

## 6. Hand-over to delivery

The PO prompt gets a **"Discovery decisions"** section with:

- accepted DRs whose decision is `pursue` or `experiment` and that have delivery items,
- for each, whether a delivery issue naming the DR (`Decision: DR-NNNN`) already exists,
- the rules: strategic initiatives need an accepted DR or another documented source; bugs,
  regressions, security fixes, benchmark-proven optimisations, maintenance and debt don't,
  and keep their priority; the PO keeps deciding capacity.

While discovery is on, the PO's own product-market-fit lens is off and ROADMAP PRs must cite
a DR.

## 7. Real-world validation

- **Landing pages.** `discovery/pages/<slug>/page.json`, rendered with
  `assets/experiment-page.html`: lunima.org's design tokens, no external resources,
  `noindex`, a mandatory honesty note, availability badges (available / planned /
  research). The CTA is a `mailto:` with the experiment id in the subject, so no form
  backend collects data.
- **Previews.** Local only, with a "PREVIEW — not published" ribbon. They are written by the
  daemon or with `lunima-team-agent experiment preview <clone> <out>`.
- **Publishing.** The maintainer runs `lunima-team-agent experiment publish <clone> <slug>
  --approved-by <name> --contact <address> [--goatcounter <code>]`. This validates the
  draft, writes `docs/x/<slug>/index.html` and stamps the approval; nothing is committed. In
  Lunima, the workflow `Experiment pages guard` (`scripts/check_experiment_pages.py`) refuses
  a page under `docs/x/` without that stamp, and any preview render.
- **Analytics** (`analytics = none | file | goatcounter`). The default `none` tells the
  strategist that metrics are unavailable and must not be inferred. `file` reads a JSON
  export the maintainer maintains. `goatcounter` (cookieless) is used only after the
  maintainer passes `--goatcounter` at publish time; the token travels on curl's stdin.
  Visits count per `/x/<slug>/`, clicks per `cta-<kind>-<slug>` event.
- **Outreach.** At most five people per week, chosen for public, relevant technical work.
  Each gets a public source, the reason for relevance, a hypothesis, a public channel (never
  a scraped address) and a short personal draft. These are stored privately on the agent
  machine. A public approval issue mentions only the count. Nothing is ever sent.
- **Campaigns.** Plans with audience, hypothesis, channel, landing page, proposed budget,
  hard cap, duration, stop conditions, measurement and a compliance checklist. The
  authorised budget is the maintainer's field and stays 0.
- **Interviews and pilots.** Interview requests with past-behaviour questions. The
  maintainer keeps raw notes private and commits anonymised summaries
  (`interviews/TEMPLATE.md`) with noted consent. `pilot/ONBOARDING.md` and `FEEDBACK.md`
  say that confidential designs never leave the participant.

## 8. Configuration and operation

```toml
[roles]
product_discovery = true          # off by default; pause independently in the Control Center

[product_discovery]
repo = ""                         # "" = product_owner.repo
weekday = "mon"                   # window start; a missed window runs later that week
hour = 6
model = ""                        # "" = models.product_owner
max_turns = 120                   # per stage
timeout_minutes = 45              # per stage (hard kill)
max_attempts = 3                  # starts per week
weekly_budget_usd = 25.0          # planning limit, notional under the subscription — not a permission to spend
stage_estimate_usd = 4.0          # until the ledger has history
max_open_opportunities = 3
max_new_experiments = 2
max_new_issues = 6                # per run, all kinds
max_spikes = 1
private_dir = ""                  # "" = <data>/state/discovery/private
analytics = "none"                # none | file | goatcounter
analytics_site = ""
analytics_token_file = ""
context_files = []                # local notes copied into the run context, never committed
```

- **Run one cycle by hand:** `lunima-team-agent once product-discovery`.
- **Emergency stop:** pause the role (Control Center or `state/control.json`). The running
  stage finishes, and the run stops before the next one without counting as a failed
  attempt. `[roles] product_discovery = false` switches it off.

**Cost.** Every stage writes a ledger line: stage, model, duration, tokens, turns, cost and
billing. Under the subscription, billing is `subscription-notional`, so no money is charged.
The budget check runs *before* each stage, using the highest of that stage's last four costs.
It is therefore not a hard cap: a running stage can overshoot by at most one stage, which
`max_turns` and `timeout_minutes` bound. Nothing upgrades subscriptions, buys credits or
enables paid tools.

## 9. Tests

Unit tests cover the parsers and validators, the plan, ledger, analytics, renderer, gate,
filing and DR verdicts. End-to-end tests in `product_discovery/tests.rs` use real git
against a local origin, a simulated GitHub and scripted Claude stages:

- a full week:
  - permissions and sandbox
  - duplicate evidence dropped, invalid evidence repaired
  - writes outside the stage's paths reverted
  - critic reject not filed, caps applied
  - draft PR not mergeable, outreach private, preview, report, ledger
  - once per week
- a crash and resume on another machine without repeated stages, and an idempotent re-run
- lock contention and a stale lock
- the budget stop between stages
- a pause between stages, not counted as an attempt
- the usage limit
- a missing critic review: nothing filed, the DR withdrawn
- PO hand-over and the merge guard

## 10. Known limits

- The budget is checked between stages, not during them (see above).
- Write confinement relies on the Claude CLI's `acceptEdits` semantics (verified for
  2.1.316). Reverting writes outside the allowed paths is the second line of defence inside
  the clone.
- Outreach drafts are local to the machine that ran the week.
- `effective` DR status needs the run record's PR number. A DR merged by any other route
  stays `proposed`.
- The architecture review's MCP capability matrix is an input. An end-to-end MCP run against
  the GUI (create → simulate → validate) is a spike for delivery, not something discovery
  executes itself.
