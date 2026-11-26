A previous QA pass on PR #{pr_number} (branch
`{branch}`) FAILED. Fix the issues described below and push to the same
branch — QA will rerun automatically.

## Original Issue
**#{issue_number}: {issue_title}**

{issue_body}

## QA Verdict (latest)
{qa_summary}

## QA Failure Details
{qa_details}

## Your Task
1. Read AGENTS.md / CLAUDE.md for project conventions, build and test commands. Locate code
   with `codegraph_explore`, not grep/sed (grep only literal text).
   If a codegraph tool reports "not initialized", use grep instead: do not ask anyone and do not build the index yourself.
2. Reproduce the failure locally if it is a build/test failure, with the guide's commands and
   only the failing tests.
3. Address EVERY BLOCKING finding above. NIT findings are optional.
4. Add or update tests so the failure can't reappear silently.
5. Commit and push to branch `{branch}`. Do NOT open a new PR — the
   existing one (#{pr_number}) will pick the new commits up.

Do not refactor unrelated code. Keep the diff focused on the QA failure.
