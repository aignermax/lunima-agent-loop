A human reviewer left feedback on PR #{pr_number}
(branch `{branch}`, title: {pr_title}). Implement what they asked for and
push to the same branch.

## Reviewer Feedback (verbatim)
{comment_body}

## Your Task
1. Read AGENTS.md / CLAUDE.md for project conventions, build and test commands. Locate code
   with the CodeGraph tools (`codegraph_explore`, `codegraph_search`), not grep/sed.
2. Implement the requested change. Stay focused — only what the feedback
   asks for (plus tests). Do not refactor unrelated code.
3. Build and run the tests of the area you changed, with the commands from the guide.
4. **If the change affects UI:** re-render the visual walkthrough so the
   reviewer sees the new state without checking out. Using the headless
   screenshot harness (see `UnitTests/UI/UiScreenshotTests.cs` for the
   Avalonia + Skia pattern), save step-ordered PNGs to
   `artifacts/ui-screenshots/issue-{issue_number}/` (e.g. `01-initial.png`,
   `02-after-click.png`) plus a `manifest.json` array of
   `{{"file": "...", "caption": "..."}}` entries (captions: ONE short
   sentence each). They are embedded into a reply comment automatically.
5. Commit to branch `{branch}`. Do NOT open a new PR.

End your final message with EXACTLY this block (parsed by tooling; 3-6
short bullet lines describing what you changed and why):

=== FEEDBACK REPORT ===
- <what changed>
- <what changed>
=== END ===
