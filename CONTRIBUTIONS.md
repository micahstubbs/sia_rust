# SIA (Rust) — Project Status & Contributions

A clear, skimmable summary of what `sia_rust` is, what is **implemented today**,
and what is **planned**. This doubles as onboarding for new contributors and
judge-facing hackathon documentation.

`sia_rust` is a native **Rust port** of the original Python
[SIA (Self-Improving AI)](https://arxiv.org/abs/2605.27276) framework: a
Meta-Agent writes and improves a Target-Agent, the Target-Agent runs a task, and
a Feedback-Agent analyzes the trajectory to propose the next improvement. The
Rust port preserves the paper's self-improvement semantics while adding native
LLM runners, a web visualizer, safety primitives, and a parity/benchmark
verification stack.

> Every quantitative claim below is sourced to a repository artifact (e.g.
> `benchmarks/REPORT.md`) or the code path that implements it. Work that is not
> yet implemented or evaluated is explicitly marked **Planned** in the
> [Roadmap](#roadmap--planned) section — no invented metrics.

---

## Highlights

- **Module-for-module Rust port** of the Python framework — `src/` mirrors `sia/`
  one-to-one (orchestrator, prompts, context manager, config/profiles/providers,
  run-directory layout, web data layer).
- **~5.8× geomean speedup** of the deterministic core vs. CPython across nine
  byte-identical-fixture operations (Criterion vs. Python `perf_counter`),
  measured in `benchmarks/REPORT.md` (up to 24.7× on prompt building).
- **Native Rust agent runners** on [`rig-core`](https://crates.io/crates/rig-core),
  feature-gated behind `--features llm`: Claude (Anthropic `/v1/messages`
  tool loop), OpenHands-style (OpenAI-compatible), and PydanticAI-style.
- **Differential Python ⇄ Rust parity harness in CI** — byte-for-byte equality
  of prompts, context, and JSON output across an ASCII + CJK + emoji +
  control-char matrix (`scripts/parity_check.py`, gated in
  `.github/workflows/rust.yml`).
- **SIA Studio web visualizer** — an Axum/Tokio dashboard that renders any
  `runs/` directory (runs, generations, trajectories, telemetry, metrics).
- **Standalone Rust eval harness on `dspy-rs`** (the DSRs port of DSPy) — a
  GPQA-style multiple-choice harness that runs fully offline in CI (`evals/`).
- **Capability sandbox** — a pure-`std`, deny-by-default allow-list around
  native tool execution, plus optional Docker confinement for target agents
  (`src/sandbox.rs`, threat model in `SECURITY.md`).
- **Lean default build** — zero LLM/network dependencies unless `--features llm`
  is enabled, so the published crate stays small.

---

## Project Foundation (PM / Research / Planning)

The repo is built on a deliberate project-management and research foundation, not
just code:

- **Detailed issue backlog across the three hackathon tracks** — Framework
  Enhancement, Applied, and Research — with grooming notes, labels, and a beads
  mirror (`.beads/`) for local dependency-aware triage. See `AGENTS.md` for the
  issue-tracking and autonomous review-fix workflow.
- **Academic paper skeleton** — an iterative, preprint-style system/experience
  report at `docs/paper/sia_rust_preprint.md` (issue #70). Its accuracy policy:
  describe only what is implemented and cite only results that exist in the repo
  (parity tests + `benchmarks/REPORT.md`); all unimplemented research is marked
  future work. See `docs/paper/README.md`.
- **Reproducibility standard** — `docs/REPRODUCIBILITY.md` defines the per-run /
  per-generation artifact set, mandatory metadata, and an offline-vs-live
  verification path.
- **Hackathon packaging** — `docs/HACKATHON_DECK.md` (slide outline) and
  `docs/HACKATHON_DEMO.md` (run-of-show + per-track narrative).
- **Native Rust port strategy** — the Python → Rust module map, native LLM-runner
  design, and testing seams live in `docs/RUST_PORT.md`; the migration plan is
  tracked under umbrella issue #34.

---

## Implemented Today

### Deterministic SIA core (Rust port)

The orchestration layer is ported to Rust while preserving Python behavior at the
artifact boundary.

- Generation loop, run/venv setup, target-agent subprocess execution, evaluation
  handling: `src/orchestrator.rs`, `src/run.rs`, `src/run_setup.rs`,
  `src/layout.rs`.
- Prompt / context / result surfaces (the byte-for-byte ones):
  `src/prompts.rs`, `src/context_manager.rs`, `src/results.rs`.
- Config, providers, profiles, credential resolution (`.env` loading with real
  env vars taking precedence): `src/config.rs`, `src/config_files.rs`,
  `src/providers.rs`, `src/profiles.rs`, `src/api_keys.rs`, `src/env_file.rs`.
- CPython-compatible JSON / formatting that underpins the parity gate:
  `src/pyjson.rs`, `src/pyfmt.rs`.
- The Python target-agent contract (`evaluate.py`, task directory layout, the
  `runs/` artifact format) is preserved unchanged — existing Python tasks run as
  before.

### Native multi-provider LLM runner layer (`--features llm`)

A feature-gated native Rust LLM layer for the Meta-Agent / Feedback-Agent paths,
built on `rig-core` + injectable HTTP transports so the full tool-use loops are
tested **offline** with scripted responses.

- Runners: `src/llm/claude_runner.rs`, `src/llm/openhands_runner.rs`,
  `src/llm/pydantic_ai_runner.rs`.
- Transports / provider mapping: `src/llm/anthropic_api.rs`,
  `src/llm/openai_api.rs`, `src/llm/provider_mapping.rs`.
- Resilience and structure: `src/llm/retry.rs` (retry/backoff + fallback),
  `src/llm/structured.rs` (structured-output extraction + answer-parsing parity).
- Tooling: `src/llm/tools.rs` (path containment + bounded bash),
  `src/llm/tavily.rs` (Tavily search client primitive).
- Without `--features llm`, the runners return a clear "build with `--features
  llm`" message and everything else still works.

### Observability & reproducible artifacts

- Trajectory + telemetry written in the existing run shapes:
  `src/llm/trajectory.rs`, `src/llm/trajectory_middleware.rs`,
  `src/llm/telemetry.rs` (per-generation `telemetry.json` with token/call/timing
  fields).
- The on-disk `runs/` tree is both the web UI's data source and the
  reproducibility bundle (`docs/REPRODUCIBILITY.md`).

### SIA Studio (web visualizer)

- Axum/Tokio server rendering `runs/` from disk: `src/web/server.rs`,
  `src/web/runs.rs`; size-limited artifact/JSON reads; covered by
  `tests/web_api.rs` and `tests/web_data.rs` (including path-safety cases).
- `cargo run -- web` serves it end-to-end.

### Safety / sandboxing

- `src/sandbox.rs`: `Capabilities` with deny-by-default / read-only / permissive
  presets, path containment, bash-prefix gating, and file-size limits. Native
  runners consult it before any filesystem or bash action.
- Optional Docker target-agent sandbox + evaluator/target wall-clock timeouts in
  `src/orchestrator.rs`. Threat model in `SECURITY.md`.

### Verifiers & adaptive closed-loop primitives

- `src/verifier.rs`: a reusable `Verifier` trait with `ExactMatch`,
  `MultipleChoice`, `NumericTolerance`, `Contains` verifiers, partial-credit
  scoring, and adversarial/stability hooks.
- `src/scheduler.rs`: a deterministic, transparent adaptive scheduler over
  improvement-efficiency and plateau signals.
- `src/closed_loop.rs`, `src/weights.rs`: scheduler-decision + weight-update
  artifact recording, a `WeightUpdater` trait, and a tested **pure-CPU reference
  LoRA** path (not production GPU training).

### Benchmarks, parity & eval infrastructure

- `benches/core.rs` (Criterion) + `benchmarks/bench_python.py` +
  `benchmarks/run_comparison.py` regenerate `benchmarks/REPORT.md` from
  byte-identical fixtures.
- `scripts/parity_check.py` + `src/bin/sia_parity.rs` form the differential
  parity gate.
- `evals/` is a standalone `dspy-rs` crate (GPQA-style, offline mock + optional
  real provider); see `evals/README.md`.

### CI

`.github/workflows/rust.yml` runs fmt, clippy, and tests for **both** the default
and `--features llm` builds, the differential parity gate, and the standalone
`evals/` crate. `.github/workflows/ci.yml` exercises the Python reference across
Python 3.11–3.14.

---

## Benchmarks

Geometric-mean speedup of the deterministic core (Python / Rust) across nine
operations: **5.8×** (from `benchmarks/REPORT.md`). Selected results:

| Operation | Speedup (Python / Rust) |
| --- | ---: |
| `build_meta_prompt` | 24.7× |
| `load_agent_execution_single` | 17.9× |
| `build_feedback_context_single` | 14.0× |
| `build_feedback_context_multi` | 10.2× |
| `web_get_run` | 2.7× |
| `context_manager_run` | 1.2× |

> Measured on a 4-core Xeon @ 2.80GHz, Linux, CPython 3.11.15 vs. rustc 1.94.1
> (release profile). ns/op figures mix CPU and filesystem I/O for fs-backed ops;
> treat speedups as order-of-magnitude indicators. Full table + methodology in
> `benchmarks/REPORT.md`.

---

## Hackathon Tracks

- **Framework Enhancement** — native Rust performance (~5.8× core), capability
  sandbox + optional Docker jail, SIA Studio dashboard, feature-gated lean build.
- **Applied** — runnable bundled tasks (`gpqa`, `lawbench`, `longcot-chess`,
  `spaceship-titanic`), the `Verifier` trait + native evaluation hooks, and the
  byte-parity harness in CI.
- **Research** — adaptive harness scheduler, native weight-update abstraction,
  and trajectory telemetry as a research signal (see Roadmap; several items are
  in-progress / planned). Paper angles tracked in `docs/paper/`.

---

## Sponsor Integrations

- **Nebius (Token Factory)** — five hosted-model profiles ship with the crate
  (`kimi-nebius-target`, `kimi-nebius-meta`, `qwen-nebius-target`,
  `gptoss-nebius-target`, `deepseek-nebius-target`). Setup is a single env var
  (`NEBIUS_API_KEY`); `src/llm/retry.rs` handles transient 429s for long
  multi-generation runs. Quickstart and model-slug verification:
  `docs/NEBIUS_QUICKSTART.md`, `docs/NEBIUS_MODELS.md`,
  `scripts/verify_nebius_models.sh`.
- **Tavily** — a native search-client primitive (`src/llm/tavily.rs`) usable as a
  tool inside the native runner loops.

---

## Roadmap / Planned

These are honestly scoped as future work, not shipped claims:

- **Live end-to-end self-improvement studies.** The repo is strongest on
  deterministic/offline surfaces; live multi-generation accuracy studies are not
  yet reported here.
- **Adaptive harness scheduler as a research result** (issue #65). The current
  `src/scheduler.rs` is a transparent heuristic; a learned policy that adjusts the
  feedback window based on the accuracy trajectory is planned.
- **Native weight updates** (paper section, cf. issue #19). The shipped path is a
  CPU **reference** LoRA; production GPU fine-tuning / loaded model adapters and
  gradient-based (RLHF/DPO) pipelines against the Rust trajectory format are
  roadmap.
- **OS-level confinement.** The in-process capability sandbox is shipped;
  kernel-enforced Landlock/seccomp/WASI isolation remains future work.
- **Full meta-RL policy** over harness-vs-weight-update decisions.
- **Broader provider/profile coverage** and continued expansion of the parity and
  eval matrices.

---

## Quick Start

```bash
cargo build                      # lean default build (no LLM client deps)
cargo build --features llm       # include the native rig-core LLM runners
cargo test                       # full suite (unit + integration + golden parity)
cargo run -- web                 # serve the SIA Studio visualizer (./runs)
cargo run -- --help              # CLI help (run / web sub-commands)
```

To drive a real loop, build with `--features llm` and set provider credentials
(e.g. `ANTHROPIC_API_KEY` or `NEBIUS_API_KEY`); see `docs/CREDENTIALS.md`.

## Pointers

- `README.md` — overview and architecture diagram.
- `docs/RUST_PORT.md` — Python → Rust module map, LLM-runner design, parity.
- `benchmarks/REPORT.md` — full benchmark table + methodology.
- `evals/README.md` — the `dspy-rs` eval harness.
- `docs/REPRODUCIBILITY.md` — artifact set + verification standard.
- `docs/paper/sia_rust_preprint.md` — preprint-style paper draft.
- `SECURITY.md` — threat model and sandbox controls.
</content>
</invoke>
