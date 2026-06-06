#!/usr/bin/env python3
"""
Evaluate arithmetic multiple-choice submissions (the `evaluate.py` contract).

This mirrors the GPQA evaluator but is fully self-contained (standard library
only) so the task is runnable without any dataset download or extra deps.

Contract (identical to every SIA task):
1. Ground truth is read from ``data/private/questions.json`` (relative to this
   script, which lives in ``data/public/``). The private file carries the
   ``correct_answer_letter`` field; the public file the agent sees does not.
2. The model's predictions are read from a submission JSON discovered under the
   ``--gen-dir`` (it searches ``results/`` then common ``*.json`` patterns).
3. Per question, the chosen letter is normalized to a single A-D letter and
   compared to the correct letter.
4. A ``results.json`` is written into ``--gen-dir`` (the orchestrator keys off
   this file existing). Its ``accuracy`` field is in ``[0, 1]`` (correct /
   attempted) — the canonical score the harness reads.

Usage:
    python evaluate.py --gen-dir path/to/generation/directory
    python evaluate.py --submission path/to/submission.json
"""

import argparse
import json
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional


def load_json(path: Path):
    if not path.is_file():
        raise FileNotFoundError(f"File not found: {path}")
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def build_correct_answers(questions: List[dict]) -> Dict[int, str]:
    """Map question_id -> correct_answer_letter from the private dataset."""
    correct = {}
    for item in questions:
        qid = item.get("id")
        letter = item.get("correct_answer_letter")
        if qid is not None and letter:
            correct[qid] = str(letter).strip().upper()
    return correct


def find_submission_file(gen_dir: Path) -> Optional[Path]:
    """Discover a submission JSON, preferring a results/ subdirectory."""
    if not gen_dir.is_dir():
        return None
    results_dir = gen_dir / "results"
    if results_dir.is_dir():
        json_files = list(results_dir.glob("*.json"))
        if json_files:
            return max(json_files, key=lambda p: p.stat().st_mtime)
    for pattern in ("results*.json", "submission*.json", "output*.json"):
        matches = list(gen_dir.glob(pattern))
        if matches:
            return max(matches, key=lambda p: p.stat().st_mtime)
    json_files = list(gen_dir.glob("*.json"))
    if json_files:
        return max(json_files, key=lambda p: p.stat().st_mtime)
    return None


def normalize_answer(answer) -> str:
    """Normalize to a single A-D letter (mirrors the GPQA normalizer)."""
    answer = str(answer).strip().upper()
    if answer in "ABCD" and len(answer) == 1:
        return answer
    for char in answer:
        if char in "ABCD":
            return char
    return ""


def extract_submission_answers(submission: Dict) -> Dict[int, str]:
    answers: Dict[int, str] = {}
    if "details" in submission:
        for detail in submission["details"]:
            qid = detail.get("question_id")
            if qid is not None:
                answers[int(qid)] = normalize_answer(detail.get("model_answer", ""))
    elif "answers" in submission:
        for qid_str, value in submission["answers"].items():
            try:
                answers[int(qid_str)] = normalize_answer(value)
            except (ValueError, TypeError):
                continue
    return answers


def evaluate_submission(submission: Dict, correct: Dict[int, str]) -> Dict:
    got = extract_submission_answers(submission)
    results = {
        "total_questions": len(correct),
        "correct": 0,
        "incorrect": 0,
        "missing": 0,
        "accuracy": 0.0,
        "accuracy_percent": 0.0,
        "details": [],
        "timestamp": datetime.now().isoformat(),
    }
    for qid, want in correct.items():
        have = got.get(qid, "")
        detail = {"question_id": qid, "correct_answer": want, "model_answer": have}
        if not have:
            results["missing"] += 1
            detail["is_correct"] = False
        elif have == want:
            results["correct"] += 1
            detail["is_correct"] = True
        else:
            results["incorrect"] += 1
            detail["is_correct"] = False
        results["details"].append(detail)

    attempted = results["correct"] + results["incorrect"]
    if attempted > 0:
        results["accuracy"] = results["correct"] / attempted
        results["accuracy_percent"] = 100 * results["accuracy"]
    return results


def main():
    parser = argparse.ArgumentParser(description="Evaluate arithmetic-mc submissions")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--gen-dir", type=Path, help="Generation directory with submission JSON")
    group.add_argument("--submission", type=Path, help="Direct path to submission JSON")
    parser.add_argument("--output", type=Path, default=None, help="Where to write results.json")
    args = parser.parse_args()

    # Ground truth: this script is in data/public/, private sits alongside it.
    private = Path(__file__).resolve().parent.parent / "private" / "questions.json"
    questions = load_json(private)
    correct = build_correct_answers(questions)
    print(f"Loaded {len(questions)} questions")

    if args.submission:
        submission_path = args.submission
    else:
        submission_path = find_submission_file(args.gen_dir)
        if submission_path is None:
            raise FileNotFoundError(
                f"No submission file found in {args.gen_dir}; pass --submission directly."
            )
    print(f"Loading submission from: {submission_path}")
    submission = load_json(submission_path)

    results = evaluate_submission(submission, correct)

    if args.output:
        output_path = args.output
    elif args.gen_dir:
        output_path = args.gen_dir / "results.json"
    else:
        output_path = submission_path.parent / "results.json"
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2)

    print("=" * 50)
    print("Arithmetic-MC Evaluation")
    print("=" * 50)
    print(f"Correct:   {results['correct']}")
    print(f"Incorrect: {results['incorrect']}")
    print(f"Missing:   {results['missing']}")
    print(f"Accuracy:  {results['accuracy_percent']:.2f}%")
    print(f"Saved:     {output_path}")


if __name__ == "__main__":
    main()
