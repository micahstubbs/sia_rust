# Review Findings Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the seven bugs documented in `docs/reviews/codebase-review-2026-06-06.md`, publish a PR, and merge it after CI is green.

**Architecture:** Keep the Rust and Python behavior aligned where both implementations expose the same workflow. Centralize subprocess timeout behavior enough to avoid pipe backpressure and child leaks, and reuse existing size-limited file helpers for dashboard artifact reads.

**Tech Stack:** Rust crate (`cargo test --all-targets --features llm`), Python package (`python -m compileall`, existing pytest-compatible tests), beads (`br`), GitHub CLI (`gh`).

### Task 1: Issue Tracking and Workflow Documentation

**Files:**
- Modify: `.beads/issues.jsonl`
- Modify: `AGENTS.md`
- Verify: `CLAUDE.md`

**Steps:**
1. Create seven beads and seven companion GitHub issues for findings H1, H2, M1, M2, M3, M4, and L1.
2. Link each bead to its GitHub issue with `br update <id> --external-ref <url>`.
3. Add the autonomous review-fix workflow to `AGENTS.md`.
4. Verify `CLAUDE.md` includes `@AGENTS.md`.

### Task 2: Rust Subprocess Timeout Safety

**Files:**
- Modify: `src/orchestrator.rs`
- Modify: `src/llm/tools.rs`
- Test: `tests/orchestrator.rs`
- Test: `src/llm/tools.rs`

**Steps:**
1. Add failing tests for evaluator timeout child cleanup and large-output Bash completion.
2. Refactor evaluator execution so the timeout path kills/reaps the process.
3. Refactor native Bash execution so stdout/stderr are drained while the process runs.
4. Verify targeted tests pass, then run Rust formatting.

### Task 3: Target-Agent Wall-Clock Timeout

**Files:**
- Modify: `src/orchestrator.rs`
- Modify: `sia/orchestrator.py`
- Test: `tests/orchestrator.rs`
- Test: `tests/test_generation_loop.py` or `tests/test_sandbox.py`

**Steps:**
1. Add failing tests for target-agent timeout handling in Rust and Python.
2. Enforce `docker_timeout` / `DOCKER_TIMEOUT` for target-agent execution.
3. Preserve partial stdout logs and return a clear timeout error.
4. Verify targeted Rust and Python tests pass.

### Task 4: Rust Setup Error Propagation

**Files:**
- Modify: `src/run.rs`
- Modify: `src/orchestrator.rs`
- Test: `tests/generation_loop.rs`

**Steps:**
1. Add failing tests proving generated requirements install failures and reference-copy failures stop the Rust run path.
2. Propagate `install_requirements` errors from `run_generation_with`.
3. Propagate `copy_reference_into` errors in initial and feedback generation setup.
4. Verify targeted tests pass.

### Task 5: Web Artifact Size Caps

**Files:**
- Modify: `src/web/runs.rs`
- Modify: `sia/web/runs.py`
- Test: `tests/web_data.rs`
- Test: `tests/test_web.py`

**Steps:**
1. Add failing tests for oversized text artifacts and telemetry/trajectory JSON.
2. Replace unbounded web reads with size-limited helpers or equivalent capped reads.
3. Return omitted/not-found behavior for oversized artifacts rather than reading them fully.
4. Verify targeted Rust and Python web tests pass.

### Task 6: Missing-Accuracy Context Summary

**Files:**
- Modify: `src/context_manager.rs`
- Modify: `sia/context_manager.py`
- Test: `tests/context_manager.rs`
- Test: `tests/test_context_manager.py`

**Steps:**
1. Add failing tests for finalization with no `accuracy` metric.
2. Render `Best Performance: N/A` when no best generation exists.
3. Verify targeted Rust and Python context tests pass.

### Task 7: Final Verification, PR, CI, and Merge

**Steps:**
1. Run `cargo fmt --all -- --check`.
2. Run `cargo clippy --all-targets --features llm -- -D warnings`.
3. Run `env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u GOOGLE_API_KEY -u GEMINI_API_KEY -u TAVILY_API_KEY cargo test --all-targets --features llm`.
4. Run `python -m compileall -q sia tests` and the relevant Python test runner if available.
5. Commit scoped changes, push `codex/review-findings-fixes`, open a PR with `Fixes #120` through `Fixes #126`, watch CI, fix failures, and merge after CI is green.

## Issue Tracking

- H1: `sia_rust-eval-timeout-kills-child-i6c` / GitHub #120
- H2: `sia_rust-native-bash-drains-output-of7` / GitHub #121
- M1: `sia_rust-target-agent-wall-clock-timeout-06k` / GitHub #122
- M2: `sia_rust-requirements-install-errors-r3i` / GitHub #123
- M3: `sia_rust-reference-copy-errors-txw` / GitHub #124
- M4: `sia_rust-web-artifact-size-caps-ff7` / GitHub #125
- L1: `sia_rust-context-best-performance-na-ogt` / GitHub #126
