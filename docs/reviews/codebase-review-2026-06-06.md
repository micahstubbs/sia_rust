# Codebase Review - SIA Rust

**Date:** 2026-06-06
**Scope:** Full repository review covering the Rust crate, Python package mirror, run orchestration, native LLM tooling, dashboard data layer, tests, and CI configuration.
**Reviewer:** Codex

## Summary

I found no critical vulnerability requiring an immediate stop-ship, but I found seven bugs worth fixing:

- **High:** Evaluation timeouts in the Rust orchestrator report timeout without killing the evaluator process.
- **High:** Native LLM shell commands can deadlock or false-timeout on large stdout/stderr.
- **Medium:** Target-agent execution has no wall-clock timeout even though timeout configuration exists.
- **Medium:** Rust generation dependency installation failures are discarded.
- **Medium:** Rust reference-copy failures are discarded before meta/feedback agent runs.
- **Medium:** The web visualizer reads run artifacts and JSON files without size caps.
- **Low:** Context summaries print `-inf%` when no accuracy metric exists.

## High Severity

### H1. Rust evaluation timeout does not terminate the evaluator process

**Files:** `src/orchestrator.rs:225`, `src/orchestrator.rs:240`, `src/orchestrator.rs:245`, `src/orchestrator.rs:252`

`run_command_with_timeout` spawns `evaluate.py`, moves the `Child` into a worker thread, then waits on a channel with `recv_timeout`. On timeout it returns `EvalOutcome::TimedOut`, but the main thread no longer owns the child handle and cannot kill it.

The likely effect is that a timed-out evaluator keeps running after the orchestrator has already recorded a timeout. That stray process can continue consuming CPU/memory and can still write `results.json`, logs, or other generation artifacts after the run has advanced. This is especially risky for long evaluation scripts and repeated generations.

The Python implementation uses `subprocess.run(..., timeout=cfg.EVAL_TIMEOUT)` at `sia/orchestrator.py:216`, which kills/waits the child before raising `TimeoutExpired`, so this is a Rust-port lifecycle regression.

**Recommended fix:** Keep kill ownership in the timeout path. Use a wait loop that retains the `Child` handle, or use a process group/container timeout and kill the group on expiry. Because stdout/stderr are piped, combine this with concurrent pipe draining so the fix does not reintroduce pipe backpressure.

### H2. Native LLM `Bash` tool can deadlock or false-timeout on large output

**Files:** `src/llm/tools.rs:80`, `src/llm/tools.rs:86`, `src/llm/tools.rs:95`, `src/llm/tools.rs:117`

The native shell tool pipes stdout and stderr but does not read either stream until after `child.try_wait()` observes process exit. If the command writes more than the OS pipe buffer, the child blocks while writing, the parent keeps polling, and the command eventually appears to time out even though it is only blocked by the parent not draining output.

This affects native Claude/OpenHands/Pydantic-AI tool calls that run verbose commands such as builds, tests, or data inspections. It can discard useful diagnostics and cause unnecessary tool failures. The project already fixed this pattern for target-agent execution in `src/orchestrator.rs:323` through `src/orchestrator.rs:342`, and `tests/stream_to_log.rs:10` covers heavy stderr for that path, but there is no analogous coverage for `tools::bash`.

**Recommended fix:** Refactor `tools::bash` to drain stdout and stderr concurrently while enforcing the deadline, or use a robust subprocess helper with timeout plus pipe draining. Add a regression test that writes substantially more than a pipe buffer to stderr/stdout and completes before the shell timeout.

## Medium Severity

### M1. Target-agent execution has no wall-clock timeout

**Files:** `src/config.rs:73`, `src/config.rs:80`, `src/orchestrator.rs:270`, `src/orchestrator.rs:323`, `sia/config.py:40`, `sia/config.py:48`, `sia/orchestrator.py:291`, `sia/orchestrator.py:324`

Both Rust and Python define target/sandbox-related timeout settings (`docker_timeout` / `DOCKER_TIMEOUT`), but target-agent execution never applies a wall-clock deadline. The Rust target path calls `stream_to_log` and waits until the child exits; the Python mirror streams from `Popen` and then calls `process.wait()`.

A generated target agent that hangs, calls `input()`, waits on a dead network path, or loops forever can hang the whole run indefinitely. Docker mode adds network, memory, and CPU limits, but it still does not bound elapsed time.

**Recommended fix:** Apply a wall-clock timeout to target-agent execution in both sandbox modes. For Docker, prefer killing the container/process group on timeout and returning a clear execution failure with the partial log retained. Add tests for a long-running target agent under both `sandbox=none` and Docker command construction.

### M2. Rust ignores dependency installation failures for generated requirements

**Files:** `src/orchestrator.rs:662`, `src/orchestrator.rs:665`, `src/run_setup.rs:116`, `sia/orchestrator.py:632`, `sia/orchestrator.py:636`

When a generation writes `requirements.txt`, Rust calls `install_requirements` but discards the result with `let _ = ...`. `install_requirements` does return `SiaResult<()>` and reports non-zero installers, but `run_generation_with` ignores that signal and proceeds to run `target_agent.py`.

This turns dependency setup failures into later import failures or misleading feedback. The Python mirror calls `install_requirements(...)` directly, and `sia/run_setup.py:104` uses `subprocess.run(..., check=True)`, so installer failures abort that generation path.

**Recommended fix:** Propagate `install_requirements` errors from `run_generation_with`, or record an explicit setup-failed generation state and skip target/eval/feedback. Add a test with a mock or invalid requirements file that verifies the run does not continue silently.

### M3. Rust ignores reference-copy failures before meta and feedback agents

**Files:** `src/run.rs:116`, `src/run.rs:120`, `src/orchestrator.rs:813`, `src/orchestrator.rs:815`, `sia/orchestrator.py:581`, `sia/orchestrator.py:586`, `sia/orchestrator.py:809`

Rust copies reference helper files into the initial meta-agent working directory and each feedback generation, but both call sites discard copy errors. Python performs the same operation without swallowing exceptions.

If a directory reference contains unreadable helper files, a disappearing symlink target, or a failing requirements copy, the agent prompt can still be generated and executed with missing context/dependencies. The resulting failure will look like an agent mistake rather than a setup failure.

**Recommended fix:** Propagate `copy_reference_into` errors with context that identifies the destination generation directory. Add tests that simulate a copy failure and assert that meta/feedback execution does not proceed.

### M4. Web visualizer reads artifacts and JSON without size caps

**Files:** `src/web/runs.rs:106`, `src/web/runs.rs:111`, `src/web/runs.rs:316`, `src/web/runs.rs:328`, `src/web/runs.rs:450`, `src/web/runs.rs:619`, `src/web/runs.rs:630`, `sia/web/runs.py:125`, `sia/web/runs.py:133`, `sia/web/runs.py:342`, `sia/web/runs.py:354`, `sia/web/runs.py:363`

The dashboard data layer reads `context.md`, `profiles.json`, text artifacts, trajectories, telemetry, and metrics JSON with unbounded `read_to_string`, `std::fs::read`, `json.load`, or `Path.read_text`. The project already has size-limited helpers in `src/io_utils.rs:21` and `sia/io_utils.py:34`, and the orchestrator/context paths use those helpers in several places.

The dashboard starts automatically for normal runs unless disabled. A generated agent or evaluator can produce very large logs/artifacts/telemetry files under `runs/`, and a later API request can read the entire file into memory. That can hang the local dashboard or cause avoidable memory pressure.

**Recommended fix:** Use the safe read/load helpers in the web data layer with explicit caps for text artifacts, trajectories, telemetry, and run summaries. Return a clear omitted/too-large response instead of loading the file. Add Rust and Python web tests with oversized artifact and telemetry files.

## Low Severity

### L1. Context summary prints `-inf%` when accuracy is unavailable

**Files:** `src/context_manager.rs:180`, `src/context_manager.rs:181`, `src/context_manager.rs:210`, `src/context_manager.rs:214`, `sia/context_manager.py:275`, `sia/context_manager.py:277`, `sia/context_manager.py:307`, `sia/context_manager.py:311`

Both context managers initialize `best_metric` to negative infinity, only update it when an `accuracy` metric exists, and always format it in the final summary. A run whose generations produce no accuracy metric will render:

```text
Best Performance: Generation N/A (-inf% accuracy)
```

That is confusing run metadata and can mislead downstream agents that read `context.md`.

**Recommended fix:** Render `Best Performance: N/A` when no accuracy metric exists, or include the best metric only when `best_gen` is present. Add a no-metrics finalization test in both Rust and Python.

## Checks Run

- `cargo fmt --all -- --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo clippy --all-targets --features llm -- -D warnings` passed.
- `cargo test --all-targets --features llm` passed, including the Rust unit and integration suites.
- `python -m compileall -q sia tests` passed.

I also ran a local static review pass over subprocess handling, unbounded file reads, reference copying, artifact path resolution, panic-prone Rust call sites, and Python/Rust parity. A parallel subagent review attempt timed out before returning usable findings, so the findings above are from the local review pass and command verification.

## Positive Observations

- The web data layer already validates run and generation names through resolver helpers, and traversal tests cover those paths.
- The target-agent log streaming path in Rust intentionally drains stdout and stderr concurrently, which is the right pattern to reuse for native shell tooling.
- Context assembly and feedback prompt construction already apply size limits in several high-risk places.
- CI covers Rust formatting, clippy, tests, parity checks, Python tests, and packaging metadata.
