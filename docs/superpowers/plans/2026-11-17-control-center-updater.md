# Control Center updater implementation plan

> Execute the approved design using parallel independent implementation tasks and a final independent review.

**Goal:** Automatically stage releases for Windows and Ubuntu, offer an update button, install on next start, and drain running agent work before replacing both binaries.
**Architecture:** A background release worker downloads exact platform packages and SHA-256 metadata from the fixed GitHub repository. Persist only verified pending updates. A dedicated installation helper survives UI exit, runs MSI or Debian installation (with elevation), then relaunches the previous UI/daemon configuration. Agent drain requests prevent new work and acknowledge only after current work finishes.
**Tech stack:** Rust/egui, gh CLI, SHA-256, PowerShell/MSI and shell/pkexec/dpkg.
**Spec:** User-approved design in this conversation, 2026-11-17.

- [x] Implement drain request/ack lifecycle including chat and regression tests. Requests must never interrupt running cycles; stale requests must not freeze the daemon.
- [x] Implement installer helper, quoting, exact supported installation paths, cancellation/failure handling, restart behavior, tests.
- [x] Implement version/asset policy, verified download cache, state machine, startup consumption, visible top update button and retry.
- [x] Use GitHub release SHA-256 digests and stamp correct release versions in CI; document installation modes and privileges.
- [x] Run focused tests, workspace tests and release build; independently review and resolve findings.

Review focus: cancellation of elevation must preserve the pending update and restart the prior agent; tampered or partial download must never install; unsaved settings must flush; shutdown must drain every work source; development binaries must not overwrite a system install; no automated release publication or running-task interruption during development.

Validation: Windows Control Center 93 passed (one explicit network test ignored by default; that test passed separately against the public MSI). Core 82 passed, six existing ignored. Drain/chat/daemon targeted tests 24 passed. Ubuntu updater 17 passed, one explicit network test ignored. Native installer tests execute stubbed package managers and verify actual Rust-compatible process locks; real elevated MSI/dpkg installation has not been performed. Release Rust binaries and Windows MSI built. Independent review findings fixed. A broad workspace attempt was limited by the host memory guard; no claim of a passing full workspace suite.
