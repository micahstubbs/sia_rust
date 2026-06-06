# sia_rust — Hackathon Demo Script, Judging Narrative & Run-of-Show

---

## 1. Core narrative

**One line:** sia_rust — the fast, safe, native Rust implementation of SIA with live, watchable self-improvement you can run and extend.

SIA (Self-Improving AI) is a peer-reviewed framework in which a Meta-Agent writes a
Target Agent, the Target Agent attempts a task, and a Feedback Agent rewrites the
code for the next generation — an autonomous improvement loop grounded in
[arXiv 2605.27276](https://arxiv.org/abs/2605.27276).
This port brings that loop to native Rust on top of
[`rig-core`](https://crates.io/crates/rig-core): the deterministic orchestration
core runs ~5.8× faster than the Python reference (geometric mean; up to 24.7× on
prompt building, 17.9× on execution-log loading), the native LLM runners
(`claude` / `openhands` / `pydantic-ai`) remove the Python SDK dependency for the
meta/feedback agents entirely, and a formal capability allow-list (`src/sandbox.rs`)
plus optional Docker confinement give the self-modifying loop a principled safety
story.
The SIA Studio dashboard (`sia web`) lets you watch accuracy, token spend, and
telemetry climb in real time — or replay any prior run offline.

---

## 2. 3–4 minute demo script (run-of-show)

> Everything below is timed for a live terminal + browser side-by-side.
> Terminal font: ≥ 18 pt. Browser open to `http://127.0.0.1:8000` before you start.

### Pre-demo setup (done before you walk on stage)

```bash
# 1. Build with LLM runners
cargo build --release --features llm

# 2. Export credentials
export NEBIUS_API_KEY="<your-token-factory-key>"
# Optional (only needed if using default-meta / Claude-backed meta agent):
# export ANTHROPIC_API_KEY="<your-anthropic-key>"

# 3. Warm a runs/ directory (so sia web has something to show immediately)
cargo run --release --features llm -- run \
  --task gpqa \
  --meta-agent-profile kimi-nebius-meta \
  --target-agent-profile kimi-nebius-target \
  --max_gen 3 \
  --run_id 0

# 4. Open browser to http://127.0.0.1:8000 and confirm the dashboard loads
```

---

### Live run-of-show

#### 0:00 – 0:30 · Problem statement

> Say (pointing at blank terminal):
> "AI self-improvement is a core research idea — you iterate on an agent's code
> across generations, each time scoring, feeding back, and rewriting. The Python
> reference implementation exists, but it is slow, hard to observe in real time, and
> has no safety story for a framework that literally rewrites and executes code.
> sia_rust fixes all three."

#### 0:30 – 1:00 · One-command build

```bash
# Show the feature flag — this is the only thing that changes from the lean default
cargo build --release --features llm
```

> Point at: the `Compiling` lines. "One flag — `--features llm` — pulls in the
> native rig-core LLM clients. The default build has zero LLM dependencies."

#### 1:00 – 2:30 · Live self-improvement run

Run in terminal (or run this live if confident; otherwise switch to the pre-warmed
run in the next section):

```bash
cargo run --release --features llm -- run \
  --task gpqa \
  --meta-agent-profile kimi-nebius-meta \
  --target-agent-profile kimi-nebius-target \
  --max_gen 3 \
  --run_id 1
```

> Point at: the structured log lines as each generation starts. Then flip to the
> browser at `http://127.0.0.1:8000`.

> Note: `sia run` starts the SIA Studio dashboard automatically on
> `http://127.0.0.1:8000` unless `--no-web` is passed. You do not need a separate
> `sia web` command during the run.

**What to show on screen while the run is live:**

- The SIA Studio dashboard at `http://127.0.0.1:8000` — generations appear as they
  land, the accuracy chart climbs, and the telemetry panel shows token counts and
  wall-clock timing updating in real time.
- Point at the `/api/runs/run_1/metrics` and `/api/runs/run_1/telemetry` endpoints
  powering the charts (show the JSON in a second tab if you have time).

#### 2:30 – 3:00 · Safety + extensibility story

> "Every tool call the native LLM runners make — Read, Write, Bash, Glob — runs
> through a capability allow-list in `src/sandbox.rs`. Deny-by-default, with a
> single auditable enforcement point. Add `--sandbox docker` and the Python target
> agent gets a container with `--network none`, read-only datasets, and memory/CPU
> caps. The roadmap extends this to kernel-level landlock/seccomp and eventually
> WASI. The threat model is formalized in `SECURITY.md`."

> "Extending the framework means dropping a JSON file in `./profiles/` — no Rust
> code. Here's a Kimi target profile [show the bundled
> `kimi-nebius-target` JSON in an editor]. Point it at any OpenAI-compatible
> endpoint on Nebius and you're running."

#### 3:00 – 3:30 · Results + ask

> "Three generations on GPQA Diamond — with a hosted open-source model on Nebius
> Token Factory, zero local GPU, ~5.8× faster orchestration core than the Python
> reference. The web UI works offline too: `cargo run -- web` replays any prior
> `runs/` directory, so you can demo anywhere.
>
> We're looking for: feedback on the safety story, interest in research
> collaborations on the paper extensions (adaptive harness scheduling, weight
> updates), and introductions to anyone building on top of SIA."

---

## 3. Why it wins (per track)

### Framework Enhancement

- **Native Rust performance.** Deterministic core runs ~5.8× faster than CPython
  (geometric mean, `benchmarks/REPORT.md`): 24.7× on prompt building, 17.9× on
  execution-log loading. No interpreter overhead on the hot path.
- **Capability sandbox.** `src/sandbox.rs` is a pure-`std`, deny-by-default
  capability allow-list (read/write/bash/network/size checks) with `check_bash`
  prefix gating that directly mitigates prompt-injection-driven tool abuse. Formal
  threat model in `SECURITY.md`.
- **SIA Studio.** The Axum-backed `sia web` dashboard (dark mode, telemetry charts,
  metrics timeline, per-generation artifacts) is embedded at build time — no separate
  front-end deploy. Runs offline against any `runs/` directory. Serves the live
  dashboard automatically during `sia run`.
- **Feature-gated LLM layer.** The default build has zero LLM client dependencies;
  `--features llm` adds the full native runner stack. Published crate stays lean.

### Applied

- **Runnable tasks out of the box.** Four bundled tasks ship with the crate: `gpqa`,
  `lawbench`, `longcot-chess`, `spaceship-titanic`. A single `cargo run --features
  llm -- run --task gpqa ...` is the entire demo command.
- **Verifier trait + native evaluation hooks** (`src/verifier.rs`, issue #66, merged):
  `ExactMatch`, `MultipleChoice`, `NumericTolerance`, `Contains` verifiers with
  partial-credit scoring and adversarial-variant / stability hooks for Goodhart
  robustness. Complements the existing Python `evaluate.py` contract. Ships with a
  runnable `arithmetic-mc` example task + `docs/TASK_AUTHORING.md`.
- **Parity harness.** `scripts/parity_check.py` asserts byte-identical output
  between the Rust port and the Python reference over an ASCII + CJK + emoji matrix.
  CI runs it on every push.

### Research (paper extension angles)

All items in this section are **in-progress / roadmap** — not yet merged to `main`:

- **Adaptive harness scheduler (#65 — planned).** The paper's harness-update loop
  implies a scheduler that decides *which* generation's context to feed back. A
  native Rust adaptive scheduler (adjusting the feedback window based on accuracy
  trajectory) is a natural extension; issue #65 tracks it.
- **Native weight updates (#19 as paper section — planned).** The arXiv paper
  discusses model weight updates as a complement to harness updates. The current port
  focuses on harness evolution; native gradient-based weight update support (via
  RLHF/DPO pipelines against the Rust trajectory format) is on the roadmap.
- **Trajectory telemetry as research signal.** Per-generation `telemetry.json`
  artifacts (`input_tokens`, `output_tokens`, `num_api_calls`, `num_tool_calls`,
  `duration_ms`) are machine-readable and structured for analysis. The web API
  (`/api/runs/:run/metrics`) surfaces these as a time-series, ready for downstream
  research tooling.

### Sponsor (Nebius)

- **Bundled Nebius profiles.** Five profiles ship with the crate:
  `kimi-nebius-target` (Kimi K2.6), `kimi-nebius-meta` (Kimi K2.6 on OpenAI-compat
  openhands runner), `qwen-nebius-target` (Qwen3 80B), `gptoss-nebius-target`
  (GPT-OSS 120B), `deepseek-nebius-target` (DeepSeek R1-0528). All route to
  `https://api.tokenfactory.us-central1.nebius.com/v1/`.
- **Single env var.** `export NEBIUS_API_KEY="..."` is the complete setup — no
  custom provider JSON required for the bundled profiles.
- **Token telemetry.** Every generation writes `telemetry.json` with provider-
  reported token counts. Correlate with Nebius Token Factory credit spend via the
  console. No invented dollar-cost fields — just counts + timing.
- **Retry layer.** Exponential-backoff retry (`src/llm/retry.rs`) handles transient
  429s automatically, making long multi-generation runs robust on shared quota.

---

## 4. Backup demo paths (graceful degradation)

### Path A (happy path) — live run on Nebius

```bash
export NEBIUS_API_KEY="..."
cargo run --release --features llm -- run \
  --task gpqa \
  --meta-agent-profile kimi-nebius-meta \
  --target-agent-profile kimi-nebius-target \
  --max_gen 3 \
  --run_id 1
```

Live dashboard auto-starts at `http://127.0.0.1:8000`.

---

### Path B — network/LLM is flaky: replay a pre-recorded `runs/` directory

The visualizer is completely offline. Before the event, copy a good `runs/`
directory to the demo machine:

```bash
# On the demo machine, point sia web at the pre-recorded directory
cargo run --release -- web --runs-dir ./runs-prerecorded
# (or just ./runs if you ran the warm-up)
```

Browse to `http://127.0.0.1:8000`. All charts, telemetry, artifacts, and
per-generation diffs are served from disk — no network, no API key needed.

> What to show: open `run_0` from the warm-up, walk through gens 1–3, show the
> accuracy chart climbing, click into `target_agent.py` for each generation to show
> the literal code changes between generations.

---

### Path C — even the server fails: offline test suite

```bash
# Proves the loop is real — runs all unit + integration tests including
# the LLM-runner mock tests (no network; scripted responses)
cargo test --features llm --all-targets
```

Expected output: all tests pass (green). Point at the test names in the output
— `test_claude_runner_*`, `test_openhands_*`, `test_pydantic_ai_*`,
`test_sandbox_*`, `test_trajectory_middleware_*` — and note that these exercise the
real tool-use loops with offline scripted responses, proving the architecture without
any live API call.

Supplement with screenshots of the SIA Studio dashboard from the pre-recorded run
(capture these during setup and keep them in the `docs/` folder or a slide).

---

## 5. Judging one-pager

**Problem**
Autonomous self-improvement (harness/code evolution across generations) is
well-studied in theory but slow, opaque, and unsafe in practice. The Python reference
has no real-time observability, no principled sandbox for self-modifying code, and
pays the interpreter tax on every generation.

**Approach**
Port SIA to native Rust: same task contracts, same orchestration semantics, byte-
identical prompt/context parity — but with native LLM runners on `rig-core`, a
formal capability allow-list, and a real-time web dashboard built into the binary.

**Why it wins**
- ~5.8× faster deterministic core (geometric mean vs. CPython); up to 24.7×
- Deny-by-default capability sandbox + optional Docker confinement (SECURITY.md)
- Five Nebius-hosted model profiles ship out of the box; one env var to run
- SIA Studio: telemetry + metrics charts + dark mode, zero external deploy
- Feature-gated: `--features llm` for full stack; default build has no LLM deps

**Live demo**
`cargo run --release --features llm -- run --task gpqa --meta-agent-profile kimi-nebius-meta --target-agent-profile kimi-nebius-target --max_gen 3 --run_id 1`
Watch accuracy climb in the SIA Studio dashboard at `http://127.0.0.1:8000`.

**Results**
- All 7 golden-master parity tests pass (byte-identical vs. Python reference)
- 5.8× geometric-mean speedup; prompt building 24.7×; execution-log loading 17.9×
- Full LLM-runner test suite exercises claude / openhands / pydantic-ai offline
- Telemetry: token counts + wall-clock timing per generation, machine-readable

**Future**
Adaptive harness scheduler (#65), native weight updates (paper §4 roadmap), kernel-
level landlock/seccomp sandboxing, WASI component model for fully isolated target
agents.

---

## 6. Q&A prep

### "Why Rust? Python is fine for research."

> The deterministic orchestration core — prompt building, context management,
> execution-log parsing — is ~5.8× faster (geometric mean) with peaks of 24.7×.
> More importantly, Rust's type system lets us express the trust-boundary model
> clearly: the orchestrator is trusted code compiled once; the generated target
> agents are untrusted subprocesses. That distinction is structurally enforced,
> not a documentation convention. See `SECURITY.md` and `benchmarks/REPORT.md`.

### "Is self-modifying code safe to run?"

> We distinguish three trust zones (SECURITY.md §2): trusted orchestrator, semi-
> trusted native runner tools (LLM-driven but our code), and untrusted Python
> target subprocess (machine-written). The capability allow-list (`src/sandbox.rs`)
> gives the tool layer a single deny-by-default enforcement point. `--sandbox
> docker` jails the target subprocess with `--network none`, read-only datasets,
> and memory/CPU caps. The `Bash` tool is the widest hole today — SECURITY.md is
> honest about this — and kernel-level landlock/seccomp is the documented next
> stage. We ship the threat model, not just the happy path.

### "How close is this to the paper's SIA algorithm?"

> The harness-evolution loop (meta → target → feedback → repeat) is implemented
> faithfully. `scripts/parity_check.py` asserts byte-identical prompt/context output
> vs. the Python reference on every CI push. Tasks (gpqa, lawbench, longcot-chess,
> spaceship-titanic) use the same `evaluate.py` scoring contract as the paper's
> experiments. The paper's weight-update component (model fine-tuning) is roadmap —
> labeled explicitly in §3 and §5 of this document.

### "How does the Nebius integration work?"

> Five profiles ship with the crate, pointing at Nebius Token Factory's
> OpenAI-compatible endpoint. `export NEBIUS_API_KEY="..."` is the only setup. The
> `openhands` agent impl (chat-completions loop) drives the meta/feedback agents via
> the native Rust client — no Python SDK, no subprocess. Per-generation
> `telemetry.json` records token counts (not dollar costs — those come from the
> Token Factory console). The retry layer (`src/llm/retry.rs`) handles 429s
> automatically. See `docs/NEBIUS_QUICKSTART.md` for the step-by-step guide.

### "What's the performance number based on?"

> `cargo bench --bench core` (Criterion) vs. `benchmarks/bench_python.py`
> (`time.perf_counter`), byte-identical fixtures, same operations. The 5.8× figure
> is the geometric mean across 9 operations measured on Intel Xeon @ 2.80 GHz /
> Linux 6.18.5. See `benchmarks/REPORT.md` for the full table. LLM/network calls
> are neutralized in both languages — the benchmark isolates the deterministic core.

---

## 7. Pre-demo checklist

### Build

- [ ] `cargo build --release --features llm` completes without errors
- [ ] `cargo test --features llm --all-targets` is green

### Credentials

- [ ] `export NEBIUS_API_KEY="..."` set and tested (`curl` or a quick 1-gen warm-up)
- [ ] (Optional) `export ANTHROPIC_API_KEY="..."` if using the default Claude meta agent

### Warm run (run before you walk on stage)

- [ ] `cargo run --release --features llm -- run --task gpqa --meta-agent-profile kimi-nebius-meta --target-agent-profile kimi-nebius-target --max_gen 3 --run_id 0` completed
- [ ] `runs/run_0/gen_1/`, `gen_2/`, `gen_3/` directories exist with artifacts
- [ ] Copy `runs/` to `runs-prerecorded/` as the Path B fallback

### Dashboard

- [ ] `cargo run --release -- web` starts and `http://127.0.0.1:8000` loads in browser
- [ ] `run_0` visible in the UI; accuracy chart renders; telemetry panel shows data
- [ ] Dark mode confirmed

### Terminal

- [ ] Font size ≥ 18 pt
- [ ] Terminal background dark (contrast for screen)
- [ ] Shell prompt is short (e.g. `$` not a long path)
- [ ] History cleared so only demo commands appear

### Slides / backup

- [ ] Screenshots of the SIA Studio dashboard saved locally
- [ ] `benchmarks/REPORT.md` open in a tab (for the 5.8× question)
- [ ] `SECURITY.md` open in a tab (for the safety question)
- [ ] Fallback: know that `cargo test --features llm` is the offline proof

### Network

- [ ] Demo machine has network access to `api.tokenfactory.us-central1.nebius.com`
- [ ] Test a live `sia run` < 30 min before the session
- [ ] Confirm Path B (`sia web --runs-dir ./runs-prerecorded`) works if network drops

---

*Linked from [README.md](../README.md) and [docs/RUST_PORT.md](RUST_PORT.md).
Companion deck outline + reproducibility one-pager: [docs/HACKATHON_DECK.md](HACKATHON_DECK.md).*
