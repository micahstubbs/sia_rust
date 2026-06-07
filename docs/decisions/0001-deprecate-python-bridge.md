# ADR-0001: Deprecate the Python bridge in favor of native Rust

- **Status:** Accepted (2026-06)
- **Issue:** #137 · **Related:** #138, #139, #140, #141, umbrella #34

## Context

`sia_rust` began as a byte-for-byte port of the Python `sia` framework, with several
runtime concerns deliberately delegated back to Python during the port:

- **Target-agent execution** runs `python target_agent.py` inside a per-generation
  `venv` (the `evaluate.py` contract), created/managed via `uv`/`pip` subprocesses.
- **Dataset/task bootstrap** (e.g. MLE-Bench) is done by Python scripts
  (`sia/prepare_mlebench_dataset.py`).
- The meta/feedback **LLM runners** were originally an integration boundary — now
  closed natively on `rig-core` behind `--features llm` (epic #38, #50/#51).

This "Python bridge" keeps a Python toolchain on the critical path for a real
`sia run`, which complicates packaging, reproducibility, sandboxing, and the
self-contained "one binary" story the project is moving toward.

## Decision

**We are going native-Rust and will deprecate, then remove, the Python bridge** for
the core self-improvement loop. New work targets a Rust-native execution and task
pipeline; the Python paths remain supported during a clearly-scoped transition.

### What goes native (tracked separately)
- **#138** — native target-agent execution strategy (replace the Python venv
  subprocess bridge). A `TargetExecutor` seam lands first, with the current
  Python-subprocess path as the default and a native path added incrementally.
- **#139** — native weight updates (`WeightUpdater` trait + `StubWeightUpdater`
  today; `CandleLoRAWeightUpdater` behind the `weight-updates` feature). ✅ landed.
- **#140** — native OS-level sandboxing (Landlock FS layer landed; seccomp/WASI
  next), replacing reliance on Docker/Python isolation.
- **#141** — native MLE-Bench / custom-task bootstrap (reduce/eliminate Python
  setup scripts; native task discovery + a clean one-command path).

### What intentionally stays (for now)
- **Target agents themselves are Python by nature** — they are LLM-authored Python
  ML solutions (sklearn/pandas/etc.). "Native execution" means a Rust-native
  *harness/sandbox* around them, **not** rewriting the agents in Rust.
- **`evaluate.py`** remains supported so existing tasks keep working; it is
  **deprecated for new tasks**, which should target the native task/evaluate
  contract as it lands (#141).
- The Python **reference package** (`sia/`) and the differential **parity gate**
  stay in CI as the correctness oracle for the deterministic surfaces.

## Consequences

- **Positive:** a self-contained Rust binary for the core loop; simpler packaging;
  stronger OS-level sandboxing; fewer moving parts on stage/CI; faster startup.
- **Negative / transitional:** two execution paths coexist during migration;
  contributors must keep the parity gate green until the Python reference is
  formally retired (a separate future decision, not in scope here).
- **Non-goal:** removing Python from *task content* or from the parity oracle in
  this ADR. Deleting the bridge entirely is gated on #138/#141 reaching parity.

## Migration guidance

- New tasks: prefer the native task/evaluate path (#141) once available; do not add
  new `evaluate.py`-only tasks unless necessary.
- New execution features: build behind the `TargetExecutor` seam (#138), keeping the
  Python-subprocess path working until the native path is at parity.
- Heavy/native deps (Candle, Landlock, …) stay behind non-default cargo features so
  the default build and CI remain lean and offline-buildable.
