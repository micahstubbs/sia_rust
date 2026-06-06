# SIA (Self-Improving AI) — Rust Port

> **Status:** Active Rust port of the original Python implementation. The TDD
> foundation (orchestrator, web UI, Python subprocess bridge for target agents,
> config/profiles/providers, byte-for-byte prompt/context parity) is in place,
> **and** the meta/feedback agents now have **native Rust LLM runners** built on
> [`rig-core`](https://crates.io/crates/rig-core) — Claude (Anthropic Messages
> API), OpenHands-style (OpenAI-compatible), and PydanticAI-style — behind the
> optional `llm` cargo feature. See the [umbrella issue #34](https://github.com/micahstubbs/sia_rust/issues/34)
> for the migration plan and status.

[![arXiv](https://img.shields.io/badge/arXiv-2605.27276-b31b1b.svg)](https://arxiv.org/abs/2605.27276)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](https://opensource.org/licenses/MIT)

Official Rust implementation (with a Python execution bridge) of
[**SIA: Self-Improving AI with Harness & Weight Updates**](https://arxiv.org/abs/2605.27276).

---

## What this is

SIA evolves an agent's *harness* (its scaffold/code) across generations: a
**Meta-Agent** writes/improves a target agent, the **Target-Agent** runs the task,
and a **Feedback-Agent** analyzes the trajectory and proposes the next
improvement. This repository ports that loop to Rust while keeping the exact
self-improvement semantics from the paper.

### Architecture & the Python bridge

```
              ┌─────────────────────────────────────────────────────────┐
              │                       sia (Rust)                        │
  sia run ──► │  orchestrator ─► context_manager ─► results             │
              │      │                                                  │
              │      ├─ Meta / Feedback agents                          │
              │      │    └─► native LLM runners  [feature: llm]        │
              │      │         src/llm/* on rig-core:                   │
              │      │           • claude      — Anthropic /v1/messages tool loop
              │      │           • openhands   — OpenAI-compatible chat-completions
              │      │           • pydantic-ai — write/read/bash + request limit
              │      │         + transports · trajectory · retry · structured
              │      │                                                  │
              │      └─ Target agent ──► Python subprocess              │
              │                          (optional Docker sandbox)      │
              │                          via the evaluate.py contract   │
              │                                                         │
              │  sia web ─► Axum visualizer of ./runs                   │
              └─────────────────────────────────────────────────────────┘
```

**Why a Python bridge?** Target agents are *generated code* that must run exactly
as the paper intends (and often depend on the Python ML/task ecosystem), so SIA
executes them as a real Python subprocess via the `evaluate.py` contract — with an
optional Docker sandbox. The **meta/feedback** agents, by contrast, are now driven
by native Rust LLM clients (no Python SDK needed) when built with `--features llm`.
This phased approach gives safety + performance wins while preserving the task
contract that existing custom tasks rely on.

The native LLM layer (`src/llm/`) is **feature-gated** so the default build and the
published crate stay lean; CI exercises both the default build and `--features llm`.

## Build & run

```bash
cargo build                      # lean default build (no LLM client deps)
cargo build --features llm       # include the native rig-core LLM runners
cargo test                       # full suite (unit + integration + golden parity)
cargo test --features llm        # also runs the LLM-runner / middleware tests (offline)

cargo run -- web                 # serve the runs visualizer (./runs by default)
cargo run -- --help              # CLI help (run / web sub-commands)
```

To drive a real self-improvement loop, build with `--features llm` and set the
provider credentials (e.g. `ANTHROPIC_API_KEY`, or a provider's `api_key_env`).

## Coming from the Python version

If you used the original Python `sia`:

- **Tasks are unchanged.** The task directory layout, the `evaluate.py` scoring
  contract, the public/private split, and "bring your own task" all work exactly as
  before — target agents still run as Python subprocesses.
- **CLI is the same shape.** `sia run` / `sia web` mirror the Python entry points;
  profiles, providers, and the `runs/` directory format are preserved.
- **Agent backends map 1:1.** The `claude`, `openhands`, and `pydantic-ai`
  `agent_impl`s have native Rust runners (build with `--features llm`); model-spec
  resolution (`resolve_model`) matches the Python behavior. Without the `llm`
  feature, the meta/feedback runners return a clear "build with `--features llm`"
  message and everything else (orchestration, web UI, parity) still works.
- **Outputs are compatible.** Trajectories are written as the same
  `agent_execution.json` / `openhands_trajectory/` shapes the web visualizer renders.

See **[docs/RUST_PORT.md](docs/RUST_PORT.md)** for the module map (Python → Rust),
the native LLM-runner design, parity/benchmarks/evals, and testing seams.

## Documentation

- [docs/RUST_PORT.md](docs/RUST_PORT.md) — Rust port architecture, module map, LLM runners, parity.
- [docs/architecture.md](docs/architecture.md) — overall SIA architecture.
- [docs/configuration.md](docs/configuration.md) — profiles, providers, config.
- [docs/walkthrough.md](docs/walkthrough.md) — end-to-end walkthrough.
- [EVALUATION_GUIDE.md](EVALUATION_GUIDE.md) — tasks, MLE-Bench, the `evaluate.py` contract.
- [docs/troubleshooting.md](docs/troubleshooting.md) — common issues.
