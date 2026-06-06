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
