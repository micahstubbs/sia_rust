#!/usr/bin/env python3
"""Drive the Python and Rust benchmarks and emit a comparison report (issue #32).

Steps:
  1. Run ``python3 benchmarks/bench_python.py`` -> benchmarks/python_results.json
     ({op: {ns_per_op, iters}}).
  2. Run ``cargo bench --bench core`` (Criterion), which writes
     ``target/criterion/<bench_id>/new/estimates.json`` with ``mean.point_estimate``
     in nanoseconds.
  3. Align the two by bench id (the ids match between the Python script and the
     Criterion ``bench_function`` names) and write ``benchmarks/REPORT.md`` with a
     table: Operation | Python ns/op | Rust ns/op | Speedup (Python/Rust), plus a
     methodology section and environment notes.

Run from anywhere; paths are resolved relative to the repo root.
"""

from __future__ import annotations

import json
import platform
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BENCH_DIR = REPO_ROOT / "benchmarks"
PYTHON_RESULTS = BENCH_DIR / "python_results.json"
CRITERION_DIR = REPO_ROOT / "target" / "criterion"
REPORT = BENCH_DIR / "REPORT.md"

# Operation order + human-readable descriptions for the report.
OPS = [
    ("build_meta_prompt", "Build the meta-agent prompt (pure string assembly)"),
    ("build_feedback_prompt", "Build the feedback-agent prompt (pure string assembly)"),
    ("context_manager_run", "ContextManager: initialize + add_generation x2 + finalize"),
    ("build_feedback_context_single", "Build feedback context, single-trajectory execution"),
    ("build_feedback_context_multi", "Build feedback context, multi-trajectory execution"),
    ("load_agent_execution_single", "Load a single-file agent execution log"),
    ("load_agent_execution_multi", "Load a multi-trajectory agent execution folder"),
    ("web_list_runs", "Web visualizer: list_runs over ~10 runs x 3 generations"),
    ("web_get_run", "Web visualizer: get_run detail for one run"),
]


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    print(f"$ {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, cwd=REPO_ROOT, check=True, **kw)


def run_python_benchmarks() -> dict[str, dict]:
    run([sys.executable, str(BENCH_DIR / "bench_python.py")])
    return json.loads(PYTHON_RESULTS.read_text())


def run_rust_benchmarks() -> None:
    # Quieter, faster Criterion run; still writes estimates.json for every bench.
    run(["cargo", "bench", "--bench", "core"])


def parse_criterion() -> dict[str, float]:
    """Map bench id -> mean ns from target/criterion/<id>/new/estimates.json."""
    results: dict[str, float] = {}
    if not CRITERION_DIR.is_dir():
        return results
    for est in CRITERION_DIR.glob("*/new/estimates.json"):
        bench_id = est.parent.parent.name
        data = json.loads(est.read_text())
        mean = data.get("mean", {}).get("point_estimate")
        if mean is not None:
            results[bench_id] = float(mean)
    return results


def fmt_ns(ns: float | None) -> str:
    if ns is None:
        return "n/a"
    if ns >= 1e6:
        return f"{ns:,.0f} ({ns / 1e6:.3f} ms)"
    if ns >= 1e3:
        return f"{ns:,.0f} ({ns / 1e3:.2f} us)"
    return f"{ns:,.1f}"


def rustc_version() -> str:
    try:
        out = subprocess.run(["rustc", "--version"], capture_output=True, text=True, check=True)
        return out.stdout.strip()
    except Exception:
        return "unknown"


def cpu_model() -> str:
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except Exception:
        pass
    return platform.processor() or platform.machine() or "unknown"


def cpu_count() -> str:
    try:
        import os

        return str(os.cpu_count())
    except Exception:
        return "unknown"


def build_report(py: dict[str, dict], rs: dict[str, float]) -> str:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    lines: list[str] = []
    lines.append("# Python vs. Rust Core Benchmark Report")
    lines.append("")
    lines.append(f"_Generated {now} by `benchmarks/run_comparison.py`._")
    lines.append("")
    lines.append(
        "This report compares the original Python `sia` implementation against the "
        "Rust port for the deterministic core operations. Each operation is measured "
        "in **both** languages with byte-identical fixtures so the numbers are directly "
        "comparable."
    )
    lines.append("")
    lines.append("## Results")
    lines.append("")
    lines.append("| Operation | Python ns/op | Rust ns/op | Speedup (Python/Rust) |")
    lines.append("| --- | ---: | ---: | ---: |")

    for op, _desc in OPS:
        py_ns = py.get(op, {}).get("ns_per_op")
        rs_ns = rs.get(op)
        if py_ns is not None and rs_ns:
            speedup = f"{py_ns / rs_ns:.1f}x"
        else:
            speedup = "n/a"
        lines.append(f"| `{op}` | {fmt_ns(py_ns)} | {fmt_ns(rs_ns)} | {speedup} |")

    # Aggregate speedup (geometric mean) over ops present in both.
    import math

    ratios = [
        py[op]["ns_per_op"] / rs[op]
        for op, _ in OPS
        if op in py and op in rs and rs[op] > 0
    ]
    if ratios:
        gmean = math.exp(sum(math.log(r) for r in ratios) / len(ratios))
        lines.append("")
        lines.append(f"**Geometric-mean speedup (Python/Rust) across {len(ratios)} operations: {gmean:.1f}x**")

    lines.append("")
    lines.append("## Operations")
    lines.append("")
    for op, desc in OPS:
        lines.append(f"- `{op}` — {desc}")

    lines.append("")
    lines.append("## Methodology")
    lines.append("")
    lines.append(
        "- **Rust**: [Criterion](https://github.com/bheisler/criterion.rs) statistical "
        "benchmarking (`cargo bench --bench core`, `harness = false`). Reported value is "
        "`mean.point_estimate` (ns) from `target/criterion/<bench_id>/new/estimates.json`. "
        "Criterion performs its own warmup, sampling, and outlier analysis; benches run in "
        "the optimized `release` profile."
    )
    lines.append(
        "- **Python**: `benchmarks/bench_python.py` times each function with "
        "`time.perf_counter` over a fixed iteration count (10% warmup), reporting "
        "total time / iterations as ns/op."
    )
    lines.append(
        "- **Fixtures**: identical between the two. Filesystem-backed operations build "
        "their fixtures once in a tempdir and reuse it across iterations; "
        "`context_manager_run` builds a fresh tempdir per iteration in both languages."
    )
    lines.append(
        "- **LLM calls neutralized**: `ContextManager._generate_llm_summary` is patched "
        "to `None` on the Python side; the Rust `claude` agent impl returns an error "
        "immediately. Neither side performs real LLM/network work, so the benchmark "
        "isolates the deterministic core."
    )
    lines.append(
        "- **Alignment**: bench ids are identical between the Criterion benches "
        "(`benches/core.rs`) and the Python script, so the comparison aligns automatically."
    )
    lines.append(
        "- **Caveats**: ns/op figures mix CPU work with filesystem I/O for the fs-backed "
        "ops and are sensitive to machine load and OS file-cache state. Treat speedups as "
        "order-of-magnitude indicators, not precise constants."
    )

    lines.append("")
    lines.append("## Environment")
    lines.append("")
    lines.append(f"- CPU: {cpu_model()} ({cpu_count()} logical cores)")
    lines.append(f"- OS / platform: {platform.platform()}")
    lines.append(f"- Python: {platform.python_version()} ({platform.python_implementation()})")
    lines.append(f"- Rust: {rustc_version()}")
    lines.append("")

    return "\n".join(lines)


def main() -> None:
    py = run_python_benchmarks()
    run_rust_benchmarks()
    rs = parse_criterion()

    missing_rust = [op for op, _ in OPS if op not in rs]
    if missing_rust:
        print(f"WARNING: no Criterion estimate for: {missing_rust}", file=sys.stderr)

    report = build_report(py, rs)
    REPORT.write_text(report)
    print(report)
    print(f"\nWrote {REPORT}", file=sys.stderr)


if __name__ == "__main__":
    main()
