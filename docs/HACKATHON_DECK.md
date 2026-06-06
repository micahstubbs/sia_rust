# sia_rust — Hackathon Slide Deck Outline & Reproducibility One-Pager

> **Companion to [`docs/HACKATHON_DEMO.md`](HACKATHON_DEMO.md).**
> That document owns the timed run-of-show, per-track judging narrative,
> backup/contingency paths, Q&A prep, and pre-demo checklist. This document
> adds the slide-deck outline (with talking points and a one-liner per slide)
> and a copy-pasteable reproducibility one-pager. Read both before presenting.

---

## Part 1 — Slide deck outline (7 slides)

### Slide 1 — Title / hook

**Title:** `sia_rust` — Self-Improving AI, natively in Rust

**Talking points:**
- Autonomous improvement loop: Meta-Agent writes Target-Agent, Target-Agent
  attempts the task, Feedback-Agent rewrites for the next generation.
- Based on arXiv 2605.27276; this is a faithful Rust port with native LLM
  runners, a real-time dashboard, and a principled sandbox.
- One command to run. One env var to unlock hosted models on Nebius.
- Live demo: watch the loop run in a terminal + browser side-by-side.

**Say this:** "What if you could watch an AI improve its own code, in real
time, in a terminal you can read?"

---

### Slide 2 — Problem

**Title:** Three gaps in self-improving agent frameworks

**Talking points:**
- **Speed.** Python interpreter overhead on every orchestration step means
  long iteration cycles; the hot path (prompt building, context management,
  execution-log loading) is pure computation, not I/O.
- **Observability.** The reference implementation has no real-time dashboard —
  you learn how a run went by reading log files after the fact.
- **Safety.** A framework that literally writes and executes code generation
  after generation has no principled threat model or sandbox in the reference
  implementation.
- These three gaps compound: slow feedback, blind operators, and unrestricted
  self-modifying code running on the host.

**Say this:** "Slow, opaque, and unsafe — we fix all three."

---

### Slide 3 — Our approach

**Title:** Native Rust SIA: orchestrator → LLM layer → Python target bridge

**Talking points:**
- **Deterministic core in Rust.** Orchestrator, context manager, prompt
  building, execution-log loading — compiled once, ~5.8× faster (geometric
  mean vs. CPython; up to 24.7× on prompt building, 17.9× on log loading).
- **Native LLM runners** (`src/llm/`, feature-gated `--features llm`) on
  `rig-core`: `claude` (Anthropic `/v1/messages` tool loop), `openhands`
  (OpenAI-compatible chat-completions), `pydantic-ai` — no Python SDK
  dependency for meta/feedback agents.
- **Python target bridge preserved.** Generated target agents run as Python
  subprocesses via the `evaluate.py` contract — preserving task compatibility
  and the paper's semantics exactly.
- **Capability sandbox.** `src/sandbox.rs` deny-by-default allow-list
  (read/write/bash/network/size checks) + optional `--sandbox docker`
  (container, `--network none`, read-only dataset, cpu/mem caps).

```
sia run ──► orchestrator ──► context_manager ──► results
               │
               ├─ Meta / Feedback agents
               │    └─► native LLM runners  [--features llm]
               │         src/llm/* on rig-core
               │
               └─ Target agent ──► Python subprocess
                                    (optional Docker sandbox)
                                    via evaluate.py contract

sia web ──► Axum visualizer of ./runs   (offline-capable)
```

**Say this:** "The orchestrator is compiled Rust. The meta-agents are native
Rust clients. The generated target agent runs in a Python subprocess, exactly
as the paper intended — with an optional Docker jail around it."

---

### Slide 4 — Live demo

**Title:** SIA Studio: watch self-improvement happen

**Talking points:**
- `sia run` starts the **SIA Studio dashboard** automatically at
  `http://127.0.0.1:8000` — no separate deploy, embedded at build time.
- Dashboard shows: accuracy chart climbing generation by generation, telemetry
  panel (input/output tokens, API calls, tool calls, wall-clock timing), and
  per-generation artifact diffs.
- JSON APIs behind the charts: `/api/runs/<run_id>/metrics`,
  `/api/runs/<run_id>/telemetry` — machine-readable, structured, replayable.
- `sia web` (standalone) replays any `runs/` directory offline — full charts,
  no API key, no network.
- Dark mode. Zero external front-end deployment.

**Say this:** "This is a real run — watch the accuracy number change as each
generation lands."

> **Presenter note:** point at the pre-warmed `run_0` in the dashboard
> while the live `run_1` is in flight. See
> [`HACKATHON_DEMO.md §2`](HACKATHON_DEMO.md) for the exact run-of-show.

---

### Slide 5 — Why it wins / Nebius sponsor

**Title:** Per-track case + Nebius integration

**Talking points:**

*Framework Enhancement track:*
- ~5.8× faster deterministic core (Criterion benchmarks, byte-identical
  fixtures; `benchmarks/REPORT.md`).
- Deny-by-default capability sandbox (`src/sandbox.rs`) + optional Docker
  jail — formal threat model in `SECURITY.md`.
- Feature-gated LLM layer: default build has zero LLM client dependencies;
  `--features llm` adds the full stack. Published crate stays lean.
- `src/verifier.rs` — `ExactMatch`, `MultipleChoice`, `NumericTolerance`,
  `Contains` verifiers with partial-credit scoring and robustness hooks.

*Nebius sponsor:*
- Five bundled profiles ship with the crate: `kimi-nebius-target`,
  `kimi-nebius-meta`, `qwen-nebius-target`, `gptoss-nebius-target`,
  `deepseek-nebius-target`.
- Single env var: `export NEBIUS_API_KEY="..."` is the entire setup.
- Exponential-backoff retry (`src/llm/retry.rs`) handles transient 429s
  automatically for long multi-generation runs.
- Per-generation `telemetry.json`: `input_tokens`, `output_tokens`,
  `num_api_calls`, `num_tool_calls`, `duration_ms` — correlate with Token
  Factory credit spend in the console.

**Say this:** "One env var, five hosted models, and a retry layer that
handles rate limits so a 10-generation run finishes unattended."

---

### Slide 6 — Results / evidence

**Title:** What we have today (no invented numbers)

**Talking points:**
- **Parity:** `scripts/parity_check.py` asserts byte-identical output vs. the
  Python reference on ASCII + CJK + emoji + control-char fixtures. CI runs on
  every push. All 7 golden-master test cases pass.
- **Benchmarks:** geometric-mean 5.8× speedup across 9 deterministic-core
  operations (Criterion vs. `time.perf_counter`; LLM calls neutralized in
  both); peaks: 24.7× prompt building, 17.9× execution-log loading. See
  `benchmarks/REPORT.md` for the full table and methodology.
- **Offline test coverage:** `cargo test --features llm --all-targets` —
  covers the full tool-use loops (`claude`/`openhands`/`pydantic-ai`),
  sandbox policy, trajectory middleware, and retry logic, all with scripted
  offline responses. No live API call required to prove the architecture.
- **Tasks:** four bundled tasks ship (`gpqa`, `lawbench`, `longcot-chess`,
  `spaceship-titanic`); the `evaluate.py` scoring contract is unchanged from
  the paper's experiments.
- **Telemetry:** per-generation machine-readable artifacts (`telemetry.json`,
  `agent_execution.json`) ready for downstream research analysis.

**Say this:** "Every benchmark is reproducible with one command. The parity
gate runs in CI on every push."

---

### Slide 7 — Future work (roadmap)

**Title:** What comes next

> All items on this slide are **roadmap / not yet merged to `main`** unless
> noted otherwise.

**Talking points:**
- **Full meta-RL adaptive scheduler** (issue #65, heuristic version merged):
  closed-loop feedback-window adjustment based on accuracy trajectory — the
  paper's harness-update loop, natively scheduled.
- **GPU LoRA fine-tuning via Candle** (issue #19, CPU reference LoRA merged):
  native gradient-based weight updates against the Rust trajectory format;
  GPU path via `candle-core` is roadmap.
- **WASI sandbox** (roadmap, `SECURITY.md §6`): run generated target agents
  as WebAssembly components under `wasmtime` — capability-based isolation by
  construction, strongest isolation target.
- **Kernel-level landlock/seccomp** (roadmap, `SECURITY.md §6`): OS-enforced
  filesystem and syscall restrictions for the native runner tools; builds
  directly on the existing `src/sandbox.rs` policy layer.
- **MLE-Bench / Kaggle task integration** (roadmap): extend the bundled task
  set to competition benchmarks for research replication.

**Say this:** "The capability allow-list is stage 1 of a 3-stage sandboxing
roadmap. Each stage enforces the same policy at a lower level — from
in-process advisory, to kernel landlock, to WASI components."

---

## Part 2 — Reproducibility one-pager

> Copy-paste the commands below exactly. Every flag is verified against
> `src/cli.rs`; every profile ID is verified against the bundled JSON files.

### Prerequisites

| Requirement | Notes |
|---|---|
| Rust toolchain (stable) | Install from <https://rustup.rs/>; `rustc --version` should show `1.70+` |
| Git | For cloning the repo |
| Python 3.x | Required for the target-agent subprocess (`evaluate.py` contract); 3.9+ recommended |
| Docker (optional) | Only needed for `--sandbox docker` mode |
| Nebius API key | Free tier available at <https://docs.tokenfactory.nebius.com/> |
| Anthropic API key (optional) | Only if using the default `default-meta` Claude profile for the meta agent |

---

### Step 1 — Build

```bash
# Lean build (no LLM dependencies — orchestrator + web UI only)
cargo build

# Full build with native LLM runners
cargo build --features llm
```

The `--features llm` flag gates the `src/llm/` crate (rig-core, HTTP clients,
tool-use loops). The default build has zero LLM client dependencies and is
always what CI checks first.

---

### Step 2 — Set credentials

```bash
# Required for the bundled Nebius profiles
export NEBIUS_API_KEY="your-nebius-api-key-here"

# Optional: only if using the default Claude-backed meta agent (default-meta)
export ANTHROPIC_API_KEY="your-anthropic-api-key-here"
```

To use Kimi as both meta and target agent (all traffic on Nebius, no
Anthropic key needed):

```bash
export NEBIUS_API_KEY="your-nebius-api-key-here"
```

See [`docs/NEBIUS_QUICKSTART.md`](NEBIUS_QUICKSTART.md) for the full
credential setup guide.

---

### Step 3 — Run a self-improvement loop

The canonical demo command (Kimi on Nebius as both meta and target agent):

```bash
cargo run --features llm -- run \
  --meta-agent-profile kimi-nebius-meta \
  --target-agent-profile kimi-nebius-target \
  --task gpqa \
  --max_gen 3 \
  --run_id 1
```

**All flags explained:**

| Flag | Value | Meaning |
|---|---|---|
| `--features llm` | (cargo flag) | Include native LLM runners |
| `--meta-agent-profile` | `kimi-nebius-meta` | Meta/feedback agent: Kimi K2.6 via OpenHands runner on Nebius |
| `--target-agent-profile` | `kimi-nebius-target` | Target agent profile: Kimi K2.6 on Nebius |
| `--task` | `gpqa` | Bundled task (`gpqa`, `lawbench`, `longcot-chess`, `spaceship-titanic`) |
| `--max_gen` | `3` | Number of improvement generations to run |
| `--run_id` | `1` | Identifier for this run; outputs land in `runs/run_1/` |

**Optional flags:**

```bash
# Docker sandbox for the Python target agent
--sandbox docker

# Suppress the auto-started web dashboard
--no-web

# Custom port for the dashboard (default: 8000)
--web-port 9000

# Use a specific run output root
# (there is no --runs-dir flag for `run`; use the SIA_RUNS_DIR env var if needed)
```

**To run the `arithmetic-mc` example task** (ships with the verifier examples):

```bash
cargo run --features llm -- run \
  --meta-agent-profile kimi-nebius-meta \
  --target-agent-profile kimi-nebius-target \
  --task arithmetic-mc \
  --max_gen 3 \
  --run_id 2
```

> **Note on `--task` values:** Only the bundled task names are accepted by the
> `--task` flag. Pass `--task_dir ./path/to/my-task` to use an external task
> directory.

---

### Step 4 — View in SIA Studio

The dashboard starts automatically at `http://127.0.0.1:8000` when `sia run`
is running (suppress with `--no-web`).

To replay any prior run offline (no API key, no network):

```bash
cargo run -- web --runs-dir ./runs
# or point at a pre-recorded directory:
cargo run -- web --runs-dir ./runs-prerecorded
```

Browse to `http://127.0.0.1:8000`. Full charts, telemetry, and per-generation
artifact diffs are served from disk.

---

### Step 5 — Expected runtime guidance

- **Build time** (`cargo build --features llm`, cold): several minutes on
  first run (dependency compilation); subsequent incremental builds are fast.
- **A generation** is the time for one meta → target → feedback cycle.
  Wall-clock time depends primarily on the LLM provider's latency and the
  task's evaluation script. For a cloud-hosted model (e.g. Kimi on Nebius),
  a typical generation on `gpqa` with `--max_gen 3` is bounded by network
  round-trips to the provider plus the Python subprocess runtime — expect
  minutes per generation, not seconds. The deterministic orchestration core
  (prompt building, context management, log loading) contributes negligibly
  (~milliseconds) compared to LLM latency.
- **Token telemetry** in `runs/run_<id>/gen_<n>/telemetry.json` records
  `duration_ms` per generation so you can observe actual timing after the
  first warm-up run.

> We do not publish precise per-generation second estimates because they vary
> significantly with model, provider load, task, and network. Use `telemetry.json`
> from a warm-up run to calibrate expectations for your environment.

---

### Step 6 — Where outputs land

```
runs/
  run_<run_id>/
    gen_1/
      target_agent.py          # generated target-agent code for this generation
      agent_execution.json     # trajectory: tool calls, model responses, timing
      telemetry.json           # token counts, API calls, tool calls, duration_ms
      results.json             # task score for this generation
    gen_2/
      ...
    gen_3/
      ...
```

The SIA Studio dashboard and `/api/runs/<run_id>/metrics` +
`/api/runs/<run_id>/telemetry` endpoints read these files directly — no
database, no separate ingestion step.

---

### Step 7 — Offline fallback

No API key? No network? Prove the architecture with the offline test suite:

```bash
# Full suite including LLM-runner mock tests (no live API calls)
cargo test --features llm --all-targets
```

This exercises:
- `claude` / `openhands` / `pydantic-ai` tool-use loops (scripted responses)
- Sandbox policy (`check_read`, `check_write`, `check_bash`, path escape
  prevention)
- Trajectory middleware (event capture, token accounting)
- Retry logic
- Orchestrator branching (injectable seams — no real Python subprocess)
- Golden-master parity (byte-identical prompt/context vs. Python reference)

Expected: all tests pass (green). See
[`HACKATHON_DEMO.md §4 Path C`](HACKATHON_DEMO.md) for the run-of-show
version of this fallback.

To replay a pre-recorded `runs/` directory in the dashboard (also offline):

```bash
cargo run -- web --runs-dir ./runs-prerecorded
```

---

### Reference links

- Nebius Token Factory quickstart: [`docs/NEBIUS_QUICKSTART.md`](NEBIUS_QUICKSTART.md)
- Formal threat model: [`SECURITY.md`](../SECURITY.md)
- Benchmark methodology and full table: `benchmarks/REPORT.md`
- Demo run-of-show and backup paths: [`docs/HACKATHON_DEMO.md`](HACKATHON_DEMO.md)

---

## Part 3 — Assets to capture before the demo

> This checklist covers media assets (screenshots, recordings, warmed data)
> that complement the build/credential/network checks in
> [`HACKATHON_DEMO.md §7`](HACKATHON_DEMO.md). Do not duplicate those checks
> here — do both.

### SIA Studio screenshots

- [ ] Dashboard home page showing the run list (dark mode, at least one run)
- [ ] Accuracy chart for a completed run (all `--max_gen` generations visible,
      ideally with an upward trend)
- [ ] Telemetry panel expanded (token counts, `duration_ms` visible)
- [ ] Per-generation artifact diff view (show `target_agent.py` changing
      between `gen_1` and `gen_2`)
- [ ] `/api/runs/run_0/metrics` JSON in a browser tab (shows the raw data
      powering the chart)

Save screenshots to `docs/screenshots/` or a slide. They are your Slide 4
backup if the live run is slow.

### Warmed `runs/` directory

- [ ] Complete a `--max_gen 3` warm-up run before the event and keep the
      output in `runs/run_0/`
- [ ] Copy to `runs-prerecorded/` so `sia web --runs-dir ./runs-prerecorded`
      works without overwriting it:
      ```bash
      cp -r runs/run_0 runs-prerecorded/run_0
      ```
- [ ] Verify that `runs-prerecorded/run_0/gen_3/results.json` exists and the
      accuracy chart renders in the dashboard

### Terminal recording

- [ ] Record an `asciinema` (or equivalent) of the `sia run` command producing
      structured log output and the first generation landing
- [ ] Trim to ≤ 60 seconds; loop it on a second monitor if available
- [ ] Keep the recording file locally — it is the slide-deck substitute for
      the live terminal if you are presenting from a PDF

### Slide backup images

- [ ] Export Slide 3 architecture diagram as a PNG and keep it in `docs/`
      (reference the ASCII diagram in this file or redraw it in your slide tool)
- [ ] Capture `benchmarks/REPORT.md` as a table image for the results slide
- [ ] Keep `SECURITY.md §5` and `§6` open in a browser tab for the sandbox Q&A

---

*Linked from [README.md](../README.md) and
[docs/HACKATHON_DEMO.md](HACKATHON_DEMO.md).*
