//! GPQA-style multiple-choice eval harness for SIA, built on [`dspy-rs`] (the DSRs
//! Rust port of DSPy).
//!
//! It mirrors the bundled Python `sia/tasks/gpqa` evaluator: each item is a
//! graduate-level multiple-choice question with options `A`-`D` and a known
//! correct letter, and the metric is accuracy = correct / attempted (also
//! reported as `accuracy_percent`), exactly like `evaluate.py`.
//!
//! # How dspy-rs is used
//!
//! * [`GpqaSignature`] is a real dspy-rs `#[Signature]` (`question` in →
//!   `answer` out).
//! * [`GpqaModule`] is a dspy-rs [`dspy_rs::Module`] wrapping a
//!   [`dspy_rs::Predict`] over that signature, and it also implements the
//!   dspy-rs [`dspy_rs::Evaluator`] trait so evaluation runs through the
//!   framework's own `evaluate()` harness.
//! * The model is injected through dspy-rs's [`dspy_rs::Adapter`] abstraction.
//!   [`MockAdapter`] is a fully offline adapter: it reuses `ChatAdapter`'s
//!   prompt formatting/parsing but answers deterministically from a lookup map
//!   instead of calling a provider, so the whole pipeline runs with **no
//!   network and no API keys**.
//!
//! # Offline vs. real provider
//!
//! * Offline (CI / tests): build the LM with [`offline_lm`] and configure with
//!   [`MockAdapter`]. See `cargo test` in this crate.
//! * Real provider: build the LM with [`real_lm`] (reads `OPENAI_API_KEY` /
//!   `ANTHROPIC_API_KEY`) and configure with `dspy_rs::ChatAdapter`. See the
//!   README.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use dspy_rs::{
    adapter::Adapter, Chat, ChatAdapter, Evaluator, Example, LmUsage, Message, MetaSignature,
    Module, Predict, Prediction, Predictor, Signature, LM,
};
use rig::tool::ToolDyn;
use serde::Deserialize;
use serde_json::Value;

/// dspy-rs signature for a single GPQA-style item: a fully rendered
/// multiple-choice `question` (stem + options A-D) in, a single-letter
/// `answer` out.
#[Signature]
pub struct GpqaSignature {
    /// Answer the multiple-choice question. Respond with exactly one letter:
    /// A, B, C, or D.

    #[input]
    pub question: String,

    #[output]
    pub answer: String,
}

/// One question from the fixture dataset (`fixtures/gpqa_sample.json`).
///
/// Field layout mirrors the GPQA task data: a `question` stem, an `options`
/// map keyed by `A`-`D`, and the ground-truth `correct_answer_letter` (as in
/// the Python task's private `diamond_questions.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct GpqaQuestion {
    pub id: u32,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub subdomain: String,
    pub question: String,
    pub options: HashMap<String, String>,
    pub correct_answer_letter: String,
}

impl GpqaQuestion {
    /// Render the stem and options into the single prompt string the signature
    /// consumes. This is also the lookup key the offline [`MockAdapter`] uses.
    pub fn render_prompt(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.question);
        s.push('\n');
        for letter in ["A", "B", "C", "D"] {
            if let Some(text) = self.options.get(letter) {
                s.push_str(&format!("{letter}. {text}\n"));
            }
        }
        s
    }

    /// Build a dspy-rs [`Example`] with `question` as input and the known
    /// `answer` letter as the labelled output.
    pub fn to_example(&self) -> Example {
        let mut data = HashMap::new();
        data.insert("question".to_string(), Value::String(self.render_prompt()));
        data.insert(
            "answer".to_string(),
            Value::String(self.correct_answer_letter.clone()),
        );
        Example::new(
            data,
            vec!["question".to_string()],
            vec!["answer".to_string()],
        )
    }
}

/// Load the bundled fixture dataset shipped at `evals/fixtures/gpqa_sample.json`.
pub fn load_fixtures() -> Result<Vec<GpqaQuestion>> {
    // Compiled in so the dataset is always available regardless of cwd.
    const RAW: &str = include_str!("../fixtures/gpqa_sample.json");
    let questions: Vec<GpqaQuestion> = serde_json::from_str(RAW)?;
    Ok(questions)
}

/// Normalize a model answer to a single `A`-`D` letter, matching
/// `evaluate.py::normalize_answer` (uppercase, then first A-D char found).
pub fn normalize_answer(answer: &str) -> String {
    let upper = answer.trim().to_uppercase();
    if upper.len() == 1 && "ABCD".contains(&upper) {
        return upper;
    }
    for ch in upper.chars() {
        if "ABCD".contains(ch) {
            return ch.to_string();
        }
    }
    String::new()
}

/// An offline dspy-rs [`Adapter`] that answers deterministically from a lookup
/// map instead of calling a real language model.
///
/// It delegates prompt **formatting** and response **parsing** to the real
/// `ChatAdapter`, but overrides `call` to synthesise a dspy-rs-formatted
/// assistant message (`[[ ## answer ## ]] ...`) from the canned answer for the
/// incoming `question`. The `LM` passed in is never used to make a network
/// request, so the full signature -> module -> metric pipeline runs offline.
pub struct MockAdapter {
    /// Map of rendered-question-prompt -> answer letter.
    answers: HashMap<String, String>,
    /// Fallback used when a question is not in the map (e.g. "always wrong"
    /// mocks for the lower-bound accuracy test).
    fallback: String,
}

impl MockAdapter {
    /// Build a mock that answers each fixture question with its *correct*
    /// letter (yields 100% accuracy).
    pub fn perfect(questions: &[GpqaQuestion]) -> Self {
        let answers = questions
            .iter()
            .map(|q| (q.render_prompt(), q.correct_answer_letter.clone()))
            .collect();
        Self {
            answers,
            fallback: "A".to_string(),
        }
    }

    /// Build a mock from an explicit prompt -> answer map, with a `fallback`
    /// letter for unknown prompts. Useful for deliberately-wrong mocks.
    pub fn from_map(answers: HashMap<String, String>, fallback: impl Into<String>) -> Self {
        Self {
            answers,
            fallback: fallback.into(),
        }
    }

    /// Build a mock that always returns the same letter regardless of question.
    pub fn always(letter: impl Into<String>) -> Self {
        Self {
            answers: HashMap::new(),
            fallback: letter.into(),
        }
    }

    fn answer_for(&self, prompt: &str) -> String {
        self.answers
            .get(prompt)
            .cloned()
            .unwrap_or_else(|| self.fallback.clone())
    }
}

#[async_trait]
impl Adapter for MockAdapter {
    fn format(&self, signature: &dyn MetaSignature, inputs: Example) -> Chat {
        ChatAdapter.format(signature, inputs)
    }

    fn parse_response(
        &self,
        signature: &dyn MetaSignature,
        response: Message,
    ) -> HashMap<String, Value> {
        ChatAdapter.parse_response(signature, response)
    }

    async fn call(
        &self,
        _lm: Arc<LM>,
        signature: &dyn MetaSignature,
        inputs: Example,
        _tools: Vec<Arc<dyn ToolDyn>>,
    ) -> Result<Prediction> {
        // NOTE: `_lm` is intentionally unused -> no network call is ever made.
        let prompt = inputs.get("question", None);
        let prompt = prompt.as_str().unwrap_or_default();
        let letter = self.answer_for(prompt);

        // Produce a response in the exact wire format ChatAdapter::parse_response
        // expects, then parse it back -> exercises the real dspy-rs parsing path.
        let raw = format!("[[ ## answer ## ]]\n{letter}\n\n[[ ## completed ## ]]\n");
        let data = self.parse_response(signature, Message::assistant(raw));
        Ok(Prediction::new(data, LmUsage::default()))
    }
}

/// A dspy-rs [`Module`] that answers GPQA-style questions via a [`Predict`]
/// over [`GpqaSignature`], and an [`Evaluator`] that scores accuracy.
pub struct GpqaModule {
    predictor: Predict,
}

impl Default for GpqaModule {
    fn default() -> Self {
        Self::new()
    }
}

impl GpqaModule {
    pub fn new() -> Self {
        Self {
            predictor: Predict::new(GpqaSignature::new()),
        }
    }
}

impl Module for GpqaModule {
    async fn forward(&self, inputs: Example) -> Result<Prediction> {
        self.predictor.forward(inputs).await
    }
}

impl Evaluator for GpqaModule {
    // Keep tests deterministic / quiet.
    const DISPLAY_PROGRESS: bool = false;

    async fn metric(&self, example: &Example, prediction: &Prediction) -> f32 {
        let predicted = prediction.get("answer", None);
        let predicted = normalize_answer(predicted.as_str().unwrap_or_default());

        let gold = example.get("answer", None);
        let gold = normalize_answer(gold.as_str().unwrap_or_default());

        if !predicted.is_empty() && predicted == gold {
            1.0
        } else {
            0.0
        }
    }
}

/// Full scoring breakdown mirroring `evaluate.py`'s output shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreReport {
    pub total: usize,
    pub correct: usize,
    pub incorrect: usize,
    /// Answers that did not normalise to an A-D letter.
    pub invalid: usize,
    /// accuracy = correct / attempted (attempted = correct + incorrect).
    pub accuracy: f32,
    pub accuracy_percent: f32,
}

/// Score predictions against the gold examples the same way `evaluate.py` does:
/// accuracy = correct / attempted, reported as both a fraction and a percent.
pub fn score(examples: &[Example], predictions: &[Prediction]) -> ScoreReport {
    let total = examples.len();
    let mut correct = 0usize;
    let mut incorrect = 0usize;
    let mut invalid = 0usize;

    for (example, prediction) in examples.iter().zip(predictions.iter()) {
        let gold = normalize_answer(example.get("answer", None).as_str().unwrap_or_default());
        let pred = normalize_answer(prediction.get("answer", None).as_str().unwrap_or_default());

        if pred.is_empty() {
            invalid += 1;
        } else if pred == gold {
            correct += 1;
        } else {
            incorrect += 1;
        }
    }

    let attempted = correct + incorrect;
    let accuracy = if attempted > 0 {
        correct as f32 / attempted as f32
    } else {
        0.0
    };

    ScoreReport {
        total,
        correct,
        incorrect,
        invalid,
        accuracy,
        accuracy_percent: 100.0 * accuracy,
    }
}

/// Build an **offline** [`LM`] that constructs without network access and
/// without any API key.
///
/// It points at a local/dummy base URL (dspy-rs's "local OpenAI-compatible"
/// path uses a dummy key and never validates connectivity at build time). When
/// paired with [`MockAdapter`], this client is never actually called.
pub async fn offline_lm() -> Result<LM> {
    LM::builder()
        // base_url set, api_key absent -> dspy-rs `from_local` path: dummy key,
        // no env-var requirement, no network at construction.
        .base_url("http://localhost:0/v1".to_string())
        .model("offline-mock".to_string())
        .build()
        .await
}

/// Build a **real** provider [`LM`] from a `provider:model` string, e.g.
/// `"openai:gpt-4o-mini"` or `"anthropic:claude-3-5-sonnet-latest"`.
///
/// Reads the matching API key from the environment (`OPENAI_API_KEY`,
/// `ANTHROPIC_API_KEY`, ...). Pair this with `dspy_rs::ChatAdapter` to run the
/// eval against a live model. Errors if the required key is missing.
pub async fn real_lm(model: &str) -> Result<LM> {
    LM::builder().model(model.to_string()).build().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspy_rs::{configure, Evaluator};
    use std::sync::Mutex;

    // dspy-rs stores the active LM + adapter in process-global settings
    // (`configure`). Tests that reconfigure those globals must not interleave,
    // so they take this lock for the duration that depends on the config they set.
    static SETTINGS_GUARD: Mutex<()> = Mutex::new(());

    fn examples() -> Vec<Example> {
        load_fixtures()
            .unwrap()
            .iter()
            .map(GpqaQuestion::to_example)
            .collect()
    }

    #[test]
    fn fixtures_load_and_have_valid_gold_letters() {
        let qs = load_fixtures().unwrap();
        assert!(qs.len() >= 4, "expected a handful of questions");
        for q in &qs {
            assert!(
                "ABCD".contains(&q.correct_answer_letter),
                "gold letter must be A-D, got {:?}",
                q.correct_answer_letter
            );
            assert!(q.options.contains_key(&q.correct_answer_letter));
        }
    }

    #[test]
    fn normalize_answer_extracts_letter() {
        // Already a bare letter (any case) -> that letter.
        assert_eq!(normalize_answer("a"), "A");
        assert_eq!(normalize_answer(" D "), "D");
        // First A-D char of the (uppercased) string wins, matching
        // evaluate.py::normalize_answer exactly. "C) sp" -> 'C'.
        assert_eq!(normalize_answer("C) sp"), "C");
        // No A-D letter present -> empty (counts as invalid/incorrect).
        assert_eq!(normalize_answer("42"), "");
    }

    // Full pipeline (signature -> module -> dspy-rs Evaluator) with a perfect
    // mock LM -> 100% accuracy, fully offline.
    #[tokio::test(flavor = "current_thread")]
    async fn perfect_mock_yields_100_percent() {
        // Hold the settings lock for the whole test so a concurrently-running
        // test can't swap out the global adapter mid-run. `current_thread`
        // flavor keeps the future `!Send`-tolerant so we can hold a std guard.
        let _guard = SETTINGS_GUARD.lock().unwrap_or_else(|e| e.into_inner());

        let questions = load_fixtures().unwrap();
        configure(
            offline_lm().await.unwrap(),
            MockAdapter::perfect(&questions),
        );

        let module = GpqaModule::new();
        let exs = examples();

        // dspy-rs's own Evaluator::evaluate -> mean metric over the dataset.
        let mean = module.evaluate(exs.clone()).await;
        assert_eq!(mean, 1.0, "perfect mock should score 1.0 mean metric");

        // And the evaluate.py-style breakdown agrees.
        let preds = module.batch(exs.clone(), 8, false).await.unwrap();
        let report = score(&exs, &preds);
        println!("[perfect mock] {report:?}");
        assert_eq!(report.correct, questions.len());
        assert_eq!(report.accuracy_percent, 100.0);
    }

    // A deliberately-wrong mock that always answers "A": only items whose gold
    // letter is "A" count as correct -> a known lower accuracy.
    #[tokio::test(flavor = "current_thread")]
    async fn always_a_mock_yields_known_lower_score() {
        let _guard = SETTINGS_GUARD.lock().unwrap_or_else(|e| e.into_inner());

        let questions = load_fixtures().unwrap();
        let expected_correct = questions
            .iter()
            .filter(|q| q.correct_answer_letter == "A")
            .count();
        let expected_pct = 100.0 * expected_correct as f32 / questions.len() as f32;

        configure(offline_lm().await.unwrap(), MockAdapter::always("A"));

        let module = GpqaModule::new();
        let exs = examples();
        let preds = module.batch(exs.clone(), 8, false).await.unwrap();
        let report = score(&exs, &preds);
        println!("[always-A mock] {report:?}");

        assert_eq!(report.correct, expected_correct);
        assert!(report.accuracy_percent < 100.0);
        assert!((report.accuracy_percent - expected_pct).abs() < 1e-3);

        // Cross-check against the dspy-rs Evaluator mean as well.
        let mean = module.evaluate(exs).await;
        assert!((mean - expected_correct as f32 / questions.len() as f32).abs() < 1e-6);
    }
}
