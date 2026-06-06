#!/usr/bin/env python3
"""Differential parity harness: run the reference Python implementation and the
Rust `sia-parity` helper on the same inputs and assert byte-identical output.

Covers the deterministic surfaces that SIA writes to disk / embeds in prompts:
  - json.dumps(..., indent=2)  (the ensure_ascii path — CJK / emoji / control)
  - build_meta_prompt / build_feedback_prompt
  - _build_feedback_context  (status + section)
  - load_agent_execution     (format detection + exact shapes)

Usage:  python scripts/parity_check.py
Exits non-zero if any diff is found.
"""

from __future__ import annotations

import difflib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO))

from sia.orchestrator import _build_feedback_context, load_agent_execution  # noqa: E402
from sia.prompts import build_feedback_prompt, build_meta_prompt  # noqa: E402
from sia.providers import load_provider  # noqa: E402
from sia.run_setup import TaskFiles  # noqa: E402

PARITY_BIN = REPO / "target" / "debug" / "sia-parity"

failures: list[str] = []


def rust(mode: str, payload: dict | list) -> str:
    res = subprocess.run(
        [str(PARITY_BIN), mode],
        input=json.dumps(payload),
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        raise RuntimeError(f"sia-parity {mode} failed: {res.stderr}")
    return res.stdout


def check(name: str, expected: str, actual: str) -> None:
    if expected == actual:
        print(f"  ok   {name}")
        return
    failures.append(name)
    diff = "\n".join(
        difflib.unified_diff(
            expected.splitlines(), actual.splitlines(), "python", "rust", lineterm=""
        )
    )
    print(f"  FAIL {name}\n{diff}")


# --------------------------------------------------------------------------- #
# 1. json.dumps(..., indent=2) — the ensure_ascii path
# --------------------------------------------------------------------------- #
def test_json_dumps() -> None:
    cases = {
        "ascii_obj": {"messages": [{"role": "user", "content": "hi"}]},
        "cjk": {"charge": "故意伤害罪", "n": 198},
        "emoji": {"msg": "ok ✓ 🔧 done"},
        "astral": {"x": "𝔘𝔫𝔦"},
        "control": {"t": "a\tb\nc\r"},
        "slash_angle": {"path": "a/b</c>"},
        "empty_obj": {},
        "empty_list": [],
        "nested": [{"role": "user", "content": "café ☕", "k": [1, 2, {"z": "汉字"}]}],
        "floats": {"a": 0.9, "b": 1.0, "c": 50.0, "d": 0.1},
        "floats_sci": {"tiny": 1e-7, "tiny2": 1.5e-7, "big": 1e16, "bigger": 1e20, "norm": 2.5e-3},
        "ints": {"big": 10_000_000_000, "huge": 9_007_199_254_740_993, "neg": -5, "zero": 0},
        "bools_null": {"t": True, "f": False, "n": None},
        "lawbench_like": {
            "messages": [
                {"role": "system", "content": "你是一名法律专家。"},
                {"role": "user", "content": "根据描述预测罪名：被告人故意伤害他人身体。"},
                {"role": "assistant", "content": "故意伤害罪"},
            ]
        },
    }
    for name, value in cases.items():
        check(f"json-dumps/{name}", json.dumps(value, indent=2), rust("json-dumps", value))


# --------------------------------------------------------------------------- #
# 2. build_meta_prompt / build_feedback_prompt
# --------------------------------------------------------------------------- #
def _tf(sample_exec, *, cjk=False) -> tuple[TaskFiles, dict]:
    if cjk:
        d = dict(
            sample_task_descriptions="样例任务：预测罪名。",
            reference_target_agent_py="print('参考实现')",
            sample_agent_execution=sample_exec,
            task_md="# 任务\n根据案情描述预测罪名。",
        )
    else:
        d = dict(
            sample_task_descriptions="SAMPLE DESCRIPTIONS BODY",
            reference_target_agent_py="print('reference target agent')",
            sample_agent_execution=sample_exec,
            task_md="# Example Task\nSolve it.",
        )
    return TaskFiles(**d), d


def test_meta_prompt() -> None:
    matrix = [
        ("ascii_default", {"messages": [{"role": "user", "content": "hi"}]}, False, None),
        ("cjk_lawbench", {"messages": [{"role": "user", "content": "罪名？"}]}, True, None),
        ("nebius_openai", {"messages": [{"role": "user", "content": "hi"}]}, False, "nebius"),
    ]
    for name, sample, cjk, provider in matrix:
        tf, tfd = _tf(sample, cjk=cjk)
        prov = load_provider(provider) if provider else None
        py = build_meta_prompt(tf, "claude-haiku-4-5-20251001", "/WORK/run_1/gen_1", provider=prov)
        payload = {
            "task_files": tfd,
            "task_model": "claude-haiku-4-5-20251001",
            "working_dir": "/WORK/run_1/gen_1",
        }
        if provider:
            payload["provider"] = provider
        check(f"meta-prompt/{name}", py, rust("meta-prompt", payload))


def test_feedback_prompt() -> None:
    matrix = [("cjk", None, None), ("nebius_openai_reqs", "nebius", "/RUN/run_1/gen_3")]
    for name, provider, reqs in matrix:
        tf, tfd = _tf({"messages": [{"role": "user", "content": "汉字"}]}, cjk=True)
        prov = load_provider(provider) if provider else None
        py = build_feedback_prompt(
            current_gen=2,
            max_gen=3,
            task_files=tf,
            agent_py="print('当前实现')",
            task="# 任务\n预测罪名。",
            execution_status="成功",
            execution_section="执行日志",
            run_dir="/RUN/run_1",
            next_gen_dir="/RUN/run_1/gen_3",
            previous_gens="1",
            task_model="claude-haiku-4-5-20251001",
            provider=prov,
            requirements_dir=reqs,
        )
        payload = {
            "current_gen": 2,
            "max_gen": 3,
            "task_files": tfd,
            "agent_py": "print('当前实现')",
            "task": "# 任务\n预测罪名。",
            "execution_status": "成功",
            "execution_section": "执行日志",
            "run_dir": "/RUN/run_1",
            "next_gen_dir": "/RUN/run_1/gen_3",
            "previous_gens": "1",
            "task_model": "claude-haiku-4-5-20251001",
        }
        if provider:
            payload["provider"] = provider
        if reqs:
            payload["requirements_dir"] = reqs
        check(f"feedback-prompt/{name}", py, rust("feedback-prompt", payload))


# --------------------------------------------------------------------------- #
# 3. _build_feedback_context (status + section), with CJK execution data
# --------------------------------------------------------------------------- #
def test_feedback_context() -> None:
    with tempfile.TemporaryDirectory() as td:
        gen = Path(td) / "gen_1"
        gen.mkdir()
        (gen / "agent_execution.json").write_text(
            json.dumps([{"role": "user", "content": "预测罪名：故意伤害"}]), encoding="utf-8"
        )
        (gen / "results.json").write_text(
            json.dumps({"accuracy": 0.9, "charge": "故意伤害罪"}), encoding="utf-8"
        )
        log = str(gen / "target_agent_stdout.log")
        tf = TaskFiles("desc", "ref", {}, "# Task")
        status, section = _build_feedback_context(
            current_gen=1,
            gen_dir=str(gen),
            dataset_dir="/data/public",
            target_agent_success=True,
            target_agent_error_msg="",
            target_agent_stdout="处理中\n完成\n",
            target_agent_stderr="",
            stdout_log_file=log,
            task_files=tf,
        )
        payload = {
            "current_gen": 1,
            "gen_dir": str(gen),
            "dataset_dir": "/data/public",
            "success": True,
            "error_msg": "",
            "stdout": "处理中\n完成\n",
            "stderr": "",
            "stdout_log_file": log,
            "task_files": {
                "sample_task_descriptions": "desc",
                "reference_target_agent_py": "ref",
                "sample_agent_execution": {},
                "task_md": "# Task",
            },
        }
        out = json.loads(rust("feedback-context", payload))
        check("feedback-context/cjk-status", status, out["status"])
        check("feedback-context/cjk-section", section, out["section"])

    # Multi-trajectory with CJK content (exercises the trajectory json.dumps path).
    with tempfile.TemporaryDirectory() as td:
        gen = Path(td) / "gen_1"
        ex = gen / "agent_execution"
        ex.mkdir(parents=True)
        for i in range(2):
            (ex / f"execution_q{i}.json").write_text(
                json.dumps([{"role": "user", "content": f"问题{i}：罪名？"}]), encoding="utf-8"
            )
        (gen / "results.json").write_text(json.dumps({"accuracy": 0.8, "类别": "刑法"}), encoding="utf-8")
        log = str(gen / "target_agent_stdout.log")
        tf = TaskFiles("desc", "ref", {}, "# Task")
        status, section = _build_feedback_context(
            current_gen=1,
            gen_dir=str(gen),
            dataset_dir="/data/public",
            target_agent_success=True,
            target_agent_error_msg="",
            target_agent_stdout="处理 q0\n处理 q1\n完成\n",
            target_agent_stderr="",
            stdout_log_file=log,
            task_files=tf,
        )
        payload = {
            "current_gen": 1,
            "gen_dir": str(gen),
            "dataset_dir": "/data/public",
            "success": True,
            "error_msg": "",
            "stdout": "处理 q0\n处理 q1\n完成\n",
            "stderr": "",
            "stdout_log_file": log,
            "task_files": {
                "sample_task_descriptions": "desc",
                "reference_target_agent_py": "ref",
                "sample_agent_execution": {},
                "task_md": "# Task",
            },
        }
        out = json.loads(rust("feedback-context", payload))
        check("feedback-context/multi-cjk-status", status, out["status"])
        check("feedback-context/multi-cjk-section", section, out["section"])


# --------------------------------------------------------------------------- #
# 4. load_agent_execution — deterministic shapes (excludes malformed JSON, whose
#    parser-specific error text legitimately differs)
# --------------------------------------------------------------------------- #
def test_load_exec() -> None:
    # single valid
    with tempfile.TemporaryDirectory() as td:
        (Path(td) / "agent_execution.json").write_text(
            json.dumps([{"role": "user", "content": "汉字"}]), encoding="utf-8"
        )
        py_data, py_multi = load_agent_execution(td)
        out = json.loads(rust("load-exec", {"gen_dir": td}))
        check("load-exec/single-data", json.dumps(py_data, sort_keys=True), json.dumps(out["data"], sort_keys=True))
        check("load-exec/single-multi", json.dumps(py_multi), json.dumps(out["is_multi"]))

    # multi valid
    with tempfile.TemporaryDirectory() as td:
        ex = Path(td) / "agent_execution"
        ex.mkdir()
        for i in range(3):
            (ex / f"execution_q{i}.json").write_text(
                json.dumps([{"role": "user", "content": f"q{i}"}]), encoding="utf-8"
            )
        py_data, py_multi = load_agent_execution(td)
        out = json.loads(rust("load-exec", {"gen_dir": td}))
        check("load-exec/multi-data", json.dumps(py_data, sort_keys=True), json.dumps(out["data"], sort_keys=True))
        check("load-exec/multi-multi", json.dumps(py_multi), json.dumps(out["is_multi"]))

    # missing
    with tempfile.TemporaryDirectory() as td:
        py_data, py_multi = load_agent_execution(td)
        out = json.loads(rust("load-exec", {"gen_dir": td}))
        check("load-exec/missing", json.dumps(py_data, sort_keys=True), json.dumps(out["data"], sort_keys=True))

    # empty multi folder
    with tempfile.TemporaryDirectory() as td:
        (Path(td) / "agent_execution").mkdir()
        py_data, py_multi = load_agent_execution(td)
        out = json.loads(rust("load-exec", {"gen_dir": td}))
        check("load-exec/empty-folder", json.dumps(py_data, sort_keys=True), json.dumps(out["data"], sort_keys=True))


def main() -> int:
    if not PARITY_BIN.exists():
        print(f"Build the helper first: cargo build --bin sia-parity  (missing {PARITY_BIN})")
        return 2
    print("Differential parity: Python ⇄ Rust")
    test_json_dumps()
    test_meta_prompt()
    test_feedback_prompt()
    test_feedback_context()
    test_load_exec()
    print()
    if failures:
        print(f"PARITY FAILED: {len(failures)} diff(s): {', '.join(failures)}")
        return 1
    print("PARITY OK: all surfaces byte-identical")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
