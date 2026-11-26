Implement issue #{issue_number}: {issue_title}
{branch_note}

## Read the repository's guide first
Read `AGENTS.md` and `CLAUDE.md` (whichever exist) before anything else. They say how this
repository is built and tested, its architecture rules, and what must never be touched (for example
real hardware). They override the general advice below.

## Issue type
- **Investigate / test / verify** → tests only, no UI
- **A user-facing feature** → the full stack the repository's guide describes, with tests
- **A bug** → fix it and add a regression test that fails without the fix

## Finding your way in the code
This worktree has a CodeGraph index. Find your way with the codegraph tools, not with grep/sed
in the shell: every shell call re-reads your whole context, one explore call replaces a dozen.
- `codegraph_explore` with the symbol or file names you know: the relevant source of many files in one call
- `codegraph_search` (where is X defined), `codegraph_callers` / `codegraph_callees` (call paths),
  `codegraph_impact` (before changing a widely used symbol), `codegraph_node` (one symbol's source)
Read big files by section, never whole. If a codegraph tool reports "not initialized", fall back to grep.

## Build and test
- Use the build and test commands from the repository's guide. Run the tests of the area you
  changed, not the whole suite: CI runs everything on the PR.
- Give every run a timeout. Never rerun the same failing test more than twice without changing
  something. A failure that also happens on the base branch is not yours: name it in the PR
  summary instead of chasing it.
- New behaviour needs a test that fails without your change.

## Issue #{issue_number}: {issue_title}

{issue_body}

## Your task
1. Read the guide, then locate the code with CodeGraph and find a similar feature to follow.
2. Implement the change, small and focused; touch only what the issue needs.
3. Build and run the relevant tests; fix what fails.
4. Before the final commit, review your own diff: correctness, edge cases, and whether the
   tests assert real behaviour.
5. If the issue lacks a number or decision you need (a threshold, a limit, a timing), say so in
   the PR summary instead of inventing one.
