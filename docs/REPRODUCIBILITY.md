# Reproducibility standards for sia_rust

This is the authoritative definition of what a **reproducible SIA run** is in this
repository. It serves two audiences:

1. **Hackathon judges** — to verify and trust a demo run quickly.
2. **Academic reviewers / readers of the preprint** ([`docs/paper/sia_rust_preprint.md`](paper/sia_rust_preprint.md), issue #70).

Every filename, field, flag, and command below is grounded in the artifacts this
repo actually produces — no invented files or fields. The source of truth for
each is cited inline.

> **Honest scope.** The deterministic core is byte-parity-tested against the
> Python reference (Section 3); a real end-to-end self-improvement run is **not
> yet validated** (issue #93). LLM sampling means live runs are **not
> bit-reproducible** — see [Non-determinism](#non-determinism).

Cross-references: the copy-pasteable command walkthrough lives in the
[reproducibility one-pager](HACKATHON_DECK.md#part-2--reproducibility-one-pager)
(`docs/HACKATHON_DECK.md`); credentials in [`docs/CREDENTIALS.md`](CREDENTIALS.md);
model-slug provenance in [`docs/NEBIUS_MODELS.md`](NEBIUS_MODELS.md); parity /
benchmark / eval methodology in [`docs/RUST_PORT.md`](RUST_PORT.md).

---

## 1. What counts as a reproducible run

A run lives under `runs/run_<run_id>/` (built by `RunLayout`, `src/layout.rs`),
with one `gen_<n>/` subdirectory per generation. A run is **complete and
reproducible** when it carries the per-run and per-generation artifacts below.
Most per-generation artifacts are best-effort observability; the **required**
ones for a meaningful run are marked.

### Per-run artifacts (in `runs/run_<id>/`)

| File | Required | One-line schema | Verified against |
|---|---|---|---|
| `context.md` | yes | Markdown. Leading `**Key**: value` metadata block (Task, Meta Model, Task Model, Agent impl, Started, Max Generations) + one `## Generation N` section each + `## Summary Statistics`. | `src/context_manager.rs` (`initialize`/`format_generation_entry`/`finalize`); parsed by `parse_context_md` in `src/web/runs.rs` |
| `profiles.json` | yes | `{ "meta": <MetaAgentProfile>, "target": <TargetAgentProfile> }` — the resolved profiles (incl. `profile_id`, `model`, `provider_id`). | `src/run_setup.rs::write_run_profiles`; read by `get_run` in `src/web/runs.rs` |
| `venv/` | — | Python virtualenv used to run the target agent. | `RunLayout::venv_dir`, `src/layout.rs` |

### Per-generation artifacts (in `runs/run_<id>/gen_<n>/`)

| File / dir | Required | One-line schema | Verified against |
|---|---|---|---|
| `target_agent.py` | yes | The generated target-agent source for this generation. | `names::TARGET_AGENT`, `src/layout.rs`; surfaced in `TEXT_ARTIFACTS`, `src/web/runs.rs` |
| `agent_execution.json` | yes (single-traj) | JSON array of chat turns `[{role, content}, …]` (`content` is a string or content-block array). The trajectory. | `names::AGENT_EXECUTION_JSON`, `src/layout.rs`; loaded in `src/closed_loop.rs::load_trajectory`; `AgentTrajectory` in `src/llm/trajectory.rs` |
| `agent_execution/execution_q*.json` | yes (multi-traj) | Per-question trajectories (one `execution_q<qid>.json` per question) — the multi-trajectory alternative to the single file above. | `names::AGENT_EXECUTION_DIR` / `EXECUTION_GLOB_PREFIX`, `src/layout.rs`; `trajectory_ids` / `get_trajectory`, `src/web/runs.rs` |
| `openhands_trajectory/<session>/events/event-*.json` | when openhands runner | Per-session OpenHands event objects (one JSON object per `*.json` event file). | `list_openhands_sessions` / `get_openhands_events`, `src/web/runs.rs` |
| `results.json` *or* `evaluation_results.json` | yes | Task score. Reader prefers `evaluation_results.json` then `results.json`; fields read: `accuracy_percent`, `accuracy`, `correct`, `incorrect`, `missing`, `invalid`, `total_questions`/`total`, and a `details[]` array of per-question rows (`domain`, `is_correct`). | `EVAL_RESULT_NAMES` + `eval_summary` in `src/web/runs.rs`; `names::RESULTS_JSON`, `src/layout.rs` |
| `improvement.md` | yes (gen ≥ 2) | Markdown rationale for this generation's change; bullet/numbered insights are mined into `context.md`. | `names::IMPROVEMENT_MD`, `src/layout.rs`; `extract_insights`, `src/context_manager.rs` |
| `telemetry.json` | recommended | `{ "generations": [GenerationTelemetry…], "cumulative": GenerationTelemetry }` where each entry is `{generation, input_tokens, output_tokens, num_api_calls, num_tool_calls, duration_ms}`. **No dollar-cost field** (per-provider pricing unknown). | `TELEMETRY_JSON` + `GenerationTelemetry` in `src/llm/telemetry.rs`; read by `get_generation_telemetry`, `src/web/runs.rs` |
| `scheduler_decision.json` | optional (closed-loop) | `{ generation, decision, recommended_next, rationale, harness_efficiency, weight_efficiency, harness_plateaued }`. | `SCHEDULER_DECISION_JSON` + `record_scheduler_decision`, `src/closed_loop.rs`; read by `get_scheduler_decision`, `src/web/runs.rs` |
| `weight_update.json` | optional (closed-loop) | `{ generation, kind, updater, num_examples, loss_before, loss_after, updated, details }`. Written only on a `weight`/`both` decision. | `WEIGHT_UPDATE_JSON` + `maybe_run_weight_update`, `src/closed_loop.rs`; read by `get_weight_update`, `src/web/runs.rs` |
| `target_agent_stdout.log` | recommended | Raw stdout of the target-agent subprocess (a metrics fallback when `results.json` is absent). | `names::STDOUT_LOG`, `src/layout.rs`; `parse_stdout_metrics`, `src/context_manager.rs` |
| `evaluation.log` | optional | Evaluator log. | `names::EVAL_LOG`, `src/layout.rs`; `TEXT_ARTIFACTS`, `src/web/runs.rs` |
| `meta_agent_prompt.txt` | optional | The meta-agent prompt for this generation. | `names::META_PROMPT`, `src/layout.rs` |

> The scheduler/weight modules are **not yet wired into the orchestrator**
> (preprint §5.4 / §6.1); their artifacts appear only when the closed-loop path
> (`src/closed_loop.rs`, issue #84) is exercised.

---

## 2. Mandatory metadata

To make a run interpretable and citable, report (and capture in the bundle) the
following. All of it is derivable from the artifacts above plus the command line.

- [ ] **Model slug(s)** — the exact `model` field of each profile (e.g.
  `moonshotai/Kimi-K2.6`), captured in `profiles.json`. Slug provenance and
  live-catalog verification: [`docs/NEBIUS_MODELS.md`](NEBIUS_MODELS.md).
- [ ] **Provider + `client_kind`** — each profile's `provider_id` resolves to a
  bundled provider JSON carrying `client_kind` (`anthropic` | `openai` |
  `google`), `base_url`, and `api_key_env` (see `sia/defaults/providers/*.json`
  and [`docs/CREDENTIALS.md`](CREDENTIALS.md)).
- [ ] **Bundled-profile id(s)** — the `--meta-agent-profile` /
  `--target-agent-profile` values (e.g. `default-meta` / `default-target`, or a
  Nebius profile). Defaults are `default-meta` / `default-target`
  (`src/config.rs`).
- [ ] **Git commit** — `git rev-parse HEAD`. Not auto-recorded by the run; record
  it alongside the bundle.
- [ ] **Timestamps** — captured per generation: `context.md` carries `Started`
  and a per-generation `**Timestamp**` / `**Duration**`
  (`GenData.timestamp`/`duration`, `src/context_manager.rs`); `telemetry.json`
  carries `duration_ms`.
- [ ] **`--max_gen` and `--run_id`** — the generation count and run identifier
  (`src/cli.rs`); `Max Generations` is echoed into `context.md`.
- [ ] **Build features** — whether `--features llm` was used (required for any
  live LLM call; the default build's runners return a *"build with `--features
  llm`"* message — see [`docs/RUST_PORT.md`](RUST_PORT.md)).

### Non-determinism

- **No random-seed field exists.** The orchestrator does **not** set or record a
  sampling seed, and the artifacts carry no `seed` field — so do not claim one.
- **Live runs are not bit-reproducible.** LLM sampling makes model outputs (and
  therefore `target_agent.py`, trajectories, and scores) vary run-to-run even
  with identical inputs.
- **What "reproducible" means here:** (a) the same *artifact set and schema* is
  produced for a given command, and (b) the **deterministic core** (prompt
  building, `context.md`, feedback-context, execution-log loading, JSON
  serialization) is **byte-parity-tested** against the Python reference — see
  Section 3 and preprint §5.1. Reproducibility is about the verifiable scaffold
  and re-derivable artifacts, **not** identical model text.

---

## 3. Third-party verification in under 5 minutes

Two distinct claims, verified separately:

### A. Verify the implementation (offline, **no API keys**)

```bash
cargo fmt --all -- --check          # formatting gate
cargo build                          # lean build (no LLM deps)
cargo test --all-targets             # full offline suite (unit + integration + golden)
cargo test --all-targets --features llm   # + native runner / middleware tests (mocked, offline)
```

Differential-parity gate (byte-identical Python ⇄ Rust core — the `parity` CI
job in `.github/workflows/rust.yml`):

```bash
cargo build --bin sia-parity
python scripts/parity_check.py       # asserts byte-identical output across an ASCII+CJK+emoji+control matrix
```

Eyeball the artifacts in the dashboard (offline, served straight from disk):

```bash
cargo run -- web --runs-dir ./runs   # then open http://127.0.0.1:8000
```

Open a generation and inspect the **Telemetry** and **Scheduler** panels (backed
by `/api/runs/<run>/telemetry`, `/api/runs/<run>/gens/<gen>/scheduler`, and
`/api/runs/<run>/gens/<gen>/weights`; routes in `src/web/server.rs`). Token/score
charts come from `/api/runs/<run>/metrics`.

### B. Reproduce a live run (**needs keys**, `--features llm`)

Requires the provider API key(s) per [`docs/CREDENTIALS.md`](CREDENTIALS.md):

```bash
export NEBIUS_API_KEY="…"            # and/or ANTHROPIC_API_KEY for a Claude meta agent
cargo run --features llm -- run \
  --meta-agent-profile kimi-nebius-meta \
  --target-agent-profile kimi-nebius-target \
  --task gpqa --max_gen 3 --run_id 1
```

This populates `runs/run_1/gen_*/` with the Section-1 artifacts. Because of
LLM sampling, outputs will differ between live runs (see
[Non-determinism](#non-determinism)). The full step-by-step is the
[reproducibility one-pager](HACKATHON_DECK.md#part-2--reproducibility-one-pager).

---

## 4. Academic reproducibility checklist (for the #70 preprint)

For the preprint's reproducibility statement, report:

- [ ] **Git commit** (`git rev-parse HEAD`) of the exact tree.
- [ ] **Profiles used** — meta + target `profile_id`s and the `--features llm`
  build flag.
- [ ] **Models** — the resolved `model` slug for each profile, with provider /
  `client_kind` and slug-verification status ([`docs/NEBIUS_MODELS.md`](NEBIUS_MODELS.md)).
- [ ] **Task** — `--task <name>` (or `--task_dir`), one of the bundled tasks
  (`gpqa`, `lawbench`, `longcot-chess`, `spaceship-titanic`).
- [ ] **Generation count** — `--max_gen` and `--run_id`.
- [ ] **Artifact bundle** — the `runs/run_<id>/` tree (Section 5), including
  `profiles.json`, `context.md`, and per-generation
  `results.json`/`evaluation_results.json` + `telemetry.json` + trajectory.
- [ ] **Parity + benchmark evidence** — `scripts/parity_check.py` (byte parity,
  §5.1), `benchmarks/REPORT.md` (microbenchmarks, §5.2), and the offline test
  suite (§5.3).
- [ ] **Non-determinism caveat** — state explicitly that live model outputs are
  not bit-reproducible and that **no random seed is set or recorded**.

**Not yet validated — mark as such:**

- A **live end-to-end self-improvement run** has never been executed; all live
  paths are `#[ignore]`-gated (**issue #93**). The preprint says no live
  accuracy study is reported (§5.4).
- The **scheduler and weight-update** modules are tested in isolation and are
  **not wired into the orchestrator** (preprint §5.4 / §6.1); their artifacts are
  not produced by a default `sia run`.
- Some bundled **Nebius model slugs need a live `/v1/models` check** (the Kimi
  `K2.6` slug in particular) — [`docs/NEBIUS_MODELS.md`](NEBIUS_MODELS.md) §3.

---

## 5. How the run artifact bundle is structured

The reproducibility export is simply the run directory tree (no separate
ingestion step):

```
runs/
  run_<run_id>/
    context.md                 # metadata block + per-generation evolution + summary
    profiles.json              # resolved { meta, target } profiles (model, provider_id, …)
    gen_1/
      target_agent.py          # generated target-agent code
      agent_execution.json     # trajectory (single)   — OR —
      agent_execution/         #   execution_q*.json    (multi-trajectory)
      openhands_trajectory/<session>/events/event-*.json   # openhands events (if used)
      improvement.md           # change rationale (gen ≥ 2)
      results.json | evaluation_results.json               # task score (+ details[])
      telemetry.json           # tokens / api calls / tool calls / duration_ms (no $ cost)
      scheduler_decision.json  # closed-loop decision (optional, #84)
      weight_update.json       # closed-loop weight update (optional, #84)
      target_agent_stdout.log  # raw target-agent stdout
    gen_2/ …
    gen_3/ …
```

`sia web` (`cargo run -- web --runs-dir ./runs`) renders this tree directly:
`src/web/runs.rs` reads each `run_<id>` / `gen_<n>` directory and serves it over
the `/api/runs/...` routes in `src/web/server.rs` (run list, per-run detail,
per-generation eval/artifacts/trajectories/openhands events, plus the telemetry,
metrics, scheduler, and weight endpoints). No database is involved — the
on-disk bundle **is** the export.
