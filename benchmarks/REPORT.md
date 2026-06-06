# Python vs. Rust Core Benchmark Report

_Generated 2026-06-06 08:29 UTC by `benchmarks/run_comparison.py`._

This report compares the original Python `sia` implementation against the Rust port for the deterministic core operations. Each operation is measured in **both** languages with byte-identical fixtures so the numbers are directly comparable.

## Results

| Operation | Python ns/op | Rust ns/op | Speedup (Python/Rust) |
| --- | ---: | ---: | ---: |
| `build_meta_prompt` | 13,616 (13.62 us) | 551.4 | 24.7x |
| `build_feedback_prompt` | 1,357 (1.36 us) | 519.9 | 2.6x |
| `context_manager_run` | 1,260,413 (1.260 ms) | 1,041,368 (1.041 ms) | 1.2x |
| `build_feedback_context_single` | 200,225 (200.22 us) | 14,333 (14.33 us) | 14.0x |
| `build_feedback_context_multi` | 283,415 (283.42 us) | 27,682 (27.68 us) | 10.2x |
| `load_agent_execution_single` | 125,182 (125.18 us) | 6,997 (7.00 us) | 17.9x |
| `load_agent_execution_multi` | 199,860 (199.86 us) | 25,149 (25.15 us) | 7.9x |
| `web_list_runs` | 3,485,486 (3.485 ms) | 1,956,338 (1.956 ms) | 1.8x |
| `web_get_run` | 684,079 (684.08 us) | 253,380 (253.38 us) | 2.7x |

**Geometric-mean speedup (Python/Rust) across 9 operations: 5.8x**

## Operations

- `build_meta_prompt` — Build the meta-agent prompt (pure string assembly)
- `build_feedback_prompt` — Build the feedback-agent prompt (pure string assembly)
- `context_manager_run` — ContextManager: initialize + add_generation x2 + finalize
- `build_feedback_context_single` — Build feedback context, single-trajectory execution
- `build_feedback_context_multi` — Build feedback context, multi-trajectory execution
- `load_agent_execution_single` — Load a single-file agent execution log
- `load_agent_execution_multi` — Load a multi-trajectory agent execution folder
- `web_list_runs` — Web visualizer: list_runs over ~10 runs x 3 generations
- `web_get_run` — Web visualizer: get_run detail for one run

## Methodology

- **Rust**: [Criterion](https://github.com/bheisler/criterion.rs) statistical benchmarking (`cargo bench --bench core`, `harness = false`). Reported value is `mean.point_estimate` (ns) from `target/criterion/<bench_id>/new/estimates.json`. Criterion performs its own warmup, sampling, and outlier analysis; benches run in the optimized `release` profile.
- **Python**: `benchmarks/bench_python.py` times each function with `time.perf_counter` over a fixed iteration count (10% warmup), reporting total time / iterations as ns/op.
- **Fixtures**: identical between the two. Filesystem-backed operations build their fixtures once in a tempdir and reuse it across iterations; `context_manager_run` builds a fresh tempdir per iteration in both languages.
- **LLM calls neutralized**: `ContextManager._generate_llm_summary` is patched to `None` on the Python side; the Rust `claude` agent impl returns an error immediately. Neither side performs real LLM/network work, so the benchmark isolates the deterministic core.
- **Alignment**: bench ids are identical between the Criterion benches (`benches/core.rs`) and the Python script, so the comparison aligns automatically.
- **Caveats**: ns/op figures mix CPU work with filesystem I/O for the fs-backed ops and are sensitive to machine load and OS file-cache state. Treat speedups as order-of-magnitude indicators, not precise constants.

## Environment

- CPU: Intel(R) Xeon(R) Processor @ 2.80GHz (4 logical cores)
- OS / platform: Linux-6.18.5-x86_64-with-glibc2.39
- Python: 3.11.15 (CPython)
- Rust: rustc 1.94.1 (e408947bf 2026-03-25)
