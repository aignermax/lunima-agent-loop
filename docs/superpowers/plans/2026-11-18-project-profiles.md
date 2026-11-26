# Project profiles implementation plan

> Execution: native in this existing checkout, continuing the approved changes. Use executing-plans and one final independent review.

**Goal:** Project-scoped models, provider restrictions, label rules and budgets.
**Architecture:** Optional typed profiles preserve old configurations. A central resolver
selects the effective backend/model for every role and validates allowlists before execution.
**Tech stack:** Rust, serde/TOML, egui, existing CLI runners.
**Spec:** ../specs/2026-11-18-project-profiles-design.md

## Constraints and review focus

- Preserve existing dirty work; no stashing, unrelated commits, or live config/model changes.
- Test forbidden label/fallback providers, inherited settings, explicit CLI-default model,
  mixed-role execution, sandbox env retention and concurrent provider quotas.
- Fallback must happen before execution only. API keys stay in global endpoint definitions.
- Budgets must affect role work, not merely UI; Codex turn limits remain unsupported.

## Tasks

- [x] 1. Add failing profile resolution/roundtrip/security/budget tests; observe failures.
- [x] 2. Add typed profiles, partial budget inheritance, validation and central resolution.
  Interfaces: ProjectProfile, ModelSpec, ProjectBudgets, SessionRole, resolve/resolve_fallback.
- [x] 3. Attach explicit role/labels to jobs, enforce decisions and safe pre-execution fallback;
  apply project budgets to role contexts and Codex effort to CLI args. Test integration.
- [x] 4. Add project editors for provider allowlist, standard/role/label/fallback model specs,
  inherited budgets and effective previews; clarify global settings. Verify save validation.
- [x] 5. Run workspace tests, independent code review, release build and app-native screenshots.
  Report result and remaining restart requirement without touching live model selections.

## Progress

- Initial: CodeGraph unavailable (not initialized); user asked asynchronously about indexing.
- Ruling: Continue in the existing checkout because approved work depends on its extensive
  uncommitted Codex/updater changes; do not isolate from or discard that baseline.

- Verification: `cargo test --workspace` → 390 passed, 7 ignored, 0 failed
  (target/profiles-final-tests.log). Existing compiler warnings retained.
- Independent review: Feedback preflight and waiting/admission findings fixed; reviewer
  confirmed no remaining important findings in the focused follow-up.
- `cargo build --release --workspace` → exit 0; binaries updated in target/release.
- Native screenshot reviewed: target/profiles-preview/projects.png, synthetic home only.
- No live configuration, running daemon, or desktop session was changed; restart required.
- Ruling: treat model admission failures as Unavailable, preserving claims/task budgets;
  only explicit pre-execution fallback is allowed. PO/discovery limits inherit per field.
