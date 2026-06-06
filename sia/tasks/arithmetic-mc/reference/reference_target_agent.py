#!/usr/bin/env python3
"""
Reference target agent for the arithmetic-mc task.

Unlike the GPQA reference (which calls an LLM), this reference solves the
arithmetic deterministically with Python's own evaluator so the task is fully
runnable offline — ideal for a demo. A real SIA target agent would instead call
a model; the *output contract* below is what matters and is identical to GPQA's:

    results/{timestamp}.json
    {
      "model": "...",
      "total_questions": N,
      "details": [ {"question_id": 1, "model_answer": "B", ...}, ... ]
    }

The agent reads the *public* questions (no answers), produces a chosen A-D
letter per question, and writes a submission JSON into ``working_dir/results/``.
``evaluate.py`` then grades it.
"""

import argparse
import ast
import json
import operator
import re
from datetime import datetime
from pathlib import Path

# A tiny safe arithmetic evaluator (no eval()): supports + - * / and parentheses.
_BINOPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
}


def safe_eval(expr: str) -> float:
    """Evaluate a simple arithmetic expression safely via the ast module."""

    def _eval(node):
        if isinstance(node, ast.Expression):
            return _eval(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
            return node.value
        if isinstance(node, ast.BinOp) and type(node.op) in _BINOPS:
            return _BINOPS[type(node.op)](_eval(node.left), _eval(node.right))
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
            return -_eval(node.operand)
        raise ValueError(f"unsupported expression: {ast.dump(node)}")

    return _eval(ast.parse(expr, mode="eval"))


def solve(question: dict) -> str:
    """Return the A-D letter whose option matches the computed answer."""
    # Pull the arithmetic expression out of the question text, e.g. "What is 2 + 3 * 4?".
    match = re.search(r"([0-9][0-9\s+\-*/()]*[0-9])", question["Question"])
    if not match:
        return ""
    try:
        value = safe_eval(match.group(1))
    except (ValueError, SyntaxError, ZeroDivisionError):
        return ""
    # Match the computed value against the option strings.
    for letter, option in question["options"].items():
        try:
            if abs(float(option) - value) < 1e-9:
                return letter
        except (ValueError, TypeError):
            continue
    return ""


def main():
    parser = argparse.ArgumentParser(description="Arithmetic-MC reference agent")
    parser.add_argument("--dataset_dir", type=Path, required=True,
                        help="Directory containing questions.json (the public copy)")
    parser.add_argument("--working_dir", type=Path, required=True,
                        help="Working directory where results/ will be created")
    args = parser.parse_args()

    data_file = args.dataset_dir / "questions.json"
    if not data_file.is_file():
        raise SystemExit(f"Missing data file: {data_file}")
    questions = json.loads(data_file.read_text(encoding="utf-8"))

    details = []
    for q in questions:
        details.append({"question_id": q["id"], "model_answer": solve(q)})

    results = {
        "model": "deterministic-arithmetic-reference",
        "total_questions": len(questions),
        "details": details,
        "timestamp": datetime.now().isoformat(),
    }

    output_dir = args.working_dir / "results"
    output_dir.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    output_file = output_dir / f"submission_{timestamp}.json"
    output_file.write_text(json.dumps(results, indent=2), encoding="utf-8")
    print(f"Wrote {len(questions)} answers to {output_file}")


if __name__ == "__main__":
    main()
