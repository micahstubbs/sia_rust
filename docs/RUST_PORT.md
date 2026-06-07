# SIA — Rust port

This repository contains a Rust conversion of the Python `sia` package, developed
with red→green TDD: every Python test (and all 7 golden-master files) is mirrored
in Rust and passes, with the golden prompt/`context.md`/feedback-context outputs
reproduced **byte-for-byte**.

## Build & test

```bash
cargo build                  # lean default build (no LLM client deps)
cargo build --features llm   # include the native rig-core LLM runners
cargo test                   # runs the full suite (unit + integration + golden)
cargo test --features llm    # also runs the LLM-runner / middleware tests (offline)
cargo run -- web             # serve the runs visualizer (./runs by default)
cargo run -- --help          # CLI help (run / web sub-commands)
```

CI (`.github/workflows/rust.yml`) runs `cargo fmt --check`, `cargo clippy
-D warnings`, and `cargo test` for **both** the default build and `--features llm`,
plus the differential-parity gate and the standalone `evals/` crate.

## Module map (Python → Rust)

| Python (`sia/…`) | Rust (`src/…`) |
|---|---|
| `config.py` | `config.rs` |
| `io_utils.py` | `io_utils.rs` |
| `results.py` | `results.rs` |
| `api_keys.py` | `api_keys.rs` |
| `config_files.py` | `config_files.rs` |
| `providers.py` | `providers.rs` |
| `agent_reference.py` | `agent_reference.rs` |
| `profiles.py` | `profiles.rs` |
| `layout.py` | `layout.rs` |
| `agent_impls/*` | `agent_impls/*` |
| `prompts.py` | `prompts.rs` |
| `context_manager.py` | `context_manager.rs` |
| `run_setup.py` | `run_setup.rs` |
| `orchestrator.py` | `orchestrator.rs` + `run.rs` |
| `cli.py` | `cli.rs` |
| `web/runs.py` | `web/runs.rs` |
| `web/server.py` | `web/server.rs` |
| *(native, no Python source)* | `llm/*` — see below |

### Native LLM client (`src/llm/`, feature `llm`)

The meta/feedback runners' actual LLM calls — left as the integration boundary in
the initial port — are now implemented natively on
[`rig-core`](https://crates.io/crates/rig-core), behind the optional `llm` cargo
feature so the default build/published crate stay lean.

| Module | Role |
|---|---|
| `llm/mod.rs` | `AgentRunner` trait, `TrajectoryContext`/`AgentRunOutcome`, re-exports |
| `llm/trajectory.rs` | `AgentTrajectory` — serializes to the `agent_execution.json` shape |
| `llm/trajectory_middleware.rs` | `TrajectoryMiddleware` — structured event/usage/timing capture |
| `llm/rig_runner.rs` | `RigAgentRunner` — single-turn prompt via rig's Anthropic client |
| `llm/anthropic_api.rs` | Anthropic Messages API types + injectable `MessagesTransport` |
| `llm/openai_api.rs` | OpenAI-compatible chat-completions types + `ChatTransport` |
| `llm/tools.rs` | Sandboxed `Bash`/`Read`/`Write`/`Edit`/`Glob` executors (shared) |
| `llm/claude_runner.rs` | Native **claude** runner — `/v1/messages` tool-use loop |
| `llm/openhands_runner.rs` | Native **openhands** runner — chat-completions loop + `openhands_trajectory/` events |
| `llm/pydantic_ai_runner.rs` | Native **pydantic-ai** runner — write/read/bash + request limit |
| `llm/provider_mapping.rs` | `Provider` → constructed transport (api-key/base-url resolution) |
| `llm/retry.rs` | `RetryPolicy` + backoff + transport decorators with optional fallback |
| `llm/structured.rs` | Structured-output extraction/parity harness + rig `Extractor` wrapper |
| `llm/workspace/` | State-externalizing **Workspace** for Target Agents (issue #148, Harness-1) — see [docs/WORKSPACE.md](WORKSPACE.md) |

Every loop is driven through an **injectable transport**, so the full tool-use
loops are tested offline with scripted/mocked responses (real provider calls are
`#[ignore]`d, gated on the relevant API key).

Library mapping: pydantic/json → `serde` + `serde_json` (`preserve_order`);
argparse → `clap`; FastAPI/uvicorn → `axum`/`tokio`; `subprocess` → `std::process`;
`importlib.resources` package-data → `include_dir` (bundled provider/profile JSON
and the web `index.html` are embedded at build time).

## Security & sandboxing

SIA runs self-modifying agents, so the execution sandbox is a first-class concern.
The formal threat model — assets, the orchestrator / native-runner-tools / Python
target-subprocess trust boundaries, escalation paths (filesystem escape, network
exfiltration, resource exhaustion, prompt-injection-driven tool abuse), current
mitigations, and the roadmap to OS-level enforcement (capability allow-list →
landlock/seccomp → WASI) — lives in [`SECURITY.md`](../SECURITY.md).

The first concrete hardening primitive is `src/sandbox.rs` (`pub mod sandbox`, on
the **default** build): a pure-`std` **capability allow-list**
(`Capabilities` + `check_read`/`check_write`/`check_bash`/`check_within_root`/
`check_size`) that native tool executors can consult before acting, with
deny-by-default and `permissive`/`read_only` presets. It is the single auditable
enforcement point the landlock/WASI roadmap builds on.

## Meta/feedback runners — native vs. the old boundary

The meta/feedback agent runners (`claude` / `openhands` / `pydantic-ai`) originally
wrapped external LLM SDKs with no Rust equivalent, so the actual LLM call was a
documented *integration boundary* that returned an error. That boundary is now
**closed natively** (epic #38): with `--features llm`, each runner drives a real
agentic tool-use loop on `rig-core`/HTTP transports (see `src/llm/` above), captures
a faithful trajectory, and writes the visualizer-compatible artifacts. The registry,
dispatch, and `resolve_model` logic remain shared and identical across builds.

Without the `llm` feature, the runners return a clear *"build with `--features
llm`"* message — keeping the default build dependency-light — while everything else
(orchestration, web UI, parity) still works. The **target agent** always runs as a
real Python subprocess (`std::process`) via the `evaluate.py` contract.

What is functional today: `sia web` (the visualizer) end-to-end; the deterministic
orchestration scaffolding (task resolution, profile/provider loading, run-directory
+ venv setup, prompt building, target-agent subprocess execution, evaluation,
context tracking, feedback context); the CLI parsing/dispatch; and — with
`--features llm` — the native meta/feedback agent loops. The Python `sia/tasks/`
reference agents + evaluators are task *data* (read/executed by the agents) and
remain unchanged.

## Parity, benchmarks & evals

- **Differential parity** — `scripts/parity_check.py` runs the reference Python
  implementation and the Rust `sia-parity` helper on the same inputs (json.dumps,
  meta/feedback prompts, feedback context, execution loading) over an ASCII + CJK +
  emoji + control-char matrix and asserts byte-identical output. CI runs it on
  every push. The `src/pyjson.rs` serializer reproduces CPython's
  `json.dumps(..., ensure_ascii=True)` exactly (needed for LawBench/Chinese).
- **Benchmarks** — `cargo bench` (Criterion, `benches/core.rs`) and
  `benchmarks/bench_python.py` measure the same core ops in both languages;
  `python benchmarks/run_comparison.py` regenerates `benchmarks/REPORT.md`. The
  Rust port runs the deterministic core **~5.8× faster** (geometric mean) — up to
  ~25× on prompt building and ~18× on execution-log loading.
- **Evals** — `evals/` is a standalone crate built on [`dspy-rs`/DSRs](https://github.com/krypticmouse/DSRs)
  implementing a GPQA-style multiple-choice `Signature` + `Module` + accuracy
  `Evaluator` with an offline mock adapter (no network/keys) and a real-provider
  path. Run with `cargo test --manifest-path evals/Cargo.toml`; see `evals/README.md`.

## Python-side offline tooling

Some tooling in this repository is **intentionally not reimplemented in Rust**
because it is offline dataset-preparation work (not part of the `sia run`/`sia web`
runtime), depends on the Python ML ecosystem (`mlebench`, Kaggle, Google Gemini),
and the project already bridges to Python for dataset and ML-ecosystem needs. The
Python module ships in this repo and remains the authority; a thin native
`Command`-wrapper port could be added later if desired (out of scope for the
current port effort).

### `sia/prepare_mlebench_dataset.py` — MLE-Bench dataset prep

**Entrypoint:** `python -m sia.prepare_mlebench_dataset`

**Purpose:** Prepares a task directory from an MLE-Bench Kaggle competition so it
is ready to be consumed by the SIA self-improvement loop. It runs these six steps:

1. `[1/6]` Run `mlebench prepare -c <competition-id>` — downloads and prepares the
   competition data into the mlebench cache (`~/.cache/mle-bench/data/<competition-id>/prepared/`).
2. `[2/6]` Copy `prepared/public/` and `prepared/private/` into
   `<tasks-dir>/<competition-id>/data/public/` and `…/data/private/`.
3. `[3/6]` Rename `data/public/description.md` → `data/public/task.md` (the
   filename the SIA orchestrator expects).
4. `[4/6]` *(optional, skipped with `--skip-gemini`)* Call the Gemini API
   (`gemini-3-flash-preview`) to synthesize 3–5 diverse similar ML task descriptions
   from `task.md`, to produce training variety.
5. `[5/6]` Write `<tasks-dir>/<competition-id>/reference/SAMPLE_TASK_DESCRIPTIONS.md`
   (Gemini output, or a placeholder when skipped).
6. `[6/6]` Copy `<tasks-dir>/_shared/reference_target_agent.py` →
   `<tasks-dir>/<competition-id>/reference/reference_target_agent.py`.

**Flags** (from `argparse`):

| Flag | Default | Description |
|---|---|---|
| `-c` / `--competition` | *(required)* | Competition ID, e.g. `spaceship-titanic` |
| `--tasks-dir` | `./sia/tasks` | Base tasks directory |
| `--skip-gemini` | off | Skip the Gemini API call (steps 4–5 produce a placeholder) |

**Prerequisites:**

- `mlebench` installed and Kaggle credentials configured (the `mlebench prepare`
  sub-command downloads competition data; see the
  [MLE-Bench docs](https://github.com/openai/mle-bench)).
- `google-generativeai` and `python-dotenv` Python packages (`pip install
  google-generativeai python-dotenv`).
- `GEMINI_API_KEY` in your environment or `.env` (see
  [docs/CREDENTIALS.md](CREDENTIALS.md)). If the key is absent the script warns
  and produces a placeholder; pass `--skip-gemini` to bypass the step entirely.

**Typical usage:**

```bash
# Full run (downloads data + generates Gemini similar-task descriptions):
python -m sia.prepare_mlebench_dataset -c spaceship-titanic

# Offline / no Gemini key (data only):
python -m sia.prepare_mlebench_dataset -c spaceship-titanic --skip-gemini

# Custom tasks directory:
python -m sia.prepare_mlebench_dataset -c spaceship-titanic --tasks-dir ./my_tasks --skip-gemini
```

**Why not reimplemented in Rust:** This is offline dataset-preparation tooling, not
part of the `sia run`/`sia web` runtime. It depends on `mlebench`, Kaggle
credentials, and the Google Gemini API — none of which are part of the Rust
runtime's dependency surface. The project already bridges to Python subprocesses
for target-agent execution, and native ML-ecosystem tooling belongs on the Python
side. A thin `std::process::Command` wrapper around this script could be added as a
future convenience if desired, but it is out of scope for the current port.

## Testing seams

Where the Python tests patch `subprocess.run` / `subprocess.Popen`, the Rust port
exposes injectable seams (`run_evaluation_with`, `run_target_agent_with`,
`run_generation_with`) so the branching logic is unit-tested without a real
interpreter — same coverage, idiomatic Rust.

## Provider quickstarts

- **[docs/NEBIUS_QUICKSTART.md](NEBIUS_QUICKSTART.md)** — Nebius Token Factory: get credentials,
  run a bundled Nebius profile end-to-end, add custom provider/profile JSON, read `telemetry.json`.

## Hackathon demo

- **[docs/HACKATHON_DEMO.md](HACKATHON_DEMO.md)** — time-boxed demo script, per-track judging narrative,
  graceful-degradation backup paths, Q&A prep, and a pre-demo checklist.
