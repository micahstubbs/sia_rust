# Unique Contributions

This document summarizes the project-specific contributions in
`micahstubbs/sia_rust`. It is based on a project-wide review of the Rust source,
Python bridge, tests, CI, documentation, benchmark assets, and eval harness as of
commit `4a18736`.

## What We Inherited

The core SIA idea comes from the original Self-Improving AI framework and paper:
a Meta-Agent writes a Target-Agent, the Target-Agent runs a task, and a
Feedback-Agent uses the resulting trajectory to improve the next generation.

This repository intentionally preserves several inherited contracts:

- The task directory shape, bundled task examples, and `evaluate.py` scoring
  contract remain Python-compatible.
- Generated target agents still run as Python subprocesses so existing Python
  task ecosystems and reference agents continue to work.
- The `runs/run_<id>/gen_<n>/` artifact layout remains the interchange format
  for trajectories, results, context, and improvements.

The contributions below are the work this repository adds around that foundation.

## 1. Faithful Native Rust Port of the Deterministic SIA Core

This repo ports the deterministic orchestration layer to Rust while preserving
the Python behavior at the artifact boundary. The implementation covers the CLI,
configuration, providers, profiles, prompt construction, context management,
run-directory layout, target-agent execution, evaluation handling, feedback
context, and the web data layer.

Key evidence:

- `src/run.rs`, `src/orchestrator.rs`, `src/run_setup.rs`, and `src/layout.rs`
  implement the generation loop and run layout.
- `src/prompts.rs`, `src/context_manager.rs`, and `src/results.rs` mirror the
  prompt/context/result surfaces.
- `src/providers.rs`, `src/profiles.rs`, `src/config_files.rs`, and bundled
  files in `sia/defaults/` implement provider/profile resolution.
- `src/pyjson.rs`, `src/bin/sia_parity.rs`, and `scripts/parity_check.py`
  provide byte-parity tooling against the Python reference.
- Golden and parity tests live under `tests/golden/`, `tests/*golden*.rs`, and
  `tests/*snapshot*.rs`.

This is not just a line-by-line translation. The Rust port adds explicit typed
interfaces, structured errors, injectable runners for tests, feature-gated
native LLM support, and CI coverage for both lean and `llm` builds.

## 2. Native Multi-Provider LLM Runner Layer

The project adds a feature-gated native Rust LLM layer for the Meta-Agent and
Feedback-Agent paths. This closes the original Python-SDK boundary for the
agent-improvement loop while keeping generated target agents in Python.

Key evidence:

- `src/llm/mod.rs` defines the `AgentRunner` abstraction, trajectory context,
  and run outcome.
- `src/llm/claude_runner.rs` implements an Anthropic Messages API tool-use loop.
- `src/llm/openhands_runner.rs` implements an OpenHands-style
  OpenAI-compatible loop and event-log format.
- `src/llm/pydantic_ai_runner.rs` implements a PydanticAI-style loop using the
  in-tree OpenAI-compatible transport and shared tools.
- `src/llm/anthropic_api.rs`, `src/llm/openai_api.rs`, and
  `src/llm/provider_mapping.rs` provide injectable transports and provider
  resolution.
- `src/llm/retry.rs` adds retry/backoff and optional fallback transport
  decorators.
- `src/llm/structured.rs` adds structured-output extraction and answer-parsing
  parity.
- `src/llm/tavily.rs` adds a Tavily search client primitive.

The entire layer is behind the non-default `llm` Cargo feature in `Cargo.toml`,
so the default crate remains smaller while CI still exercises the native runner
code offline with mocked/scripted transports.

## 3. Trajectory, Telemetry, and Reproducible Artifact System

The repo turns SIA runs into inspectable, file-backed evidence bundles. Native
runners write trajectories and telemetry in shapes that the orchestrator and web
visualizer can read without adapters.

Key evidence:

- `src/llm/trajectory.rs` writes `agent_execution.json` in the existing run
  format.
- `src/llm/trajectory_middleware.rs` records structured events, token usage,
  tool calls, errors, and timing.
- `src/llm/telemetry.rs` writes `telemetry.json` with token/call/timing fields.
- `src/web/runs.rs` and `src/web/server.rs` expose run, generation, artifact,
  trajectory, telemetry, metrics, scheduler, and weight-update data.
- `docs/REPRODUCIBILITY.md` defines the run artifact set and verification
  standard.

This gives third parties a concrete audit trail: the on-disk `runs/` tree is both
the UI data source and the reproducibility bundle.

## 4. SIA Studio: File-Backed Rust Web Visualizer

The Rust port includes a native Axum web surface for inspecting self-improvement
runs directly from disk. It extends the original visualization concept with
telemetry, metrics, scheduler, and weight-update surfaces.

Key evidence:

- `src/web/server.rs` serves the HTTP API and static dashboard.
- `src/web/runs.rs` parses run directories and exposes typed summaries/details.
- `tests/web_api.rs` and `tests/web_data.rs` cover API behavior and path-safety
  cases.
- `docs/HACKATHON_DECK.md` and `docs/HACKATHON_DEMO.md` document the dashboard
  as "SIA Studio" for demos.

Recent hardening also caps web artifact and JSON reads through size-limited
helpers, preventing oversized run artifacts from being loaded unboundedly.

## 5. Safety and Sandboxing Primitives for Self-Modifying Agents

Because SIA runs model-generated code and model-selected tool calls, this repo
adds an explicit safety layer around native tools and target-agent execution.

Key evidence:

- `src/sandbox.rs` defines `Capabilities`, deny-by-default/read-only/permissive
  presets, path containment checks, Bash prefix checks, and file-size limits.
- Native LLM runners call `Capabilities` before filesystem writes, reads, edits,
  or Bash execution in `src/llm/claude_runner.rs`,
  `src/llm/openhands_runner.rs`, and `src/llm/pydantic_ai_runner.rs`.
- `src/llm/tools.rs` adds lexical path containment and bounded Bash execution.
- `src/orchestrator.rs` supports a Docker target-agent sandbox path and enforces
  evaluator/target-agent wall-clock timeouts.
- `SECURITY.md` records the threat model, trust boundaries, current controls,
  and OS-level roadmap.

The current contribution is a practical in-process policy layer plus optional
Docker confinement. Kernel-enforced Landlock/seccomp/WASI isolation remains a
roadmap item, not a shipped claim.

## 6. Adaptive Closed-Loop Extensions

The repository adds first-pass native Rust primitives for the SIA paper's broader
harness-vs-weight-update research direction.

Key evidence:

- `src/scheduler.rs` implements a deterministic adaptive scheduler over
  improvement efficiency and plateau signals.
- `src/closed_loop.rs` records scheduler decisions and weight-update artifacts
  into generation directories.
- `src/weights.rs` defines a `WeightUpdater` trait, trajectory-to-training
  extraction, a trigger seam, and a pure-CPU reference LoRA updater.
- `src/verifier.rs` defines a reusable `Verifier` trait, exact/multiple-choice/
  numeric/contains verifiers, adversarial variants, and stability checks.

These are deliberately scoped contributions. The scheduler is a transparent
heuristic, the LoRA path is a CPU reference implementation, and production GPU
fine-tuning or a full meta-RL policy is future work.

## 7. Standalone Rust Eval Harness on `dspy-rs`

The `evals/` crate adds a separate GPQA-style multiple-choice evaluation harness
using `dspy-rs`.

Key evidence:

- `evals/src/lib.rs` defines the `GpqaSignature`, `GpqaModule`, scoring, mock
  adapter, and real-provider path.
- `evals/fixtures/gpqa_sample.json` provides a small offline fixture.
- `evals/examples/run_eval.rs` demonstrates offline and real-provider execution.
- `.github/workflows/rust.yml` builds and tests the eval crate offline.

This gives the project a Rust-native evaluation surface independent of the root
crate while preserving the GPQA-style answer-normalization and accuracy semantics.

## 8. Performance and Parity Measurement Infrastructure

The project includes reproducible benchmark and parity machinery, not only code.

Key evidence:

- `benches/core.rs` benchmarks Rust deterministic core operations with Criterion.
- `benchmarks/bench_python.py` measures the corresponding Python operations.
- `benchmarks/run_comparison.py` regenerates the comparison report.
- `benchmarks/REPORT.md` reports a 5.8x geometric-mean speedup across nine
  deterministic core operations in the measured environment.
- `scripts/parity_check.py` compares Python and Rust outputs over an ASCII,
  CJK, emoji, and control-character matrix.

These tools make the Rust-port claims independently rerunnable instead of relying
on prose claims alone.

## 9. Provider, Profile, Credential, and Demo Packaging

The repo adds a practical provider/profile layer and documentation package for
running SIA against multiple hosted model providers.

Key evidence:

- Bundled providers: `sia/defaults/providers/anthropic.json`,
  `gemini.json`, `nebius.json`, `openai.json`, `tinker.json`, and
  `together.json`.
- Bundled profiles: default Claude profiles plus Nebius, Gemini, and GPT-OSS
  target/meta profiles, including Tinker-backed GPT-OSS and Qwen3 target
  profiles in `sia/defaults/profiles/`.
- `src/api_keys.rs`, `src/env_file.rs`, and `docs/CREDENTIALS.md` document and
  implement credential resolution, including `.env` loading with real env vars
  taking precedence.
- `docs/NEBIUS_QUICKSTART.md`, `docs/NEBIUS_MODELS.md`,
  `scripts/verify_nebius_models.sh`, and `tests/nebius_live.rs` support Nebius
  profile validation.
- `docs/HACKATHON_DEMO.md`, `docs/HACKATHON_DECK.md`, and
  `docs/paper/sia_rust_preprint.md` package the work for judges, reviewers, and
  future maintainers.

## 10. Testing, CI, and Review Workflow

The project has a cross-language verification setup that covers both the Python
bridge and the Rust port.

Key evidence:

- `.github/workflows/ci.yml` tests Python packaging, imports, lint/format/type
  checks, and Python tests across Python 3.11-3.14.
- `.github/workflows/rust.yml` runs Rust format, clippy, default tests,
  `--features llm` tests, Python reference tests, differential parity, and eval
  crate tests.
- Rust tests cover CLI, config, context, prompts, orchestration, LLM loops,
  sandboxing, web APIs, telemetry, scheduler, weights, and verifiers.
- Python tests preserve bridge behavior for the still-shipped Python package.
- `AGENTS.md` documents the autonomous review-fix workflow, including beads and
  companion GitHub issues for review findings.

## Honest Current Limits

This repository is strongest on deterministic/offline surfaces: Rust port
fidelity, native runner seams, observability, safety primitives, benchmark
infrastructure, and test coverage. It should not be overstated as having already
completed every research ambition.

Current limits to keep explicit:

- Live end-to-end self-improvement accuracy studies are not reported here.
- Some live provider checks are `#[ignore]` tests that require network access and
  real API keys.
- The scheduler is heuristic, not a full meta-RL policy.
- The weight-update implementation is a CPU reference LoRA path, not production
  GPU fine-tuning or a loaded model adapter.
- The in-process capability sandbox improves native tool safety, but OS-level
  confinement remains future work.

## Short Version

`sia_rust` contributes a faithful, parity-tested Rust implementation of SIA's
deterministic core; native Rust LLM runners for the meta/feedback loop; a
file-backed observability and reproducibility system; SIA Studio; capability
sandboxing primitives; adaptive scheduler, verifier, and weight-update research
extensions; a standalone Rust eval harness; and benchmark/CI infrastructure that
makes the claims rerunnable.
