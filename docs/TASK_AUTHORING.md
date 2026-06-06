# Authoring a high-quality task + robust verifier

This guide covers how to add a new SIA task and — just as importantly — how to
write a **robust verifier** for it. In a self-improving loop the verifier *is*
the objective: the agent optimizes against whatever the verifier rewards, so a
weak verifier is the single biggest source of silent failure. The SIA paper
repeatedly calls out verifier quality as a key limitation and a direct
**Goodhart's law** risk ("when a measure becomes a target, it ceases to be a good
measure"). This document ties the mechanics of task authoring to that concern.

A complete, runnable example of everything below lives in
[`sia/tasks/arithmetic-mc/`](../sia/tasks/arithmetic-mc/) — a tiny arithmetic
multiple-choice task that needs no dataset and no API key.

## 1. Task directory layout

Mirror the existing tasks (`gpqa`, `spaceship-titanic`, `arithmetic-mc`):

```
sia/tasks/<task>/
  data/
    public/                 # what the target agent sees
      task.md               # the task description + output contract
      <questions/data>      # inputs WITHOUT answers
      evaluate.py           # the canonical grader (see §3)
    private/
      <questions/data>      # ground truth WITH answers (grader-only)
  reference/
    reference_target_agent.py   # a known-good solver (the baseline)
    SAMPLE_TASK_DESCRIPTIONS.md  # optional: example items
```

Key invariant: **the agent only ever sees `data/public/`.** Answers live in
`data/private/` and are read solely by `evaluate.py`. Leaking answers into the
public directory makes the benchmark meaningless.

## 2. Writing a clear `task.md`

A good `task.md`:

- States the **objective and the score** explicitly (e.g. "accuracy = correct /
  attempted, in `[0, 1]`").
- Documents the **exact output format** the agent must produce, with a JSON
  example.
- Documents the **`evaluate.py` contract** (how to invoke it, where it reads
  ground truth, what file it writes).
- Lists constraints that prevent trivial gaming (e.g. "answer only from the given
  options").

## 3. The `evaluate.py` contract

Every task is graded by an `evaluate.py` in `data/public/`. The orchestrator runs
it as a subprocess (see `src/orchestrator.rs::run_evaluation`):

```
python evaluate.py --gen-dir <generation_dir>
```

It must:

1. Load ground truth from `data/private/` (relative to the script).
2. Discover the agent's submission JSON under `<generation_dir>` (conventionally
   in a `results/` subdirectory).
3. Write a **`results.json`** into `<generation_dir>`. The orchestrator treats the
   existence of this file as success and reads its **`accuracy`** field, which
   **must be in `[0, 1]`** (1.0 = fully correct).

Keep the grader dependency-light (standard library only where possible) so the
task runs anywhere.

## 4. Designing a robust verifier (the Goodhart-resistant part)

A verifier should measure **correctness**, not **format compliance**. The failure
mode the paper warns about: a verifier that accepts `"B"` but rejects
`"The answer is B."` does not measure whether the agent is right — it measures
whether the agent matched a string. The optimizer will then learn to satisfy the
format rather than solve the task. Concretely:

- **Normalize before comparing.** Trim whitespace, fold case, and *extract* the
  meaningful token (a letter, a number) from surrounding prose rather than
  requiring an exact string.
- **Never panic on malformed input.** Treat unparseable submissions as a failing
  score with an explanatory message, not a crash.
- **Use partial credit where it reflects reality** (e.g. fraction of required
  keywords present) and a clear pass threshold.
- **Test stability against adversarial perturbations.** If trivial reformatting
  changes the score, the verifier is brittle.

### The native Rust `Verifier` layer

The crate provides a native, fully-offline `Verifier` trait in
[`src/verifier.rs`](../src/verifier.rs) (on the default build) that encodes these
principles. It is **complementary** to `evaluate.py`: same `[0, 1]` score
semantics, but it scores a single `(submission, reference)` pair in-process —
ideal for fast iteration and for unit-testing a grader's robustness. It does not
replace `evaluate.py` for whole-dataset aggregation.

Reusable verifiers:

| Verifier                   | Use for                                  |
| -------------------------- | ---------------------------------------- |
| `ExactMatchVerifier`       | short canonical strings (trim + ci)      |
| `MultipleChoiceVerifier`   | A–D choices (GPQA `Answer` semantics)    |
| `NumericToleranceVerifier` | numeric answers within `\|a-b\| <= tol`  |
| `ContainsVerifier`         | keyword / substring partial credit       |

```rust
use sia::verifier::{MultipleChoiceVerifier, Verifier, is_stable};

let v = MultipleChoiceVerifier;
let outcome = v.verify("The model picks B", "B");
assert!(outcome.passed);                 // score == 1.0
assert!(is_stable(&v, "42", "42"));      // robust to reformatting (numeric)
```

### Robustness hooks

Two helpers make verifier robustness *testable*:

- `adversarial_variants(submission) -> Vec<String>` produces
  semantically-equivalent perturbations of a submission (extra whitespace, case
  flip, trailing punctuation, prose wrapping).
- `is_stable(&verifier, submission, reference) -> bool` returns `true` iff the
  verifier's outcome is invariant across every variant.

Use these in your tests: a robust verifier should be `is_stable`; a brittle,
format-only one will not be, and asserting the difference guards against
Goodhart-style regressions. `src/verifier.rs`'s own tests demonstrate both a
stable verifier (numeric extraction across all variants) and an unstable one
(strict exact match).

## 5. Checklist for a new task

- [ ] `data/public/task.md` documents objective, output format, and the
      `evaluate.py` contract.
- [ ] Ground truth lives only in `data/private/`; public inputs have no answers.
- [ ] `evaluate.py` reads private ground truth, discovers the submission, and
      writes `results.json` with `accuracy` in `[0, 1]`.
- [ ] A `reference/reference_target_agent.py` produces a valid submission (a
      known-good baseline).
- [ ] The verifier normalizes/extracts, handles malformed input without panic,
      and is `is_stable` across `adversarial_variants`.
- [ ] The whole pipeline runs end-to-end (ideally offline for demos).
