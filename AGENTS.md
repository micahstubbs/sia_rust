# AGENTS.md

Project instructions for coding agents working in this repository.

## Issue Tracking

- Use `br` (`beads_rust`) for issue tracking. Do not run `bd`.
- Every user prompt, command, or requested work item must be represented by one or more beads issues before substantial work begins.
- Do this automatically. Do not wait for the user to invoke a beads creation skill or explicitly ask for tracking.
- Reuse an existing issue when it clearly covers the work. Otherwise create a new issue with `br create --title "..." --type task --priority <n>`.
- For compound requests, create separate issues or an epic plus sub-issues so each distinct workstream can be tracked.
- Claim active work with `br update <id> --status in_progress --assignee <agent>`.
- Close completed issues with `br close <id> --reason "..."` after implementation and verification.
- Run `br sync --flush-only` after issue changes so `.beads/issues.jsonl` stays current.

## Autonomous Review-Fix Workflow

When a codebase review produces actionable findings and the user asks for autonomous follow-through:

1. Create one bead for each distinct finding before implementation. Use stable, descriptive slugs and include the review report path plus acceptance criteria in the description.
2. Create a companion GitHub issue for each bead when the repository has a GitHub remote. Put the bead ID in the GitHub issue body, then update the bead `external_ref` with the GitHub issue URL.
3. Work in an isolated branch or git worktree when the main checkout has unrelated changes. Do not stage or rewrite unrelated user work.
4. Use test-driven development for each bug fix: add the regression test, verify it fails for the expected reason, implement the minimal fix, then verify it passes.
5. Keep commits scoped and reference bead IDs plus GitHub issue numbers in commit messages or PR bodies. Use `Fixes #...` only when the PR is intended to close that GitHub issue.
6. Push a PR after local verification. Watch GitHub Actions with `gh pr checks` or the GitHub CI skill, fix failures on the branch, and merge only after required CI is green.
7. After merge, close the corresponding beads with the PR or merge SHA as the reason, run `br sync --flush-only`, and ensure the issue export is committed or otherwise synchronized as appropriate for the branch.
