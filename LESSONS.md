# LESSONS.md

Append-only debugging and process lessons for this project.

## 2026-06-06T20:30 - Install Rust CLIs with the repo lockfile and intended toolchain

**Problem**: Reinstalling the local `beads_rust` binary with plain `cargo install --path ...` failed even though a working older binary was already installed.

**Root Cause**: The first install used stable Rust, but the `fsqlite` dependency graph requires nightly-only features. Retrying with nightly but without `--locked` allowed Cargo to float transitive dependencies; `fsqlite-core 0.1.7` then compiled against an incompatible `asupersync 0.3.2` API instead of the lockfile-pinned `asupersync 0.3.1`.

**Lesson**: For local Rust CLI installs from a checked-out repository, verify the active binary, source version, toolchain, and lockfile expectations before reinstalling. Use the repository's lockfile when the checkout has one.

**Solution**: Reinstalled with `cargo +nightly install --locked --path <checkout> --root /home/m/.local --force`, then verified `command -v`, `--version`, workspace health, and smoke-test behavior.

**Prevention**: Prefer `cargo +<toolchain> install --locked --path ... --root ... --force` for project-local CLI installs. If a plain install fails in a dependency, compare the lockfile-pinned versions against the floated versions before changing source code.

## 2026-06-06T21:15 - Scripting `br` issue creation: `--json` shape and the `--silent` trap

**Problem**: A bulk script to mirror GitHub issues into beads failed twice with `jq: parse error: Invalid numeric literal` and `jq: error: Cannot index array with string "external_ref"` — but only after `br create` had already created the issue, leaving a stray TEST probe and a half-applied run.

**Root Cause**: Two separate `br` output-shape facts that aren't obvious from `--help`:
1. `br create --json --silent` — `--silent` ("output only issue ID") *overrides* `--json` and prints a bare `sia_rust-xxx` string. Piping that into `jq` parses it as a (non-)numeric literal and dies. The two flags are mutually exclusive in practice.
2. `br list --json` does **not** return a bare array. It returns an object: `{issues:[...], total, limit, offset, has_more}`. The correct jq path is `.issues[].external_ref`, not `.[].external_ref`. Per-issue fields (including `external_ref`, `labels`) live on `.issues[]`.

**Lesson**: When scripting `br`, parse with `br create --json | jq -r '.id'` (no `--silent`) and `br list --json | jq -r '.issues[]'`. Probe the JSON shape once (`br <cmd> --json | jq 'type, keys'`) before writing a loop — `br create` mutates on every call, so a parse bug *after* creation orphans real records.

**Solution**: Dropped `--silent`, fixed the jq path to `.issues[].external_ref`, made the script idempotent by skipping GH numbers already present as an `external_ref` (`https://.../issues/N`), and deleted the stray probe with `br delete <id> --reason ...` (creates a tombstone; the JSONL keeps a deleted record, so `wc -l` exceeds the live issue count by the tombstone count).

**Prevention**: Use `--external-ref` as the idempotency key when mirroring an external tracker into beads — it survives re-runs and lets the script self-skip. Never combine `--json` with `--silent`. In a shared/multi-agent beads repo, expect the live DB to diverge from your seed (another agent had grown the tracker from 18 to 23 issues); the `external-ref` guard prevents duplicate creation regardless.

## 2026-06-06T21:37 - Verify tracker claims against the beads database, not issue prose

**Problem**: Several open GitHub issues claimed they had companion bead IDs, but the local beads database did not actually contain those IDs. A naive grooming pass could have treated the issues as already mirrored and left GitHub/beads parity broken.

**Root Cause**: GitHub issue bodies are descriptive prose and can be stale, speculative, or produced by another agent before a local `br sync`/import completed. The authoritative local state is the beads database plus exported JSONL, especially the `external_ref` field.

**Lesson**: During GitHub/beads grooming, compare open GitHub issue numbers against open beads `external_ref` values, then verify any named bead with `br show` before trusting it. Use issue prose as a clue, not as the source of truth.

**Solution**: Created the missing beads for GitHub #120-#126, parented them under the umbrella, added GitHub comments with the actual local bead IDs, raised the H1/H2 findings to P1, and verified parity with a `comm` comparison of GitHub numbers and beads `external_ref` numbers.

**Prevention**: Make issue grooming idempotent around `external_ref`. For every open GitHub issue, require exactly one open bead with a matching `external_ref` unless the GitHub issue is intentionally closed or superseded. After syncing, run a parity check before reporting completion.
