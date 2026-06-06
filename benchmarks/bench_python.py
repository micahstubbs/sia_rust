#!/usr/bin/env python3
"""Python side of the Python-vs-Rust core benchmark suite (issues #31/#32).

Times the deterministic ``sia`` operations that have a Rust port in
``benches/core.rs``, using byte-identical fixtures so the two can be aligned by
bench id in ``run_comparison.py``.

For each op we run a short warmup, then ``ITERS`` timed iterations measured with
``time.perf_counter``, and report ``ns_per_op`` (median-of-the-mean style: total
time / iters). Filesystem-backed ops create their fixtures in a tempdir once and
reuse it across iterations, matching how the Criterion benches are written.

Output: JSON ``{op: {"ns_per_op": float, "iters": int}}`` to stdout and to
``benchmarks/python_results.json``.

The ``_generate_llm_summary`` hook of ContextManager is patched to ``None`` (as the
golden tests do); the Rust ``claude`` impl returns an error immediately, so neither
side performs real LLM work — the benchmark measures the deterministic core.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from pathlib import Path
from typing import Callable
from unittest.mock import patch

# Ensure repo root (parent of this file's dir) is importable as ``sia``.
REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from sia.context_manager import ContextManager  # noqa: E402
from sia.orchestrator import (  # noqa: E402
    TaskFiles,
    _build_feedback_context,
    load_agent_execution,
)
from sia.prompts import build_feedback_prompt, build_meta_prompt  # noqa: E402
from sia.web import runs as rd  # noqa: E402

# Iteration counts: cheap pure ops get many iters; fs-backed ops fewer.
ITERS_FAST = 20_000
ITERS_FS = 5_000
ITERS_CTX = 2_000
ITERS_WEB = 1_000
WARMUP_FRACTION = 0.1  # 10% of iters as warmup


# --------------------------------------------------------------------------- #
# Fixtures
# --------------------------------------------------------------------------- #

GEN1_AGENT = "print('gen 1 agent')\n"
GEN2_AGENT = "import sys\n\n\ndef main():\n    print('gen 2 agent, improved')\n\n\nmain()\n"
IMPROVEMENT_MD = (
    "# Improvement Plan\n\n"
    "- Added structured error handling so the agent recovers from tool failures gracefully.\n"
    "- Switched to a retry loop with exponential backoff for transient API errors.\n"
    "- Improved logging to capture each tool call and its result for later analysis.\n"
)


def task_files() -> TaskFiles:
    return TaskFiles(
        "SAMPLE DESCRIPTIONS BODY",
        "print('reference target agent')",
        {"messages": [{"role": "user", "content": "hi"}]},
        "# Example Task\nSolve the example problem precisely.",
    )


def feedback_task_files() -> TaskFiles:
    return TaskFiles("desc", "ref", {}, "# Task")


def write_context_run(base: Path) -> tuple[Path, Path, Path]:
    run_dir = base / "run_1"
    gen1 = run_dir / "gen_1"
    gen2 = run_dir / "gen_2"
    gen1.mkdir(parents=True, exist_ok=True)
    gen2.mkdir(parents=True, exist_ok=True)
    (gen1 / "target_agent.py").write_text(GEN1_AGENT)
    (gen2 / "target_agent.py").write_text(GEN2_AGENT)
    (gen2 / "improvement.md").write_text(IMPROVEMENT_MD)
    (gen1 / "results.json").write_text(json.dumps({"accuracy": 50.0, "correct": 99, "total": 198}))
    (gen2 / "results.json").write_text(json.dumps({"accuracy": 75.0, "correct": 148, "total": 198}))
    return run_dir, gen1, gen2


def write_feedback_single(base: Path) -> tuple[Path, str]:
    gen_dir = base / "gen_1"
    gen_dir.mkdir(parents=True, exist_ok=True)
    (gen_dir / "agent_execution.json").write_text(json.dumps([{"role": "user", "content": "solve it"}]))
    (gen_dir / "results.json").write_text(json.dumps({"accuracy": 0.9, "correct": 9, "total": 10}))
    return gen_dir, str(gen_dir / "target_agent_stdout.log")


def write_feedback_multi(base: Path) -> tuple[Path, str]:
    gen_dir = base / "gen_1"
    exec_dir = gen_dir / "agent_execution"
    exec_dir.mkdir(parents=True, exist_ok=True)
    for i in range(2):
        (exec_dir / f"execution_q{i}.json").write_text(json.dumps([{"role": "user", "content": f"q{i}"}]))
    (gen_dir / "results.json").write_text(json.dumps({"accuracy": 0.8}))
    return gen_dir, str(gen_dir / "target_agent_stdout.log")


def write_load_single(base: Path) -> Path:
    (base / "agent_execution.json").write_text(json.dumps([{"role": "user", "content": "hello"}]))
    return base


def write_load_multi(base: Path) -> Path:
    exec_dir = base / "agent_execution"
    exec_dir.mkdir(parents=True, exist_ok=True)
    for i in range(3):
        (exec_dir / f"execution_q{i}.json").write_text(
            json.dumps([{"role": "user", "content": f"question {i}"}])
        )
    return base


def make_runs_tree(base: Path, n_runs: int = 10, n_gens: int = 3) -> Path:
    root = base / "runs"
    for r in range(1, n_runs + 1):
        run_dir = root / f"run_{r}"
        run_dir.mkdir(parents=True, exist_ok=True)
        (run_dir / "context.md").write_text(
            f"# Run Context: run_{r}\n\n"
            "**Task**: /tasks/gpqa\n"
            "**Meta Model**: kimi\n"
            "**Task Model**: haiku\n"
            "**Agent impl**: openhands\n"
            "**Started**: 2026-06-05 13:31:32\n"
            f"**Max Generations**: {n_gens}\n\n"
            "---\n\n## Generation 1\n**Status**: ok\n"
        )
        for g in range(1, n_gens + 1):
            gen = run_dir / f"gen_{g}"
            exec_dir = gen / "agent_execution"
            exec_dir.mkdir(parents=True, exist_ok=True)
            (gen / "target_agent.py").write_text("print('hello')\n")
            (gen / "meta_agent_prompt.txt").write_text("meta prompt body")
            (gen / "context.md").write_text("gen context\n")
            (gen / "evaluation_results.json").write_text(
                json.dumps(
                    {
                        "total_questions": 4,
                        "correct": 2,
                        "incorrect": 2,
                        "accuracy": 0.5,
                        "accuracy_percent": 50.0,
                        "details": [
                            {"question_id": 1, "domain": "Physics", "is_correct": True},
                            {"question_id": 2, "domain": "Physics", "is_correct": False},
                            {"question_id": 3, "domain": "Biology", "is_correct": True},
                            {"question_id": 4, "domain": "Biology", "is_correct": False},
                        ],
                    }
                )
            )
            for q in range(1, 4):
                (exec_dir / f"execution_q{q}.json").write_text(
                    json.dumps(
                        [
                            {"role": "system", "content": [{"type": "text", "text": "You are an expert."}]},
                            {"role": "user", "content": f"Question {q}?"},
                            {"role": "assistant", "content": [{"type": "text", "text": "Answer: A"}]},
                        ]
                    )
                )
    return root


# --------------------------------------------------------------------------- #
# Timing harness
# --------------------------------------------------------------------------- #


def time_op(fn: Callable[[], object], iters: int) -> float:
    """Return ns/op for ``fn`` over ``iters`` timed iterations (after warmup)."""
    warmup = max(1, int(iters * WARMUP_FRACTION))
    for _ in range(warmup):
        fn()
    start = time.perf_counter()
    for _ in range(iters):
        fn()
    elapsed = time.perf_counter() - start
    return (elapsed / iters) * 1e9


# --------------------------------------------------------------------------- #
# Benches
# --------------------------------------------------------------------------- #


def bench_build_meta_prompt() -> float:
    tf = task_files()
    return time_op(
        lambda: build_meta_prompt(
            task_files=tf,
            task_model="claude-haiku-4-5-20251001",
            working_dir="/WORK/run_1/gen_1",
        ),
        ITERS_FAST,
    )


def bench_build_feedback_prompt() -> float:
    tf = task_files()
    return time_op(
        lambda: build_feedback_prompt(
            current_gen=2,
            max_gen=3,
            task_files=tf,
            agent_py="print('current target agent gen 2')",
            task="# Example Task\nSolve the example problem precisely.",
            execution_status="SUCCESS: example status block",
            execution_section="EXECUTION SECTION BODY",
            run_dir="/RUN/run_1",
            next_gen_dir="/RUN/run_1/gen_3",
            previous_gens="1",
            task_model="claude-haiku-4-5-20251001",
        ),
        ITERS_FAST,
    )


def bench_context_manager_run() -> float:
    config = {
        "task_dir": "/tasks/example",
        "meta_model": "haiku",
        "task_model": "claude-haiku-4-5-20251001",
        "agent_impl": "claude",
        "max_gen": 2,
    }

    with patch.object(ContextManager, "_generate_llm_summary", return_value=None):

        def one_iteration() -> None:
            with tempfile.TemporaryDirectory() as td:
                run_dir, gen1, gen2 = write_context_run(Path(td))
                cm = ContextManager(str(run_dir), config)
                cm.initialize()
                cm.add_generation(
                    1,
                    {
                        "success": True,
                        "timestamp": "2026-01-01 00:00:00",
                        "duration": 1.5,
                        "agent_path": str(gen1 / "target_agent.py"),
                        "gen_dir": str(gen1),
                        "improvement_path": None,
                        "execution_type": "Single",
                    },
                )
                cm.add_generation(
                    2,
                    {
                        "success": True,
                        "timestamp": "2026-01-01 00:05:00",
                        "duration": 2.5,
                        "agent_path": str(gen2 / "target_agent.py"),
                        "gen_dir": str(gen2),
                        "improvement_path": str(gen2 / "improvement.md"),
                        "execution_type": "Single",
                    },
                )
                cm.finalize()

        return time_op(one_iteration, ITERS_CTX)


def bench_build_feedback_context_single(td: Path) -> float:
    gen_dir, stdout_log = write_feedback_single(td)
    tf = feedback_task_files()
    return time_op(
        lambda: _build_feedback_context(
            current_gen=1,
            gen_dir=str(gen_dir),
            dataset_dir="/data/public",
            target_agent_success=True,
            target_agent_error_msg="",
            target_agent_stdout="line1\nline2\nline3\n",
            target_agent_stderr="",
            stdout_log_file=stdout_log,
            task_files=tf,
        ),
        ITERS_FS,
    )


def bench_build_feedback_context_multi(td: Path) -> float:
    gen_dir, stdout_log = write_feedback_multi(td)
    tf = feedback_task_files()
    return time_op(
        lambda: _build_feedback_context(
            current_gen=1,
            gen_dir=str(gen_dir),
            dataset_dir="/data/public",
            target_agent_success=True,
            target_agent_error_msg="",
            target_agent_stdout="processing q0\nprocessing q1\ndone\n",
            target_agent_stderr="",
            stdout_log_file=stdout_log,
            task_files=tf,
        ),
        ITERS_FS,
    )


def bench_load_agent_execution_single(td: Path) -> float:
    write_load_single(td)
    gen_dir = str(td)
    return time_op(lambda: load_agent_execution(gen_dir), ITERS_FS)


def bench_load_agent_execution_multi(td: Path) -> float:
    write_load_multi(td)
    gen_dir = str(td)
    return time_op(lambda: load_agent_execution(gen_dir), ITERS_FS)


def bench_web_list_runs(td: Path) -> float:
    root = make_runs_tree(td)
    return time_op(lambda: rd.list_runs(root), ITERS_WEB)


def bench_web_get_run(td: Path) -> float:
    root = make_runs_tree(td)
    return time_op(lambda: rd.get_run(root, "run_5"), ITERS_WEB)


# --------------------------------------------------------------------------- #
# Driver
# --------------------------------------------------------------------------- #


def main() -> None:
    results: dict[str, dict[str, float | int]] = {}

    def record(op: str, ns: float, iters: int) -> None:
        results[op] = {"ns_per_op": ns, "iters": iters}
        print(f"  {op:32s} {ns:14.1f} ns/op  ({iters} iters)", file=sys.stderr)

    print("Running Python benchmarks...", file=sys.stderr)

    # Pure ops (own fixtures).
    record("build_meta_prompt", bench_build_meta_prompt(), ITERS_FAST)
    record("build_feedback_prompt", bench_build_feedback_prompt(), ITERS_FAST)
    record("context_manager_run", bench_context_manager_run(), ITERS_CTX)

    # Filesystem ops (one tempdir per op, reused across iterations).
    fs_benches = [
        ("build_feedback_context_single", bench_build_feedback_context_single, ITERS_FS),
        ("build_feedback_context_multi", bench_build_feedback_context_multi, ITERS_FS),
        ("load_agent_execution_single", bench_load_agent_execution_single, ITERS_FS),
        ("load_agent_execution_multi", bench_load_agent_execution_multi, ITERS_FS),
        ("web_list_runs", bench_web_list_runs, ITERS_WEB),
        ("web_get_run", bench_web_get_run, ITERS_WEB),
    ]
    for op, fn, iters in fs_benches:
        with tempfile.TemporaryDirectory() as td:
            record(op, fn(Path(td)), iters)

    out_path = Path(__file__).resolve().parent / "python_results.json"
    out_path.write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))
    print(f"\nWrote {out_path}", file=sys.stderr)


if __name__ == "__main__":
    main()
