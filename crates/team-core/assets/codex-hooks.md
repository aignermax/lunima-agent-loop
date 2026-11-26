# Codex commit-comment hook and CI merge gate

Parent integration: register `codex_hooks` in `lib.rs`, then pass every value from
`config_overrides() -> Vec<String>` as a separate `-c` argument on unrestricted
sessions (loop and chat). Do not pass `--dangerously-bypass-hook-trust`. The returned trust
override approves the two embedded hook definitions separately. Run with
`--ignore-user-config` and plugins disabled, as the parent runner does, so user
trust does not enable additional hooks. Administrator-managed hooks remain policy.

Requires Node.js and Codex 0.169.0. The compressed asset keeps the command below
Windows' 8191-character cmd limit. No installation or global edits are performed.
Set `LUNIMA_CODEX_EXECUTABLE` to the native CLI executable and `LUNIMA_CODEX_MODEL`
to the parent model. The hook prefers the event's model when supplied.
Set `LUNIMA_CODEX_AUTH_STORE` to the parent's credential store: `file`, `keyring`,
`auto`, or `ephemeral`. The reviewer passes it as `cli_auth_credentials_store`,
defaults to `file` when absent, and rejects invalid values without echoing them.

API mode uses inherited `LUNIMA_CODEX_API_BASE_URL` and `LUNIMA_CODEX_API_KEY` with
the parent's Responses-compatible provider configuration. Incomplete API settings
block review instead of falling back to a subscription. Subscription runs must
remove both variables; the parent performs login preflight. No forced-login
configuration is used. Credentials never appear in hook config or CLI arguments.

The command hook filters ordinary `git commit` invocations, including compound
commands, `cd`/`Set-Location`, and `git -C`. It reviews receipt SHAs, or HEAD for
quiet successful commits. Failed commits and dry runs are skipped. Arbitrary shell
aliases, nested `sh -c`/`eval`, dynamically computed directories, and commits made
inside scripts are not interpreted. Multiple repositories in one tool call produce
blocking clarification feedback. This is a workflow check, not a Git enforcement
boundary; PostToolUse cannot undo a commit.

Review uses a separate Codex process and the chosen model, a supplied `git show`
diff, a read-only sandbox, disabled hooks/tools/connectors, structured output, a
240-second child timeout, and a recursion environment guard. Findings instruct the
parent to remove comments and amend; failures are reported as incomplete reviews.

Codex currently skips `agent` and `prompt` hook types. Hook sources merge across
config layers, so an empty CLI hooks table does not erase project hooks:
https://learn.chatgpt.com/docs/hooks

Trust keys and hashes are version-sensitive. Codex rust-v0.169.0 explicitly uses
`C:\` for its Windows synthetic session-flags path, not the current drive:
https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/hooks/src/engine/discovery.rs

After editing the script or hook definition, regenerate with:

```text
node crates/team-core/assets/codex-hooks-build.cjs --write
node --test crates/team-core/assets/codex-comment-hook.test.cjs
node --test crates/team-core/assets/codex-hooks-cli.test.cjs
cargo test -p team-core codex_hooks -- --include-ignored
```

The build helper discovers and verifies the hash using the installed app-server
in a temporary CODEX_HOME. The CLI test uses a local fake Responses endpoint and
temporary CODEX_HOME, simulates commit output without making a commit, and verifies
that the native hook's failure feedback reaches the model. Neither test needs
credentials. The Node-dependent Rust asset check is ignored in ordinary Cargo tests.
Windows execution was tested; Linux uses the same Node implementation and POSIX
process-group cleanup, but was not executed in this environment.

The PreToolUse CI gate ports the global Claude `ci-gate-merge.js` policy for
`gh pr merge` and REST `gh api -X PUT repos/OWNER/REPO/pulls/N/merge`. It invokes
only `gh pr checks --json name,state,bucket`. All checks must pass or be skipped;
pending, failed, cancelled, missing, unknown, or unreadable checks deny the tool.
The original Claude gate's `ask` becomes `deny` in headless runs, with a request
for human review. `--auto` and `--disable-auto` retain the original exemptions.
`LUNIMA_GH_EXECUTABLE` optionally selects a native gh executable. The query budget
is 25 seconds inside a 30-second hook timeout. As with the original gate, arbitrary
shell aliases, scripts, GraphQL merges, and other merge clients are not covered.

Run CI-gate unit tests with `node --test crates/team-core/assets/codex-ci-gate.test.cjs`.
The native CLI tests also verify denial before a simulated merge command executes;
they deliberately use a nonexistent gh path and never invoke a live merge.

The parent handles global memory injection directly: the inspected SessionStart
PowerShell loader reads sorted UTF-8 `~/.claude/memory/*.md` files with source
headings. No PowerShell invocation is needed. Global wmux lifecycle handlers are
UI telemetry and intentionally omitted from headless automation.
