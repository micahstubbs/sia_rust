# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Shared Project Instructions

@AGENTS.md

## What this repo is

A **Rust port** of the Python `sia` (Self-Improving AI) framework. Both
implementations coexist on purpose: `src/` is the Rust port, `sia/` is the
original Python package, and the two are held to **byte-for-byte parity** on every
deterministic surface (prompts, `context.md`, feedback context, JSON serialization)
via a CI gate. When changing anything that produces those outputs, parity is the
contract — see "Parity" below.

The self-improvement loop: a **Meta-Agent** writes/improves a target agent, the
**Target-Agent** runs the task as a real Python subprocess (the `evaluate.py`
contract, optionally Docker-sandboxed), and a **Feedback-Agent** analyzes the
trajectory and proposes the next improvement. Meta/feedback agents have native Rust
LLM runners behind the optional `llm` feature; target agents always run as Python
subprocesses.

## Build, test, run

```bash
cargo build                      # lean default build (no LLM client deps)
cargo build --features llm       # include native rig-core LLM runners
cargo test                       # full suite (unit + integration + golden parity)
cargo test --features llm        # also LLM-runner / middleware tests (offline)
cargo run -- web                 # serve the runs visualizer (./runs by default)
cargo run -- --help             # CLI help (run / web sub-commands)

# Single test / filter (standard cargo):
cargo test <name_substring>                         # e.g. cargo test context_golden
cargo test --test orchestrator                       # one integration test file
cargo test --features llm --test end_to_end_llm      # llm-gated integration file

cargo fmt --all -- --check       # CI requires clean fmt
cargo clippy --all-targets -- -D warnings            # CI requires zero warnings
cargo clippy --all-targets --features llm -- -D warnings
```

CI (`.github/workflows/rust.yml`) runs fmt, clippy, and `cargo test` for **both**
the default and `--features llm` builds, plus the parity gate and the standalone
`evals/` crate. Match that locally before pushing.

### Parity gate (cross-language)

```bash
cargo build --bin sia-parity     # builds target/debug/sia-parity (helper)
python scripts/parity_check.py   # diffs Rust vs Python; exits non-zero on any diff
```

`sia-parity` emits the Rust output for an operation given a JSON request on stdin;
`parity_check.py` runs the Python reference on the same inputs and asserts
byte-identical results across an ASCII + CJK + emoji + control-char matrix. The
`src/pyjson.rs` serializer must reproduce CPython's
`json.dumps(..., ensure_ascii=True)` exactly. If you touch prompts, context
building, or JSON output, run this.

### Evals crate (standalone)

```bash
cargo test --manifest-path evals/Cargo.toml          # GPQA-style harness, offline mock
```

`evals/` is a separate crate built on `dspy-rs`. It runs fully offline in CI; see
`evals/README.md`.

### Python side

```bash
python -m pytest tests/ -v       # Python reference test suite
ruff check sia/ tests/ && ruff format --check sia/ tests/   # lint + format
```

## Architecture

The Rust module layout in `src/` mirrors the Python package `sia/` one-to-one
(`config.rs`↔`config.py`, `orchestrator.rs`+`run.rs`↔`orchestrator.py`,
`web/`↔`web/`, etc.). The full Python→Rust module map, native-LLM-runner design,
and testing seams are in **[docs/RUST_PORT.md](docs/RUST_PORT.md)** — read it before
non-trivial work.

Key pieces:

- **Orchestration** (`orchestrator.rs`, `run.rs`, `run_setup.rs`,
  `scheduler.rs`, `closed_loop.rs`): the generation loop — task resolution,
  profile/provider loading, run-directory + venv setup, prompt building,
  target-agent subprocess execution, evaluation, context tracking. Branching logic
  is unit-tested through injectable seams (`run_evaluation_with`,
  `run_target_agent_with`, `run_generation_with`) instead of spawning a real
  interpreter.
- **Prompt / context** (`prompts.rs`, `context_manager.rs`): the byte-for-byte
  surfaces. Golden masters live in `tests/golden/` and are checked from
  `tests/context_golden.rs`, `tests/feedback_context_golden.rs`,
  `tests/prompts_snapshot.rs`.
- **Config & providers** (`config.rs`, `config_files.rs`, `providers.rs`,
  `profiles.rs`, `api_keys.rs`, `env_file.rs`): bundled provider/profile JSON is
  embedded at build time via `include_dir`. `.env` is loaded at startup (real env
  vars win); see [docs/CREDENTIALS.md](docs/CREDENTIALS.md).
- **Agent registry** (`agent_impls/`): `claude`, `openhands`, `pydantic_ai`
  registration, dispatch, and `resolve_model`. This logic is **shared and
  identical** across default and `llm` builds.
- **Native LLM runners** (`src/llm/`, feature `llm`): the actual agentic tool-use
  loops on `rig-core`/HTTP. Every loop is driven through an **injectable transport**
  (`MessagesTransport`, `ChatTransport`), so the full loops are tested offline with
  scripted responses; real-provider tests are `#[ignore]`d and gated on API keys.
  Without `--features llm`, the runners return a clear "build with `--features llm`"
  message and everything else still works.
- **Sandbox** (`sandbox.rs`, on the **default** build): a pure-`std` capability
  allow-list (`Capabilities` + `check_read`/`check_write`/`check_bash`/…),
  deny-by-default — the single auditable enforcement point native tool executors
  consult. Threat model in [SECURITY.md](SECURITY.md).
- **Web visualizer** (`web/`): an `axum`/`tokio` server rendering the `runs/`
  directory. `sia web` is fully functional end-to-end.
- **Serialization helpers** (`pyjson.rs`, `pyfmt.rs`): CPython-compatible JSON /
  formatting — the foundation of the parity gate. Do not "simplify" these toward
  idiomatic serde output; they intentionally match Python.

### Conventions specific to this port

- Feature gating: keep the default build dependency-light. New LLM/network code
  goes behind `feature = "llm"`; the registry/dispatch stays shared.
- When you add a deterministic output surface, add a golden/parity test for it and
  wire the Python reference into `scripts/parity_check.py`.
- Tasks (`tasks/<name>/data/{public,private}/`, `evaluate.py` contract) are
  unchanged from the Python version — see [EVALUATION_GUIDE.md](EVALUATION_GUIDE.md)
  and [docs/TASK_AUTHORING.md](docs/TASK_AUTHORING.md).

### Using bv as an AI sidecar

bv is a graph-aware triage engine for Beads projects (.beads/beads.jsonl). Instead of parsing JSONL or hallucinating graph traversal, use robot flags for deterministic, dependency-aware outputs with precomputed metrics (PageRank, betweenness, critical path, cycles, HITS, eigenvector, k-core).

**Scope boundary:** bv handles *what to work on* (triage, priority, planning). For agent-to-agent coordination (messaging, work claiming, file reservations), use [MCP Agent Mail](https://github.com/Dicklesworthstone/mcp_agent_mail).

**⚠️ CRITICAL: Use ONLY `--robot-*` flags. Bare `bv` launches an interactive TUI that blocks your session.**

#### The Workflow: Start With Triage

**`bv --robot-triage` is your single entry point.** It returns everything you need in one call:
- `quick_ref`: at-a-glance counts + top 3 picks
- `recommendations`: ranked actionable items with scores, reasons, unblock info
- `quick_wins`: low-effort high-impact items
- `blockers_to_clear`: items that unblock the most downstream work
- `project_health`: status/type/priority distributions, graph metrics
- `commands`: copy-paste shell commands for next steps

bv --robot-triage        # THE MEGA-COMMAND: start here
bv --robot-next          # Minimal: just the single top pick + claim command

#### Other Commands

**Planning:**
| Command | Returns |
|---------|---------|
| `--robot-plan` | Parallel execution tracks with `unblocks` lists |
| `--robot-priority` | Priority misalignment detection with confidence |

**Graph Analysis:**
| Command | Returns |
|---------|---------|
| `--robot-insights` | Full metrics: PageRank, betweenness, HITS (hubs/authorities), eigenvector, critical path, cycles, k-core, articulation points, slack |
| `--robot-label-health` | Per-label health: `health_level` (healthy\|warning\|critical), `velocity_score`, `staleness`, `blocked_count` |
| `--robot-label-flow` | Cross-label dependency: `flow_matrix`, `dependencies`, `bottleneck_labels` |
| `--robot-label-attention [--attention-limit=N]` | Attention-ranked labels by: (pagerank × staleness × block_impact) / velocity |

**History & Change Tracking:**
| Command | Returns |
|---------|---------|
| `--robot-history` | Bead-to-commit correlations: `stats`, `histories` (per-bead events/commits/milestones), `commit_index` |
| `--robot-diff --diff-since <ref>` | Changes since ref: new/closed/modified issues, cycles introduced/resolved |

**Other Commands:**
| Command | Returns |
|---------|---------|
| `--robot-burndown <sprint>` | Sprint burndown, scope changes, at-risk items |
| `--robot-forecast <id\|all>` | ETA predictions with dependency-aware scheduling |
| `--robot-alerts` | Stale issues, blocking cascades, priority mismatches |
| `--robot-suggest` | Hygiene: duplicates, missing deps, label suggestions, cycle breaks |
| `--robot-graph [--graph-format=json\|dot\|mermaid]` | Dependency graph export |
| `--export-graph <file.html>` | Self-contained interactive HTML visualization |

#### Scoping & Filtering

bv --robot-plan --label backend              # Scope to label's subgraph
bv --robot-insights --as-of HEAD~30          # Historical point-in-time
bv --recipe actionable --robot-plan          # Pre-filter: ready to work (no blockers)
bv --recipe high-impact --robot-triage       # Pre-filter: top PageRank scores
bv --robot-triage --robot-triage-by-track    # Group by parallel work streams
bv --robot-triage --robot-triage-by-label    # Group by domain

#### Understanding Robot Output

**All robot JSON includes:**
- `data_hash` — Fingerprint of source beads.jsonl (verify consistency across calls)
- `status` — Per-metric state: `computed|approx|timeout|skipped` + elapsed ms
- `as_of` / `as_of_commit` — Present when using `--as-of`; contains ref and resolved SHA

**Two-phase analysis:**
- **Phase 1 (instant):** degree, topo sort, density — always available immediately
- **Phase 2 (async, 500ms timeout):** PageRank, betweenness, HITS, eigenvector, cycles — check `status` flags

**For large graphs (>500 nodes):** Some metrics may be approximated or skipped. Always check `status`.

#### jq Quick Reference

bv --robot-triage | jq '.quick_ref'                        # At-a-glance summary
bv --robot-triage | jq '.recommendations[0]'               # Top recommendation
bv --robot-plan | jq '.plan.summary.highest_impact'        # Best unblock target
bv --robot-insights | jq '.status'                         # Check metric readiness
bv --robot-insights | jq '.Cycles'                         # Circular deps (must fix!)
bv --robot-label-health | jq '.results.labels[] | select(.health_level == "critical")'

**Performance:** Phase 1 instant, Phase 2 async (500ms timeout). Prefer `--robot-plan` over `--robot-insights` when speed matters. Results cached by data hash.

Use bv instead of parsing beads.jsonl—it computes PageRank, critical paths, cycles, and parallel tracks deterministically.
