//! Native-Rust weight-update abstraction + a CPU reference LoRA (issue #19).
//!
//! # Why this module exists
//!
//! Issue #19 — *"LoRA / Test-Time Training / Weight Update Integration"* — asks
//! for a path from the harness-only self-improvement loop (which only edits the
//! agent's *prompt/scaffold*) toward actually **updating model weights** from
//! high-reward trajectories. This module is the first, honest step: a small,
//! fully-offline **abstraction** plus a **tested CPU reference LoRA updater**
//! that runs the low-rank-adapter gradient mechanics end-to-end in pure Rust,
//! plus the **trigger seam** that decides *when* a weight update is worth trying.
//!
//! # Native-Rust-first strategy (and the Candle roadmap)
//!
//! The intended *real* backend is [Candle](https://github.com/huggingface/candle)
//! (`candle-core` + `candle-nn`), HuggingFace's native-Rust ML framework, which
//! would supply GPU tensors, autograd, and a production LoRA implementation. We
//! deliberately do **not** depend on it here for two reasons:
//!
//! 1. **It is not available offline.** `candle-core` is not present in this
//!    repo's offline cargo cache, so it cannot be added in the current build
//!    environment without breaking reproducible offline builds.
//! 2. **Honest scaffolding over fake completeness.** A real GPU LoRA trainer is
//!    a large effort; shipping the *abstraction* and a *correct, tested* CPU
//!    reference first lets downstream work (the scheduler in #65) integrate
//!    against a stable seam now, and swap in Candle later behind a cargo feature.
//!
//! When Candle becomes cache-available, the plan is to add a non-default
//! `weights-gpu` (a.k.a. `candle`) cargo feature that pulls in `candle-core` /
//! `candle-nn` and provides a `CandleLoraUpdater` implementing [`WeightUpdater`].
//! The trait, [`TrainingExample`] extraction, and [`should_trigger_weight_update`]
//! are designed to be reused unchanged by that backend.
//!
//! # The Python bridge is NOT used for training
//!
//! SIA's Python bridge is **target-execution only** (it runs the target agent
//! and the `evaluate.py` grader). Per issue #19 it is explicitly *not* a training
//! path: weight updates are to be done in native Rust (Candle), not by shelling
//! out to a Python trainer. This module honors that — it spawns no subprocess and
//! touches no Python.
//!
//! # What is (and isn't) claimed
//!
//! [`LoraReferenceUpdater`] is a **reference implementation**, not a GPU-scale
//! trainer. It embeds each example into a small fixed-dimensional `f64` feature
//! vector, keeps the LoRA factors as `Vec<Vec<f64>>`, and does plain
//! reward-weighted MSE gradient descent. It demonstrates that the LoRA update
//! mechanics (`W_eff = (B·A) · (alpha/rank)`, gradients through both factors,
//! loss decreasing on a learnable signal) are correct. It does **not** load a
//! real model, run on a GPU, or update a real LLM's parameters. Those are the
//! Candle backend's job.

use serde_json::Value;

/// Hyperparameters for a LoRA-style weight update.
///
/// # LoRA semantics
///
/// A LoRA adapter replaces a full weight delta `ΔW` (shape `out × in`) with the
/// product of two low-rank factors `B` (`out × rank`) and `A` (`rank × in`),
/// scaled by `alpha / rank`:
///
/// ```text
/// W_eff = (B · A) · (alpha / rank)
/// ```
///
/// Only `B` and `A` are trained, so the number of trainable parameters drops
/// from `out · in` to `rank · (out + in)`. The **effective scale** `alpha / rank`
/// decouples the adapter's magnitude from its rank: raising `rank` for more
/// capacity does not also inflate the update size, because the scale shrinks
/// proportionally. With the defaults below the effective scale is
/// `alpha / rank = 8.0 / 4 = 2.0`.
#[derive(Debug, Clone)]
pub struct WeightUpdateConfig {
    /// Rank of the low-rank adapter (`r`). Lower = fewer trainable parameters.
    pub rank: usize,
    /// LoRA `alpha`; the effective adapter scale is `alpha / rank`.
    pub alpha: f64,
    /// Gradient-descent step size.
    pub learning_rate: f64,
    /// Number of full passes over the training examples.
    pub epochs: usize,
}

impl Default for WeightUpdateConfig {
    fn default() -> Self {
        Self {
            rank: 4,
            alpha: 8.0,
            learning_rate: 0.01,
            epochs: 50,
        }
    }
}

impl WeightUpdateConfig {
    /// The LoRA effective scale `alpha / rank` applied to the adapter product.
    ///
    /// Returns `0.0` when `rank == 0` (a degenerate config) rather than dividing
    /// by zero, so callers never panic.
    pub fn effective_scale(&self) -> f64 {
        if self.rank == 0 {
            0.0
        } else {
            self.alpha / self.rank as f64
        }
    }
}

/// A single supervised training pair distilled from a trajectory.
///
/// `reward` is the generation's score (typically the `evaluate.py` accuracy in
/// `[0, 1]`, the same semantics as [`crate::verifier::VerifierOutcome::score`]).
/// It is used to **reward-weight** the loss so high-scoring trajectories pull the
/// adapter harder than low-scoring ones.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainingExample {
    /// The user prompt / input that elicited the response.
    pub input: String,
    /// The assistant response we want to reinforce.
    pub target: String,
    /// The generation's score (reward), used to weight this example's loss.
    pub reward: f64,
}

/// Extract `(user prompt → assistant response)` training pairs from a trajectory
/// in the `agent_execution.json` shape, attaching `reward` to each.
///
/// # Trajectory shape
///
/// The input is the JSON array produced by [`crate::llm::AgentTrajectory`]: a
/// list of `{"role", "content"}` message objects where `content` is either a
/// plain string or an array of content blocks (`text` / `tool_use` /
/// `tool_result`).
///
/// # Mapping
///
/// We walk the messages in order and pair each `user` message with the **next**
/// `assistant` message that follows it. Concretely:
///
/// * The user input text is the concatenation of all `text` content of the user
///   message (a plain-string content counts as its text). `tool_result` blocks
///   are skipped — they are tool plumbing, not a human prompt.
/// * The assistant target text is the concatenation of all `text` blocks of the
///   first following assistant message. `tool_use` blocks are skipped — we
///   reinforce the model's *natural-language* answer, not its tool calls.
/// * If either side has no text after that filtering, the pair is dropped (an
///   assistant turn that was purely a tool call yields no target).
/// * Every emitted pair carries the same `reward`.
///
/// # Robustness
///
/// The function never panics. A non-array `trajectory`, missing `role`/`content`,
/// unexpected block shapes, or an empty array all yield `Vec::new()` or simply
/// fewer pairs — whatever can be salvaged is returned.
pub fn extract_training_examples(trajectory: &Value, reward: f64) -> Vec<TrainingExample> {
    let mut examples = Vec::new();
    let Some(messages) = trajectory.as_array() else {
        return examples;
    };

    let mut i = 0;
    while i < messages.len() {
        let role = messages[i].get("role").and_then(Value::as_str);
        if role != Some("user") {
            i += 1;
            continue;
        }

        let input = message_text(&messages[i]);
        if input.is_empty() {
            // A user turn with no human text (e.g. only a tool_result): skip it.
            i += 1;
            continue;
        }

        // Find the next assistant message after this user turn.
        let mut j = i + 1;
        while j < messages.len()
            && messages[j].get("role").and_then(Value::as_str) != Some("assistant")
        {
            j += 1;
        }
        if j >= messages.len() {
            break; // No assistant response follows; nothing more to pair.
        }

        let target = message_text(&messages[j]);
        if !target.is_empty() {
            examples.push(TrainingExample {
                input: input.clone(),
                target,
                reward,
            });
        }

        // Continue scanning after the assistant turn we just consumed.
        i = j + 1;
    }

    examples
}

/// Concatenate the human/natural-language text of one message.
///
/// Plain-string content is returned verbatim. Array content contributes the
/// `text` field of every `text` block (and any block carrying a `text` field),
/// joined by spaces; `tool_use` / `tool_result` blocks are ignored.
fn message_text(message: &Value) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };

    if let Some(s) = content.as_str() {
        return s.trim().to_string();
    }

    let Some(blocks) = content.as_array() else {
        return String::new();
    };

    let mut parts = Vec::new();
    for block in blocks {
        let btype = block.get("type").and_then(Value::as_str);
        // Skip tool plumbing; reinforce only natural-language text.
        if btype == Some("tool_use") || btype == Some("tool_result") {
            continue;
        }
        if let Some(text) = block.get("text").and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }
    parts.join(" ")
}

/// Outcome of one [`WeightUpdater::update`] call.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightUpdateOutcome {
    /// Number of training examples the update actually consumed.
    pub num_examples: usize,
    /// Reward-weighted MSE before training.
    pub loss_before: f64,
    /// Reward-weighted MSE after training.
    pub loss_after: f64,
    /// Whether weights were updated (false on empty input).
    pub updated: bool,
    /// Human-readable summary of what happened.
    pub details: String,
}

/// A pluggable weight-update backend.
///
/// The CPU reference lives in [`LoraReferenceUpdater`]; a future Candle-backed
/// GPU implementation (behind a `weights-gpu` feature) would implement this same
/// trait so the scheduler in #65 is backend-agnostic.
pub trait WeightUpdater {
    /// Stable identifier for logging / telemetry (e.g. `"lora-reference-cpu"`).
    fn name(&self) -> &str;

    /// Run a weight update on `examples`, returning before/after loss and a
    /// summary. Implementations must be panic-free on empty input.
    fn update(&mut self, examples: &[TrainingExample]) -> WeightUpdateOutcome;
}

/// Dimensionality of the fixed feature space the reference updater embeds into.
const FEATURE_DIM: usize = 16;

/// A CPU reference LoRA updater operating on a small fixed feature space.
///
/// This is the end-to-end demonstration of the LoRA update mechanics in pure
/// Rust (no deps, no GPU). It:
///
/// 1. Embeds each [`TrainingExample::input`] into a fixed `FEATURE_DIM` `f64`
///    feature vector via a deterministic hashed bag-of-bytes (each input byte
///    bumps one feature bucket; the vector is L2-normalized).
/// 2. Embeds each [`TrainingExample::target`] into a scalar regression label in
///    `[0, 1]` (a deterministic hash of the target string), so the synthetic
///    task is "predict a target-dependent number from the input features".
/// 3. Keeps a LoRA adapter as factors `a` (`rank × FEATURE_DIM`) and `b`
///    (`1 × rank`); the effective row vector is `w_eff = (b · a) · (alpha/rank)`
///    and the prediction is `w_eff · features`.
/// 4. Runs `epochs` of reward-weighted MSE gradient descent, updating **both**
///    factors (true LoRA: the full delta is never materialized as trainable
///    parameters).
///
/// On a learnable synthetic signal this reduces the loss; that is asserted in
/// this module's tests. It is a reference, **not** a GPU-scale trainer.
#[derive(Debug, Clone)]
pub struct LoraReferenceUpdater {
    config: WeightUpdateConfig,
    /// LoRA `A` factor, shape `rank × FEATURE_DIM`.
    a: Vec<Vec<f64>>,
    /// LoRA `B` factor, shape `1 × rank` (single-output regression head).
    b: Vec<Vec<f64>>,
}

impl LoraReferenceUpdater {
    /// Create a reference updater with the given config and deterministically
    /// seeded (non-zero) adapter factors.
    ///
    /// `B` would be zero-initialized in a real LoRA (so the adapter starts as a
    /// no-op); here `B` is seeded with a tiny deterministic value so the very
    /// first gradient w.r.t. `A` is non-zero, keeping the reference learnable
    /// from step one without any randomness.
    pub fn new(config: WeightUpdateConfig) -> Self {
        let rank = config.rank.max(1);
        // A: small deterministic ramp so rows differ; B: tiny constant seed.
        let a: Vec<Vec<f64>> = (0..rank)
            .map(|r| {
                (0..FEATURE_DIM)
                    .map(|c| 0.01 * (((r * FEATURE_DIM + c) % 7) as f64 - 3.0))
                    .collect()
            })
            .collect();
        let b: Vec<Vec<f64>> = vec![(0..rank).map(|_| 0.05).collect()];
        Self { config, a, b }
    }

    /// The effective single-output weight row `w_eff = (b · a) · (alpha/rank)`.
    fn effective_weights(&self) -> Vec<f64> {
        let scale = self.config.effective_scale();
        let rank = self.b[0].len();
        let mut w = vec![0.0; FEATURE_DIM];
        for (k, &bk) in self.b[0].iter().enumerate().take(rank) {
            for (c, wc) in w.iter_mut().enumerate() {
                *wc += bk * self.a[k][c];
            }
        }
        for wc in &mut w {
            *wc *= scale;
        }
        w
    }

    /// Reward-weighted MSE of the current adapter over `(features, label, weight)`.
    fn loss(&self, data: &[(Vec<f64>, f64, f64)]) -> f64 {
        if data.is_empty() {
            return 0.0;
        }
        let w = self.effective_weights();
        let mut total = 0.0;
        let mut weight_sum = 0.0;
        for (features, label, weight) in data {
            let pred = dot(&w, features);
            let err = pred - label;
            total += weight * err * err;
            weight_sum += weight;
        }
        if weight_sum == 0.0 {
            0.0
        } else {
            total / weight_sum
        }
    }
}

impl WeightUpdater for LoraReferenceUpdater {
    fn name(&self) -> &str {
        "lora-reference-cpu"
    }

    fn update(&mut self, examples: &[TrainingExample]) -> WeightUpdateOutcome {
        if examples.is_empty() {
            return WeightUpdateOutcome {
                num_examples: 0,
                loss_before: 0.0,
                loss_after: 0.0,
                updated: false,
                details: "no training examples; weights unchanged".to_string(),
            };
        }

        // Build the synthetic supervised dataset: (features, label, reward-weight).
        // Rewards <= 0 contribute no gradient; we floor the weight at a tiny
        // positive value only for the loss denominator via weight_sum handling.
        let data: Vec<(Vec<f64>, f64, f64)> = examples
            .iter()
            .map(|ex| {
                (
                    embed_input(&ex.input),
                    embed_target(&ex.target),
                    ex.reward.max(0.0),
                )
            })
            .collect();

        let scale = self.config.effective_scale();
        let lr = self.config.learning_rate;
        let rank = self.b[0].len();

        let loss_before = self.loss(&data);

        for _ in 0..self.config.epochs {
            // Accumulate gradients over the (reward-weighted) batch.
            let mut grad_a = vec![vec![0.0; FEATURE_DIM]; rank];
            let mut grad_b = vec![0.0; rank];
            let mut weight_sum = 0.0;

            let w = self.effective_weights();
            for (features, label, weight) in &data {
                if *weight == 0.0 {
                    continue;
                }
                weight_sum += weight;
                let pred = dot(&w, features);
                // d/dpred of weight*(pred-label)^2 = 2*weight*(pred-label).
                let dloss_dpred = 2.0 * weight * (pred - label);

                // pred = sum_c w_eff[c]*x[c], w_eff[c] = scale * sum_k b[k]*a[k][c].
                // dpred/db[k]    = scale * sum_c a[k][c]*x[c]
                // dpred/da[k][c] = scale * b[k] * x[c]
                for k in 0..rank {
                    let bk = self.b[0][k];
                    let mut dpred_dbk = 0.0;
                    for (c, &xc) in features.iter().enumerate() {
                        dpred_dbk += self.a[k][c] * xc;
                        grad_a[k][c] += dloss_dpred * scale * bk * xc;
                    }
                    grad_b[k] += dloss_dpred * scale * dpred_dbk;
                }
            }

            if weight_sum == 0.0 {
                break; // No positive-reward signal; nothing to learn from.
            }

            // Mean gradient step.
            for k in 0..rank {
                self.b[0][k] -= lr * grad_b[k] / weight_sum;
                for (c, ac) in self.a[k].iter_mut().enumerate() {
                    *ac -= lr * grad_a[k][c] / weight_sum;
                }
            }
        }

        let loss_after = self.loss(&data);
        let details = format!(
            "{}: {} example(s), rank {}, scale {:.4}, loss {:.6} -> {:.6}",
            self.name(),
            data.len(),
            rank,
            scale,
            loss_before,
            loss_after,
        );
        WeightUpdateOutcome {
            num_examples: examples.len(),
            loss_before,
            loss_after,
            updated: true,
            details,
        }
    }
}

/// Dot product of two equal-length vectors (truncates to the shorter length).
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Deterministic FNV-1a 64-bit hash of a byte slice (no deps, stable).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Embed an input string into a fixed `FEATURE_DIM` feature vector.
///
/// Hashed bag-of-bytes: each byte increments one feature bucket (bucket chosen
/// by the byte value), then the vector is L2-normalized so magnitude is bounded.
/// Empty / whitespace inputs yield a small constant bias feature so they are not
/// all-zero (which would make the prediction trivially zero regardless of `W`).
fn embed_input(input: &str) -> Vec<f64> {
    let mut features = vec![0.0; FEATURE_DIM];
    for &byte in input.as_bytes() {
        features[byte as usize % FEATURE_DIM] += 1.0;
    }
    let norm: f64 = features.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm == 0.0 {
        features[0] = 1.0; // bias feature for empty input
    } else {
        for f in &mut features {
            *f /= norm;
        }
    }
    features
}

/// Embed a target string into a scalar regression label in `[0, 1]`.
///
/// A deterministic hash of the target, mapped into the unit interval. This makes
/// the synthetic task "predict a target-dependent number from input features",
/// which is learnable when distinct inputs map to distinct targets.
fn embed_target(target: &str) -> f64 {
    (fnv1a(target.as_bytes()) % 1000) as f64 / 1000.0
}

/// Decide whether it's time to attempt a weight update instead of more
/// prompt-only iteration — the trigger seam the scheduler (#65) builds on.
///
/// # Policy
///
/// The harness-only loop improves the agent by editing its prompt/scaffold. When
/// that improvement *plateaus*, further prompt iteration has diminishing returns
/// and a weight update becomes the more promising move. We detect a plateau by
/// looking at the most recent score deltas:
///
/// * Let `recent_scores` be per-generation scores in chronological order.
/// * Compute the consecutive deltas `s[i+1] - s[i]`.
/// * Return `true` when the **last `K` deltas are all `< plateau_eps`** (where
///   `K = 3`, or all available deltas if fewer), i.e. recent improvement is
///   below the meaningful-progress threshold. Negative deltas (regressions) also
///   count as "plateaued".
///
/// # Edge cases
///
/// Needs at least two scores to form one delta; with fewer than two scores (or
/// an empty slice) there is no evidence of a plateau, so it returns `false`. A
/// non-positive `plateau_eps` makes the trigger effectively never fire on any
/// real improvement, which is a safe default.
pub fn should_trigger_weight_update(recent_scores: &[f64], plateau_eps: f64) -> bool {
    /// Number of trailing deltas that must all be below `plateau_eps`.
    const PLATEAU_WINDOW: usize = 3;

    if recent_scores.len() < 2 {
        return false;
    }

    let deltas: Vec<f64> = recent_scores
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();

    let window = PLATEAU_WINDOW.min(deltas.len());
    deltas
        .iter()
        .rev()
        .take(window)
        .all(|&delta| delta < plateau_eps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_config_values_and_effective_scale() {
        let c = WeightUpdateConfig::default();
        assert_eq!(c.rank, 4);
        assert_eq!(c.alpha, 8.0);
        assert_eq!(c.learning_rate, 0.01);
        assert_eq!(c.epochs, 50);
        // Effective scale = alpha / rank = 8.0 / 4 = 2.0 (doc check).
        assert_eq!(c.effective_scale(), 2.0);
    }

    #[test]
    fn effective_scale_handles_zero_rank() {
        let c = WeightUpdateConfig {
            rank: 0,
            ..Default::default()
        };
        assert_eq!(c.effective_scale(), 0.0);
    }

    #[test]
    fn extract_pairs_from_synthetic_trajectory() {
        // user(text) -> assistant(text), then a tool loop that yields no pair,
        // then user(text) -> assistant(text) again.
        let traj = json!([
            {"role": "user", "content": "what is 2+2?"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "The answer is 4."}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}
            ]},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "calc", "input": {}}
            ]},
            {"role": "user", "content": "and 3+3?"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Six."}
            ]},
        ]);

        let examples = extract_training_examples(&traj, 0.75);
        assert_eq!(
            examples,
            vec![
                TrainingExample {
                    input: "what is 2+2?".to_string(),
                    target: "The answer is 4.".to_string(),
                    reward: 0.75,
                },
                TrainingExample {
                    input: "and 3+3?".to_string(),
                    target: "Six.".to_string(),
                    reward: 0.75,
                },
            ]
        );
    }

    #[test]
    fn extract_skips_assistant_only_tool_use_turn() {
        // A user prompt followed only by a tool_use assistant turn yields no
        // pair (no natural-language target), but the trailing real answer does.
        let traj = json!([
            {"role": "user", "content": "look up x"},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "search", "input": {}}
            ]},
            {"role": "assistant", "content": [
                {"type": "text", "text": "x is 42"}
            ]},
        ]);
        let examples = extract_training_examples(&traj, 1.0);
        // The user pairs with the FIRST following assistant (the tool_use turn),
        // which has no text, so the pair is dropped — no panic, empty result.
        assert_eq!(examples, vec![]);
    }

    #[test]
    fn extract_is_robust_to_empty_and_odd_shapes() {
        assert_eq!(extract_training_examples(&json!([]), 1.0), vec![]);
        assert_eq!(extract_training_examples(&json!({}), 1.0), vec![]);
        assert_eq!(extract_training_examples(&json!("nope"), 1.0), vec![]);
        assert_eq!(extract_training_examples(&json!(42), 1.0), vec![]);
        // Messages missing role/content must not panic.
        let weird = json!([
            {"content": "no role"},
            {"role": "user"},
            {"role": "assistant", "content": 99},
        ]);
        assert_eq!(extract_training_examples(&weird, 1.0), vec![]);
    }

    /// Build a tiny *learnable* dataset: distinct inputs mapping to distinct
    /// hashed targets, all with positive reward.
    fn learnable_dataset() -> Vec<TrainingExample> {
        vec![
            TrainingExample {
                input: "alpha beta gamma".to_string(),
                target: "first".to_string(),
                reward: 1.0,
            },
            TrainingExample {
                input: "delta epsilon zeta".to_string(),
                target: "second".to_string(),
                reward: 1.0,
            },
            TrainingExample {
                input: "eta theta iota".to_string(),
                target: "third".to_string(),
                reward: 0.5,
            },
        ]
    }

    #[test]
    fn lora_reference_reduces_loss_on_learnable_signal() {
        let mut updater = LoraReferenceUpdater::new(WeightUpdateConfig {
            // More epochs / a slightly larger lr to make the decrease unambiguous.
            epochs: 500,
            learning_rate: 0.5,
            ..Default::default()
        });
        let examples = learnable_dataset();
        let outcome = updater.update(&examples);

        assert!(outcome.updated);
        assert_eq!(outcome.num_examples, 3);
        assert_eq!(updater.name(), "lora-reference-cpu");
        assert!(
            outcome.loss_after < outcome.loss_before,
            "loss must decrease: {} -> {}",
            outcome.loss_before,
            outcome.loss_after
        );
        assert!(outcome.loss_after >= 0.0);
        assert!(outcome.details.contains("lora-reference-cpu"));
    }

    #[test]
    fn lora_reference_empty_input_no_update_no_panic() {
        let mut updater = LoraReferenceUpdater::new(WeightUpdateConfig::default());
        let outcome = updater.update(&[]);
        assert!(!outcome.updated);
        assert_eq!(outcome.num_examples, 0);
        assert_eq!(outcome.loss_before, 0.0);
        assert_eq!(outcome.loss_after, 0.0);
    }

    #[test]
    fn lora_reference_zero_reward_does_not_panic() {
        // All-zero reward => no positive-weight signal; must not panic or NaN.
        let mut updater = LoraReferenceUpdater::new(WeightUpdateConfig::default());
        let examples = vec![TrainingExample {
            input: "x".to_string(),
            target: "y".to_string(),
            reward: 0.0,
        }];
        let outcome = updater.update(&examples);
        assert!(outcome.updated);
        assert_eq!(outcome.num_examples, 1);
        assert!(outcome.loss_after.is_finite());
    }

    #[test]
    fn trigger_false_while_improving() {
        // Steadily rising scores with deltas above eps => not plateaued.
        let scores = [0.1, 0.3, 0.5, 0.7];
        assert!(!should_trigger_weight_update(&scores, 0.05));
    }

    #[test]
    fn trigger_true_once_plateaued() {
        // Early jump then flat: last 3 deltas all below eps => plateaued.
        let scores = [0.1, 0.6, 0.61, 0.611, 0.6111];
        assert!(should_trigger_weight_update(&scores, 0.05));
    }

    #[test]
    fn trigger_counts_regression_as_plateau() {
        // Deltas negative/tiny => below eps => trigger.
        let scores = [0.8, 0.79, 0.78, 0.781];
        assert!(should_trigger_weight_update(&scores, 0.05));
    }

    #[test]
    fn trigger_handles_short_and_empty_slices() {
        assert!(!should_trigger_weight_update(&[], 0.05));
        assert!(!should_trigger_weight_update(&[0.5], 0.05));
        // Exactly two scores: one delta; below eps => trigger.
        assert!(should_trigger_weight_update(&[0.5, 0.5001], 0.05));
        // Two scores improving above eps => no trigger.
        assert!(!should_trigger_weight_update(&[0.1, 0.5], 0.05));
    }
}
