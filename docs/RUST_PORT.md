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
call is the documented boundary and surfaces a clear error in this port. The
**target agent** runs as a real Python subprocess (`std::process`), and the web
visualizer, prompts, context tracking, evaluation flow, and CLI are fully
functional. The Python `sia/tasks/` reference agents + evaluators are task *data*
(read/executed by the agents) and remain unchanged.

## Testing seams

Where the Python tests patch `subprocess.run` / `subprocess.Popen`, the Rust port
exposes injectable seams (`run_evaluation_with`, `run_target_agent_with`,
`run_generation_with`) so the branching logic is unit-tested without a real
interpreter — same coverage, idiomatic Rust.
