# Arithmetic multiple-choice (`questions.json`)

A tiny, fully self-contained multiple-choice task used as a polished, runnable
**example template** for authoring SIA tasks. It needs no dataset download and no
API key — the bundled reference agent solves it deterministically — so it is
ideal for demos and for learning the `evaluate.py` contract.

You are scored on **how many questions you answer correctly** on a fixed set of
elementary arithmetic items, each with four options `A`–`D` and exactly one
correct answer.

## Data

`questions.json` (the public copy) contains records with:

- `id` — stable question identifier
- `domain`, `subdomain` — topic labels
- `Question` — the prompt, e.g. `"What is 2 + 3 * 4?"`
- `options` — a dict with keys `A`, `B`, `C`, `D` mapping to option text

The matching **private** copy (`data/private/questions.json`) additionally carries
`correct_answer` and `correct_answer_letter`. The grader reads the private copy;
your agent only ever sees the public copy.

## Objective

**Maximize accuracy** = correct / attempted, reported in `[0, 1]`.

## Output format

Your agent must write a submission JSON into `working_dir/results/`:

```json
{
  "model": "your-model-name",
  "total_questions": 5,
  "details": [
    {"question_id": 1, "model_answer": "B"},
    {"question_id": 2, "model_answer": "C"}
  ]
}
```

**Required per-detail fields:** `question_id` and `model_answer` (a single letter
`A`–`D`). The grader normalizes `model_answer` to a single A–D letter, so a value
like `"The answer is B"` is tolerated (it resolves to the first A–D letter found).

## The `evaluate.py` contract

`evaluate.py` (in `data/public/`) is the canonical grader and follows the same
contract as every SIA task:

1. Run it with `python evaluate.py --gen-dir <generation_dir>`.
2. It loads ground truth from `data/private/questions.json`.
3. It discovers the submission JSON under `<generation_dir>` (it checks a
   `results/` subdirectory first, then common `*.json` patterns).
4. It writes a **`results.json`** into the generation directory. The orchestrator
   keys off this file existing, and reads its **`accuracy`** field (in `[0, 1]`).

End-to-end (offline, no API key):

```sh
# 1. generate a submission with the reference agent
python reference/reference_target_agent.py \
    --dataset_dir data/public --working_dir /tmp/arith_run

# 2. grade it
python data/public/evaluate.py --gen-dir /tmp/arith_run
# -> writes /tmp/arith_run/results.json with "accuracy": 1.0
```

## How the native Rust `Verifier` scores this

The crate ships a native [`Verifier`](../../../../src/verifier.rs) trait
(`src/verifier.rs`, default build) that scores a single `(submission, reference)`
pair with the **same `[0, 1]` semantics** as `evaluate.py`'s `accuracy`. For this
task the natural fit is `MultipleChoiceVerifier`, which extracts the chosen A–D
letter and compares it to the reference letter:

```rust
use sia::verifier::{MultipleChoiceVerifier, Verifier};

let v = MultipleChoiceVerifier;
// reference letter for question 1 is "B"
let outcome = v.verify("The model picks B", "B");
assert!(outcome.passed);          // score == 1.0
```

Because the numeric answers are also available, `NumericToleranceVerifier` could
grade the same items against the numeric `correct_answer` (e.g. accept `"43"` for
`17 + 26` within a tolerance). The native verifiers are **complementary** to
`evaluate.py`: use them for fast in-Rust iteration and robustness tests; keep
`evaluate.py` as the authoritative whole-dataset grader.

See `docs/TASK_AUTHORING.md` for how to author a high-quality task and a robust,
Goodhart-resistant verifier.

## Constraints

- Answer **only** from the four given option strings; do not invent a fifth.
- Follow the output format exactly so automated grading can map your letter.
- Questions are independent.
