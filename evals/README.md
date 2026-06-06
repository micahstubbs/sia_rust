# `sia-evals` — GPQA-style eval harness on `dspy-rs`

A small, self-contained eval harness for SIA that mirrors the bundled Python
`sia/tasks/gpqa` evaluator, implemented on top of [`dspy-rs`] — the DSRs Rust
port/rewrite of DSPy.

[`dspy-rs`]: https://github.com/krypticmouse/DSRs

It runs **fully offline in CI** (no network, no API keys) via a deterministic
mock model, and can also run against a real LLM provider when an API key is
present.

## What it is

* **Task:** GPQA-style multiple choice (graduate-level science questions). Input
  is a question stem plus options `A`–`D`; output is a single letter `A`–`D`.
  This mirrors `sia/tasks/gpqa`.
* **Metric:** accuracy = `correct / attempted`, also reported as
  `accuracy_percent`, exactly like `sia/tasks/gpqa/data/public/evaluate.py`
  (including its `normalize_answer` behaviour: uppercase, then take the first
  `A`–`D` character found).
* **Dataset:** a small in-repo fixture, `fixtures/gpqa_sample.json` (a handful
  of questions with known `correct_answer_letter`s).

## How `dspy-rs` is used

This is a real `dspy-rs` pipeline, not a reimplementation:

| Concern                | dspy-rs construct used                                                       |
| ---------------------- | --------------------------------------------------------------------------- |
| Task I/O contract      | `#[Signature]` → `GpqaSignature { question -> answer }`                      |
| Prediction step        | `dspy_rs::Predict` over the signature                                        |
| Pipeline               | `dspy_rs::Module` (`GpqaModule`)                                             |
| Evaluation harness     | `dspy_rs::Evaluator` (`GpqaModule::metric` + framework `evaluate()`)         |
| Model injection point  | `dspy_rs::Adapter` (`MockAdapter` for offline, `ChatAdapter` for real)       |
| Global config          | `dspy_rs::configure(lm, adapter)`                                            |

### Why a custom `Adapter` instead of a custom LM

In `dspy-rs 0.7.3` the language model is a concrete `LM` struct (with a private
HTTP client), not a trait you can implement. The clean, supported injection
point is the `Adapter` trait, which `Predict::forward` invokes as
`adapter.call(lm, signature, inputs, tools)`. `MockAdapter`:

* delegates prompt **formatting** and response **parsing** to the real
  `ChatAdapter` (so the genuine dspy-rs wire format `[[ ## answer ## ]] …` is
  exercised), and
* overrides `call` to return a deterministic answer for each question from a
  lookup map — the `LM` argument is never used, so **no network request is ever
  made**.

The offline `LM` itself is built via `offline_lm()` using a local/dummy base URL
(dspy-rs's "local OpenAI-compatible" path), which constructs without any API key
and without contacting a server.

## Run it offline (CI / no keys)

```sh
cd evals
cargo build
cargo test
```

The tests assert:

* a **perfect** mock (answers each question with its correct letter) → **100%**
  accuracy, and
* a deliberately-wrong **always-"A"** mock → a known lower score (20% on the
  bundled fixture, since one of five gold answers is `A`).

There is also a runnable demo that prints an `evaluate.py`-style report:

```sh
cd evals
cargo run --example run_eval        # OFFLINE, prints 100% with the perfect mock
```

## Run it against a real provider

Build the LM with `real_lm("provider:model")` and configure with the real
`ChatAdapter`. The provider and key come from the environment:

| Provider  | Model string example                         | Required env var     |
| --------- | -------------------------------------------- | -------------------- |
| OpenAI    | `openai:gpt-4o-mini`                         | `OPENAI_API_KEY`     |
| Anthropic | `anthropic:claude-3-5-sonnet-latest`         | `ANTHROPIC_API_KEY`  |

Using the bundled example:

```sh
cd evals
EVAL_MODEL=openai:gpt-4o-mini OPENAI_API_KEY=sk-... cargo run --example run_eval -- --real
# or
EVAL_MODEL=anthropic:claude-3-5-sonnet-latest ANTHROPIC_API_KEY=sk-... cargo run --example run_eval -- --real
```

Or in code:

```rust
use dspy_rs::{configure, ChatAdapter, Module};
use sia_evals::{load_fixtures, real_lm, score, GpqaModule, GpqaQuestion};

let questions = load_fixtures()?;
let examples: Vec<_> = questions.iter().map(GpqaQuestion::to_example).collect();

configure(real_lm("openai:gpt-4o-mini").await?, ChatAdapter);

let module = GpqaModule::new();
let preds = module.batch(examples.clone(), 8, false).await?;
let report = score(&examples, &preds);
println!("{:.2}%", report.accuracy_percent);
```

## Layout

```
evals/
├── Cargo.toml                  # standalone crate (NOT a workspace member)
├── README.md
├── fixtures/
│   └── gpqa_sample.json        # in-repo dataset with known answers
├── examples/
│   └── run_eval.rs             # offline/real demo runner
└── src/
    └── lib.rs                  # signature, module, metric, mock adapter, tests
```

## Versions

* `dspy-rs = "=0.7.3"`
* `rig-core = "=0.22.0"` — **must** match the exact `rig` version `dspy-rs 0.7.3`
  depends on, because the public `Adapter` trait signature references
  `rig::tool::ToolDyn`. A different `rig-core` resolves to a second, incompatible
  `rig` crate and the `Adapter` impl fails to compile.

This crate is intentionally **standalone**: it has its own `[package]` with no
`[workspace]` inheritance and does not depend on the repo-root crate, so the root
crate stays independently buildable. If/when the root `sia` crate exposes
reusable types, add `sia = { path = ".." }` to `Cargo.toml`.
