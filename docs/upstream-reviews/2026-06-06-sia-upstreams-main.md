# Upstream Review: sia_rust and hexo-ai/sia main

**Date:** 2026-06-06
**Compare refs:**
- `micahstubbs/sia_rust`: local branch before sync through upstream `0c87518`
- `hexo-ai/sia`: `4b4877f61be62c3d3c707e077b1c0b65fbc8bb67..38250cb18df6cac58359ca5876c61095fa96f17b`
**Status:** reviewed 5 candidate commits across both upstreams

## Context

This repository is the Rust port. The Python package remains in-tree for parity and packaging support, but new runtime direction favors Rust-native orchestration, scheduler, provider/profile resolution, and weight-update seams. Upstream commits were adopted only when they improved that direction or were safe declarative configuration.

## Commit Decisions

### 0c87518 — Fix review finding bugs
**Decision: CHERRY-PICK**

Adopted via the Rust fork history. This commit fixes concrete review findings in the current direction: evaluator and target-agent timeout handling, concurrent pipe draining for native Bash tool output, generated requirements/reference-copy error propagation, bounded web artifact reads, and context summaries without `-inf` best-performance output. These are reliability and hardening fixes for the Rust port and its Python parity layer.

Local note: the `.beads/issues.jsonl` conflict was resolved from the local `br` database export so local tracker state stayed authoritative.

### 2b49905 — feat: add focus argument for RL-based tuning and integrate RL Integration Guide and orchestration (#24)
**Decision: IGNORE**

The high-level harness-vs-weights direction is relevant, but this implementation is a Python-specific training pipeline with `--focus weights`, `train.py` prompt generation, Tinker Cookbook dependencies, and Modal/SandboxFusion assumptions. The Rust port already has native scheduler and weight-update seams in `src/scheduler.rs`, `src/weights.rs`, and `src/closed_loop.rs`, with follow-up work tracked by `sia_rust-wen` and `sia_rust-grq`. Directly importing this commit would add a second Python-only orchestration path and conflict with the native-Rust preferred direction.

### b3eca6c — feat: add new profiles and provider configuration for Gemini, GPT OSS, and Qwen3 on Tinker (#25)
**Decision: REWRITE AND APPLY**

Applied the safe declarative portion: added bundled Tinker provider/profile JSON files under `sia/defaults/`, which are also embedded into the Rust binary via `include_dir`. Added Rust and Python tests for provider/profile discovery and resolution, and updated configuration/credential docs.

Applied files:
- `sia/defaults/providers/tinker.json`
- `sia/defaults/profiles/gemini-meta.json`
- `sia/defaults/profiles/gptoss-tinker-target.json`
- `sia/defaults/profiles/qwen3-tinker-target.json`
- `src/providers.rs`
- `src/profiles.rs`
- `tests/test_providers.py`
- `tests/test_profiles.py`
- `docs/configuration.md`
- `docs/CREDENTIALS.md`

### d514cde — Update README.md (#26)
**Decision: IGNORE**

The upstream README change adds Python-project introduction video links. This repository maintains a Rust-port README with different first-page messaging, architecture, and documentation links. No Rust-port README change was adopted.

### 38250cb — chore: bump version to 0.5.1 in pyproject.toml (#27)
**Decision: IGNORE**

This is an upstream Python package release bump. The Rust port is currently versioned separately (`pyproject.toml` remains `0.4.0`), and there is an unrelated local packaging compatibility edit in the worktree. No version churn was adopted.

## Summary

| Commit | Decision | Reason |
|--------|----------|--------|
| `0c87518` | CHERRY-PICK | Rust-port review finding fixes align with reliability and hardening goals |
| `2b49905` | IGNORE | Python-only RL training path conflicts with native Rust weight-update direction |
| `b3eca6c` | REWRITE AND APPLY | Safe declarative Tinker provider/profile config fits data-driven provider registry |
| `d514cde` | IGNORE | Python README marketing links do not fit Rust-port README |
| `38250cb` | IGNORE | Python package release bump does not apply to Rust port versioning |

**Net actions:** 2 apply, 0 already done, 3 ignored.

## Verification

Passed:

```bash
cargo test --test context_manager --test generation_loop --test orchestrator --test stream_to_log --test web_data
cargo test tinker
.venv/bin/python -m pytest tests/test_context_manager.py tests/test_generation_loop.py tests/test_web.py tests/test_providers.py tests/test_profiles.py
```

Python verification passed with one non-failing Starlette/httpx deprecation warning from FastAPI's test client.
