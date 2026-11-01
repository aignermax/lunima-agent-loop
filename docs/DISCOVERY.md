# Product discovery — design (draft, not implemented)

Status: proposal, 2026-11-01. Nothing in this document is implemented yet.

## 1. Why

The team agent today is a delivery organisation. The Product Owner seeds the backlog from
`docs/ROADMAP.md`, the coder implements it, QA and the UX review check it. The roadmap is the
*input*, and a human writes it.

Discovery turns this around: **the roadmap becomes an output of validated discovery.**
Agents look for real problems, collect traceable evidence, challenge assumptions (including
our favourite ones) and write evidence-backed Decision Records. The Product Owner still
decides what gets built, when, and with how much of the delivery capacity.

### Q1 2027 outcome (the system's only success measure)

Identify and externally validate **at least one genuinely valuable photonic engineering
workflow**:

| Target | Measured by |
|---|---|
| 3–5 conversations with real PIC designers | interview issues with an anonymised summary |
| ≥ 2 workflows compared | Decision Record comparing them, with evidence on both sides |
| ≥ 1 practical external validation | an E3 evidence entry (see §4.2) |

### Non-goals

- No new delivery machinery. Spikes, analysis and checks reuse the existing roles.
- No settled strategy. *Agent-first*, *visual-first*, *GDS reconstruction* and *chiplet
  simulation* are **competing hypotheses** that discovery investigates. None of them is a
  premise.
- No issue throughput target. A week that ends with "nothing worth doing" is a valid result.
- No separate repository yet. Everything lives in `aignermax/Lunima` (see §9 for when to split).

## 2. What already exists and gets reused

The daemon was inspected (`crates/team-agent`, `crates/team-core`, prompts, Control Center).
Discovery reuses these parts as they are:

| Existing piece | Where | Reuse for discovery |
|---|---|---|
| `Role` trait, one thread per role, pause via `control.json`, heartbeat, shared usage-limit wait, backoff | `role.rs`, `daemon.rs` | One new role `product-discovery`. It gets pause, status and the usage-limit handling for free. |
| Run windows plus a cross-machine run record via lock refs | `ux/plan.rs` (`in_window`, `window_day`, `lock_key`), `forge.try_lock` | Weekly window (`weekday` + `hour`). The lock `refs/team-locks/discovery-<yyyy>-w<ww>` guarantees one run per week across machines. |
| Built-in prompts with disk overrides, `str.format` rendering | `team-core/src/prompts.rs` | Three new built-ins: `discovery-scout`, `discovery-strategist`, `discovery-critic`. Max can edit them on the *Regeln* page. |
| `Llm` / `Job` with subscription env, model, `max_turns`, hard time limit, per-session JSONL log | `llm.rs`, `po/pass.rs` | One `Job` per stage, unchanged. `--dangerously-skip-permissions` already allows WebSearch/WebFetch. |
| Claude result with `cost_usd` and token usage | `team-core/src/claude/parse.rs` (`Outcome`) | Source of the discovery cost ledger (§7). |
| One maintained GitHub issue as team-wide evidence, injected into the PO prompt as fenced, untrusted text | `ux/report.rs` (`publish`, `po_section`) | Same pattern for the weekly discovery report and the PO's "Discovery decisions" section. |
| Isolated clone per role, reset to the integration branch | `workspace.rs`, `po/pass.rs::prepare` | The discovery clone on `dev`. It writes evidence and Decision Records on a weekly branch. |
| UX review (simulated personas) and the customer cycle | `ux/`, `tools/ux-tester` | Usability experiments: the strategist writes persona goals and the existing UX review runs them. The output counts as **internal / simulated** evidence, never external. |
| Coder with `Team branch:` directive | `directives.rs`, `coder/` | Feasibility spikes: an `agent-task` issue with `Team branch: spike/<slug>`. The PR targets the spike branch, never `dev`, and is not merged. |
| `needs-human` label | `config.rs` `Labels` | Marks every item waiting for Max (approval, interview). |
| GitHub Pages site `lunima.org` (`docs/`, `docs/site`) | Lunima repo | Phase 3 only: landing pages and fake doors, always behind human approval. |

### Overlap that has to be removed, not duplicated

The `po-owner` prompt already contains two discovery fragments:

- duty 4, "Product-market-fit lens" (web research, ROADMAP proposals),
- duty 5, the strategic half of the "Kill review".

Once the discovery role runs, both **move out of the PO**. The PO keeps the kill review of its
own merges and backlog, which is about delivery quality. ROADMAP edit PRs must then cite an
accepted Decision Record. Otherwise there would be two uncoordinated places making strategy.

### Gaps found (small, needed)

1. **Name collision.** `[discovery]` in `team.toml` and `repos.rs` already means "find org
   repos with open `agent-task` issues". The new config section is `[product_discovery]` and
   the role is `product-discovery`. "Discovery" alone stays ambiguous in code.
2. **No cost record outside the coder.** `HistoryRecord` is written only by the coder, and the
   PO stores no cost at all. Discovery needs a ledger (§7). The PO could later use it too.
3. **`Forge` lists only open issues.** Deduplication against closed or rejected opportunities
   needs `issues(label, state=all)`. That is a one-method extension of `Forge`, `gh::Repo` and
   `FakeForge`.
4. **The PO merges agent PRs with `--admin`.** Discovery PRs must therefore **not** carry
   `agent-pr`. They get their own label (`discovery`), and the PO prompt forbids merging them.
   Max merges them, and that merge is the approval.
5. **The Lunima repo is public.** Raw interview notes, names and anything customer-specific
   must never reach it (§6.3).

## 3. Architecture

```
          Max: outcome, budgets, interviews, approvals
                          │
   ┌──────────── weekly window (one machine, lock ref) ───────────┐
   │  product-discovery role = three sequential Claude sessions   │
   │                                                              │
   │   scout ───────▶ strategist ───────▶ critic                  │
   │   evidence only   hypotheses,         attacks every          │
   │   (sourced)       opportunities,      proposal; verdict      │
   │                   experiments,        per item               │
   │                   draft DRs                                  │
   └──────────────────────────┬───────────────────────────────────┘
                              │ one PR `discovery/2027-w03` → dev (label `discovery`)
                              │ + issue updates + weekly report issue
                              ▼
   Max reviews ── merge = DRs accepted ── needs-human items answered
                              │
                              ▼
   Product Owner (existing): reads accepted DRs, turns them into agent-task
   issues within capacity, keeps maintenance and delivery priorities
                              │
                              ▼
   coder / QA / UX review (existing) ── spikes on spike/* branches,
   persona journeys for usability experiments ── results flow back as evidence
```

The three stages run **in one role, one thread, one weekly run**. Three daemon roles would need
three schedules, three locks and three Control Center entries for something that is a single
pipeline. The critic runs as a **separate session with its own prompt**, so it does not argue
from inside the strategist's context.

## 4. Data model

Discovery state lives in two places, chosen by what it is used for:

- **Files in the repo** for durable, versioned, reviewable knowledge: evidence,
  hypotheses, Decision Records. They change only through the weekly PR.
- **GitHub issues** for workflow and conversation: opportunities, experiments, human tasks.
  Issues are kept few and long-lived.

```
docs/discovery/
  HYPOTHESES.md                 competing strategic hypotheses, status, evidence for/against
  evidence/2027-w03.md          append-only evidence entries of that week
  decisions/DR-0001-<slug>.md   Decision Records
  interviews/I-0001.md          anonymised interview summaries (never raw notes)
```

### 4.1 Evidence entry

```markdown
### E-2027-w03-04
- class: external | internal | simulated | agent-hypothesis
- level: E0 | E1 | E2 | E3
- source: <URL or I-0001 or #issue>          (required unless agent-hypothesis)
- observed: 2027-01-14 · retrieved: 2027-01-16
- who: "PIC designer at a foundry customer (forum post)"   (role, never a name)
- quote: "> verbatim excerpt, ≤ 3 sentences"
- supports / contradicts: H2, O-#812
- note: one line, clearly the scout's reading, not the source's claim
```

### 4.2 Evidence levels

The core rule: **agent reasoning is not evidence.**

| Level | Meaning | Examples |
|---|---|---|
| E0 | agent hypothesis, no external source | "Users probably want a Python API" |
| E1 | public, external, indirect | forum or issue posts by PIC designers, papers, competitor docs and changelogs, job ads |
| E2 | direct observation of a target user | interview, a user's own words to us, real usage of Lunima |
| E3 | behavioural commitment | uses the workflow on their own design, sends files, signs up for a pilot, LOI, payment |

Simulated-persona results from the UX review are `class: simulated`. They are useful for
usability, but they never count as E2 or E3.

**A Decision Record may recommend *pursue* only if at least one supporting entry is E2 or
higher.** E0 and E1 can justify an experiment, not a build.

### 4.3 Hypotheses (`HYPOTHESES.md`)

One section per competing hypothesis. The register is seeded with H0 *status quo: do
nothing / keep current direction*, H1 agent-first, H2 visual-first, H3 GDS reconstruction and
H4 chiplet simulation. Each section has:

- statement,
- who would have to be true for it,
- riskiest assumption,
- evidence for and against (IDs),
- status: `open | testing | supported | weakened | dropped`.

The strategist may add hypotheses. It may not delete one: it can only drop it, with reasoning.

### 4.4 Opportunity (issue, label `discovery-opportunity`)

An observed user problem, not a feature. Body template:

- problem in the user's words,
- affected role or persona,
- evidence IDs with their highest level,
- linked hypotheses,
- what we would have to believe,
- cheapest next test,
- status `open | experimenting | decided | parked`.

At most **3 opportunities are open at the same time**. A new one has to replace or park an old
one. This keeps focus and prevents growing the backlog for its own sake.

### 4.5 Experiment (issue, label `discovery-experiment`)

Pre-registered **before** it starts and not edited afterwards. Results go into a comment.
Required fields:

- assumption under test,
- method (desk research, spike, persona journey, interview, fake door …),
- success and failure criteria with numbers where possible,
- time box,
- token or money budget,
- autonomy class from §6.

### 4.6 Decision Record (`docs/discovery/decisions/DR-NNNN-<slug>.md`)

```markdown
# DR-0003: <question>
- date, status: proposed | accepted | rejected | superseded-by DR-…
- decision: pursue | experiment | park | kill | do-nothing | need-more-evidence
- options compared (≥ 2, one of them "do nothing")
- evidence per option (IDs + levels; agent hypotheses listed separately)
- critic: strongest counter-argument, alternative explanation, what would change our mind
- consequence for delivery: none | agent-task proposals (title + acceptance) for the PO
- revisit when: <trigger or date>
```

A DR without a critic section is invalid. `do-nothing` is a first-class decision and is
counted as a useful outcome.

### 4.7 Labels (new)

| Label | Meaning |
|---|---|
| `discovery` | weekly discovery PR. **Never `agent-pr`.** Only Max merges it. |
| `discovery-opportunity` | opportunity issue |
| `discovery-experiment` | experiment issue |
| `discovery-interview` | interview task for Max (plus `needs-human`) |
| `discovery-approval` | gated action waiting for Max (plus `needs-human`) |

None of them is `agent-task`, so the coder and the org repo discovery ignore them. A spike gets
`agent-task` deliberately and separately (§5.4).

## 5. Roles

### 5.1 Scout: collect, don't conclude

- **Inputs:**
  - the web: competitor and adjacent tools (gdsfactory, KLayout, Luceda IPKISS, Ansys
    Lumerical, Synopsys OptoCompiler, Tidy3D …) with their issue trackers, forums and
    changelogs,
  - arXiv,
  - PIC community discussions,
  - Lunima's own issues incl. `ux-finding` and the UX report,
  - interview summaries,
  - the open experiments and their results.
- **Output:** evidence entries for the week, and nothing else. No opinions, no issues, no
  recommendations.
- **Rules:**
  - every non-E0 entry has a resolvable source and a verbatim quote,
  - deduplicate against earlier weeks,
  - look explicitly for evidence **against** the open hypotheses as well as for them,
  - cap of ~15 entries per week, so quality wins over volume.

### 5.2 Strategist: decide what is worth knowing next

- **Inputs:** `HYPOTHESES.md`, all evidence, open opportunities and experiments, accepted DRs,
  `docs/ROADMAP.md`, `docs/PERSONAS.md`, issue #537, the delivery backlog size (read-only),
  last week's critic output.
- **Tasks:**
  - update the hypothesis statuses,
  - open, update or park opportunities, within the cap of 3,
  - evaluate experiments whose time box ended (this replaces a separate analyst role),
  - pre-register at most 2 new experiments,
  - draft Decision Records,
  - draft interview requests for Max (§6.3),
  - compare workflows against the Q1 outcome.
- **Explicitly allowed:** "no new opportunity, no new experiment, keep going" and "do nothing".

### 5.3 Critic: try to kill it

- **Input:** the strategist's diff for the week (new or changed opportunities, experiments,
  DRs) plus the evidence.
- For each item it answers:
  - the strongest reason this is wrong or not worth it,
  - alternative explanations of the evidence,
  - whether this is already solved elsewhere,
  - which persona would really change behaviour,
  - whether a cheaper test would answer the same question,
  - whether agent hypotheses are being passed off as evidence.
- **Verdict per item:** `stands | weaken | reject`. The verdict and the reasoning are written
  into the DR's critic section or the experiment issue. A `reject` does not delete the item:
  it is shown to Max with both positions.

### 5.4 Reused roles (no new code beyond prompt context)

| Need | Existing role |
|---|---|
| Feasibility spike | **coder**. The strategist files an `agent-task` issue with `Team branch: spike/<slug>` and "throwaway, not for dev" in the body. QA runs as usual. The PO never merges spike PRs because they don't target `dev`. |
| Usability test of a prototype | **UX review** with persona goals written by the strategist (goals file). Result = `simulated` evidence. |
| Analysis of results | **strategist**, in the next weekly run. |
| Prioritising and delivering accepted DRs | **Product Owner**, unchanged authority over capacity and maintenance. |
| Landing page or fake door (phase 3) | **coder** on the existing `lunima.org` site, publishing only after approval. |

## 6. Decision gates

### 6.1 Autonomous (small, reversible, internal)

- desk research and writing evidence entries,
- opening, updating and parking opportunity and experiment issues within the caps,
- drafting DRs and interview requests,
- spikes on `spike/*` branches (never merged into `dev`),
- persona journeys in the UX review.

### 6.2 Needs Max (label `discovery-approval` + `needs-human`)

- **Any external communication:** outreach, posting in forums or social media, emails, surveys,
  contacting a company.
- **Publishing anything:** landing page, fake door in the released app, blog post.
- **Any financial commitment:** ads, paid tools, data sets, recruiting incentives.
- **Architecture, delivery and roadmap:** architecture changes, new major dependencies, public
  API breaks, or a ROADMAP change resulting from a DR.
- **Accepting a DR:** merging the weekly `discovery` PR is the approval. Max can also accept
  some DRs and leave others proposed by commenting; the strategist reads that next week.

An approval request states the action, the cost, the risk, the reversibility and the evidence
behind it. Without an answer nothing happens. The request does not re-escalate every week, it
stays in the weekly report.

### 6.3 Interviews (human in the loop)

1. The strategist files a `discovery-interview` issue. It contains:
   - the target profile,
   - the hypotheses to probe,
   - a script that asks about **past behaviour, not opinions** ("tell me about the last time you
     …", no pitching),
   - a summary template.
2. Max does the recruiting and the interview.
3. Raw notes and recordings **stay private** (local or a private store), because the repo is
   public.
4. Max posts, or asks an agent to write from the notes, an **anonymised summary**: role,
   company type, no names. It is committed as `docs/discovery/interviews/I-NNNN.md`.
5. The scout turns the summary into E2 entries.

## 7. Cost and value tracking

**Ledger:** `state/discovery/ledger.jsonl`, one line per stage run, built from the Claude
`Outcome`:

```json
{"week":"2027-w03","stage":"critic","machine":"…","started":"…","duration_sec":512,
 "tokens":1840000,"cost_usd":3.12,"result":"ok"}
```

On the subscription, `cost_usd` is a notional figure, but it makes runs comparable and the
budget enforceable.

**Budgets** (`[product_discovery]`):

- `max_turns` and `timeout_minutes` per stage,
- `max_cost_usd_per_week`: the run stops before the next stage once it is exceeded, and the
  report says so.

**Weekly report:** one maintained issue "Discovery — weekly" (label `discovery-report`, same
mechanism as `ux/report.rs`). It shows the week's result, items waiting for Max, and this table:

| Metric | Source |
|---|---|
| discovery cost (week / quarter) | ledger |
| useful decisions: DRs accepted, incl. `do-nothing` and `kill` | merged DR files, status field |
| rejected DRs, and why | DR status + Max's PR comments |
| human review time | `review-minutes: N` line that Max adds to the merge or PR comment (self-reported, kept optional). The PR open→merge time is shown only as a rough proxy. |
| external validation | count of E2 and E3 entries, interviews done (vs. Q1 targets 3–5 / 2 / 1) |
| hypothesis movement | status changes in `HYPOTHESES.md` |

**Explicitly not tracked as success:** number of issues, opportunities or evidence entries.

## 8. Configuration (sketch)

```toml
[roles]
product_discovery = false        # off by default

[product_discovery]
repo = ""                        # "" = product_owner.repo
weekday = "mon"
hour = 6                         # window start, local; window lasts 6 h
model = ""                       # "" = models.product_owner
max_turns = 120                  # per stage
timeout_minutes = 45             # per stage
max_cost_usd_per_week = 25.0
max_open_opportunities = 3
max_new_experiments = 2
```

The Control Center gets the same kind of entries as `ux_review` in
`control-center/src/core/settings_model.rs`.

## 9. Phased plan

Every phase has an exit criterion. The first one checks whether discovery itself is worth
building, and the system is allowed to recommend doing nothing about itself too.

### Phase 0: manual dry run (no code, 2 weeks)

- Create the labels and seed `docs/discovery/HYPOTHESES.md` (H0–H4).
- Write the three prompts as files and run them by hand once a week (`claude -p`) against
  the Lunima clone. Record cost and Max's review time by hand.
- **Exit:**
  - Max judges at least one DR or experiment proposal useful,
  - at least one interview request is concrete enough to act on,
  - cost per week is acceptable.
- If not: rewrite the prompts or stop. No Rust is written before this.

### Phase 1: the `product-discovery` role (≈ 1 PR)

- The role: weekly window and lock modelled on `ux/plan.rs`, three sequential `Job`s, built-in
  prompts, ledger, cost cap.
- The weekly branch and PR (`discovery`, never `agent-pr`) and the weekly report issue
  (reusing the `ux/report.rs` publish pattern).
- `Forge::issues(label, state)`, the config section and Control Center fields.
- Tests: the existing `FakeForge` / `FakeLlm` harness, at integration level (one full weekly run
  against fakes: lock, three stages, PR, report, ledger, cost cap stops a stage).
- **Exit:** 2 consecutive automated weeks with no manual fix-up, and the ledger and report
  are correct.

### Phase 2: hand-over to delivery

- A PO prompt section "Discovery decisions" lists the accepted DRs with delivery consequences
  (fenced, like the UX report).
- Remove the PMF-lens duty and the strategic kill-review half from `po-owner.md`. ROADMAP PRs
  must cite a DR.
- Spikes through the coder with `Team branch: spike/*`, and persona goals into the UX review.
- **Exit:** at least one accepted DR turned into delivered work by the PO, or deliberately not,
  with reasoning.

### Phase 3: external experiments (only if phases 0–2 pay off)

- Landing pages and fake doors on `lunima.org`, an in-app fake door with an honest "coming
  soon", a waitlist form and privacy-friendly analytics. Every publish goes through §6.2.
- Outreach drafts that Max sends personally. No agent posts under anyone's identity.
- Split out `lunima-discovery` (private) if interview material, competitive analysis or
  strategy becomes too sensitive for the public Lunima repo.

## 10. Open questions for Max

1. Is it acceptable that hypotheses and competitor analysis are public (Lunima repo)? If not,
   phase 1 should already write the files to a private repo, which changes nothing else in this
   design.
2. Weekly window: which weekday and hour? Discovery should finish before Max's weekly review.
3. `max_cost_usd_per_week`: what is the starting value?
4. Who counts as a "real PIC designer" for the Q1 target, e.g. Akhetonics colleagues or only
   people outside the company?
