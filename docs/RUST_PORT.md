# SIA — Rust port

This repository contains a Rust conversion of the Python `sia` package, developed
with red→green TDD: every Python test (and all 7 golden-master files) is mirrored
in Rust and passes, with the golden prompt/`context.md`/feedback-context outputs
reproduced **byte-for-byte**.

## Build & test

```bash
cargo build            # builds the `sia` library + binary
cargo test             # runs the full suite (unit + integration + golden)
cargo run -- web       # serve the runs visualizer (./runs by default)
cargo run -- --help    # CLI help (run / web sub-commands)
```

CI (`.github/workflows/rust.yml`) runs `cargo fmt --check`, `cargo clippy
-D warnings`, and `cargo test`.

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

Library mapping: pydantic/json → `serde` + `serde_json` (`preserve_order`);
argparse → `clap`; FastAPI/uvicorn → `axum`/`tokio`; `subprocess` → `std::process`;
`importlib.resources` package-data → `include_dir` (bundled provider/profile JSON
and the web `index.html` are embedded at build time).

## Integration boundary

The meta/feedback agent runners (`claude` / `openhands` / `pydantic-ai`) wrap
external LLM SDKs that have no Rust equivalent. The registry, dispatch, and
model-spec resolution (`resolve_model`) are ported and tested; the actual LLM
call is the documented boundary and surfaces a clear error in this port (native
runners are tracked in #38–#41). The **target agent** runs as a real Python
subprocess (`std::process`).

What is functional today: `sia web` (the visualizer) end-to-end; the deterministic
orchestration scaffolding (`sia run` up to the meta-agent call — task resolution,
profile/provider loading, run-directory + venv setup, prompt building, target-agent
subprocess execution, evaluation, context tracking, feedback context); and the CLI
parsing/dispatch. What is **not yet end-to-end**: a full `sia run` self-improvement
loop, because the meta/feedback agents need a native LLM runner (#38–#41) — it stops
with a clear error at the first LLM call. The Python `sia/tasks/` reference agents +
evaluators are task *data* (read/executed by the agents) and remain unchanged.

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

## Testing seams

Where the Python tests patch `subprocess.run` / `subprocess.Popen`, the Rust port
exposes injectable seams (`run_evaluation_with`, `run_target_agent_with`,
`run_generation_with`) so the branching logic is unit-tested without a real
interpreter — same coverage, idiomatic Rust.
