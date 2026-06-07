//! Closed-loop wiring: turn the standalone scheduler (#65) and weight-update
//! path (#19) into per-generation, observable artifacts — issue #84.
//!
//! # What this module does (and why it lives here)
//!
//! [`crate::scheduler`] (#65) and [`crate::weights`] (#19) shipped as
//! standalone, fully-tested modules whose docs each describe an *integration
//! seam* but deliberately left the orchestrator untouched. This module is that
//! integration, kept **out of** `orchestrator.rs` so the wiring is small,
//! conflict-light, and obviously additive: the orchestrator only calls two
//! free functions here, right after evaluation.
//!
//! Two artifacts are produced per generation, both best-effort:
//!
//! 1. [`record_scheduler_decision`] reads the score history (`results.json`)
//!    and per-generation compute cost (`telemetry.json` total tokens) of every
//!    generation so far, feeds them to an [`AdaptiveScheduler`], and writes
//!    `<gen_dir>/scheduler_decision.json` recording whether the *next* update
//!    should pull the harness or the weight lever, with a human-readable
//!    rationale and the efficiency summary.
//! 2. [`maybe_run_weight_update`] — only when the decision is `"weight"` /
//!    `"both"` — extracts training examples from this generation's trajectory
//!    ([`crate::weights::extract_training_examples`]), runs the CPU reference
//!    LoRA ([`crate::weights::LoraReferenceUpdater`]), and writes
//!    `<gen_dir>/weight_update.json` with the before/after loss.
//! 3. [`record_acted_decision`] (issue #90) annotates
//!    `<gen_dir>/scheduler_decision.json` with the [`ActedDecision`] the
//!    orchestrator actually executed and the weight-update summary, so the loop
//!    surfaces what it *did*, not just what it recommended.
//!
//! # Closing the loop (issue #90)
//!
//! Through #84 this module was **observational** — it only wrote artifacts and a
//! log line. As of #90 the orchestrator *acts* on the decision via
//! [`action_for_decision`]: a `weight` decision runs the weight update and
//! short-circuits the feedback (harness) step for that generation, `both` runs
//! both, and `harness` keeps today's behavior.
//!
//! The control-flow change is gated on a decision actually being produced.
//! [`record_scheduler_decision`] still degrades to `None` when inputs are missing
//! (no score yet, no telemetry/trajectory), and the orchestrator maps a missing
//! decision to the harness path — so its existing tests (which drive
//! `run_generation_with` with mock fns and no `results.json`) are unaffected and
//! the default path stays byte-for-byte as before. Every function here remains
//! panic-free and never mutates a pre-existing deterministic output
//! (`context.md`, the parity-checked feedback context, `results.json`); the only
//! additions are the JSON artifacts and the acted-decision keys.

use std::path::Path;

use serde_json::{json, Value};

use crate::layout::{names, RunLayout};
use crate::scheduler::{AdaptiveScheduler, GenerationRecord, SchedulerConfig, UpdateKind};
use crate::weights::{
    extract_training_examples, StubWeightUpdater, WeightUpdateConfig, WeightUpdateOutcome,
    WeightUpdater,
};

/// Filename for the per-generation scheduler decision artifact (#65 wiring).
pub const SCHEDULER_DECISION_JSON: &str = "scheduler_decision.json";

/// Filename for the per-generation weight-update artifact (#19 wiring).
pub const WEIGHT_UPDATE_JSON: &str = "weight_update.json";

/// Filename of the per-generation token/timing telemetry (mirrors
/// [`crate::llm::telemetry::TELEMETRY_JSON`]; duplicated as a `&str` so this
/// module compiles without the optional `llm` feature).
const TELEMETRY_FILENAME: &str = "telemetry.json";

/// Fallback compute cost used when a generation has no `telemetry.json` (or it
/// carries no token totals). A constant positive value keeps
/// [`AdaptiveScheduler::improvement_efficiency`] well-defined (it divides score
/// improvement by this) without crediting any lever with free compute.
const FALLBACK_COMPUTE_COST: f64 = 1.0;

// --------------------------------------------------------------------------- //
// Small robust readers (no panic, best-effort)
// --------------------------------------------------------------------------- //

/// Parse JSON from `path`, returning `None` on any IO / parse error.
fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Read a generation's accuracy score normalized to `[0, 1]` from its
/// evaluation results.
///
/// Mirrors the web dashboard's reader (`web::runs::eval_summary`): prefer
/// `evaluation_results.json`, then `results.json` (so the scheduler's score
/// series matches the series SIA Studio charts), and within a file prefer
/// `accuracy_percent` (the authoritative percent), then `accuracy` (a fraction
/// by convention), then `correct/total`. Because some task evaluators write
/// `accuracy` on a 0–100 scale, any value `> 1.0` is treated as a percent and
/// divided by 100 — keeping the series on the `[0, 1]` scale the plateau
/// detector ([`SchedulerConfig::plateau_eps`]) is calibrated for. Returns
/// `None` when no score can be recovered.
fn read_gen_score(gen_dir: &str) -> Option<f64> {
    const EVAL_RESULT_NAMES: &[&str] = &["evaluation_results.json", names::RESULTS_JSON];
    // Treat a value `> 1.0` as a 0–100 percent and rescale to `[0, 1]`.
    let as_fraction = |v: f64| if v > 1.0 { v / 100.0 } else { v };
    for name in EVAL_RESULT_NAMES {
        let path = Path::new(gen_dir).join(name);
        let Some(data) = read_json(&path) else {
            continue;
        };
        let Some(obj) = data.as_object() else {
            continue;
        };

        if let Some(pct) = obj.get("accuracy_percent").and_then(Value::as_f64) {
            return Some(as_fraction(pct));
        }
        if let Some(acc) = obj.get("accuracy").and_then(Value::as_f64) {
            return Some(as_fraction(acc));
        }
        let correct = obj.get("correct").and_then(Value::as_f64);
        let total = {
            let tq = obj.get("total_questions").and_then(Value::as_f64);
            match tq {
                Some(n) if n != 0.0 => Some(n),
                _ => obj.get("total").and_then(Value::as_f64),
            }
        };
        if let (Some(c), Some(t)) = (correct, total) {
            if t > 0.0 {
                return Some(c / t);
            }
        }
    }
    None
}

/// Total tokens (input + output) for a generation from its `telemetry.json`,
/// preferring the `cumulative` block and falling back to summing `generations`.
/// Returns `None` when absent or carrying no token fields.
fn read_gen_total_tokens(gen_dir: &str) -> Option<f64> {
    let path = Path::new(gen_dir).join(TELEMETRY_FILENAME);
    let data = read_json(&path)?;
    let token_total = |v: &Value| -> Option<f64> {
        let obj = v.as_object()?;
        let input = obj.get("input_tokens").and_then(Value::as_f64);
        let output = obj.get("output_tokens").and_then(Value::as_f64);
        match (input, output) {
            (None, None) => None,
            (i, o) => Some(i.unwrap_or(0.0) + o.unwrap_or(0.0)),
        }
    };

    if let Some(t) = data.get("cumulative").and_then(token_total) {
        return Some(t);
    }
    if let Some(gens) = data.get("generations").and_then(|v| v.as_array()) {
        let mut sum = 0.0;
        let mut seen = false;
        for entry in gens {
            if let Some(t) = token_total(entry) {
                sum += t;
                seen = true;
            }
        }
        if seen {
            return Some(sum);
        }
    }
    token_total(&data)
}

/// Compute cost for a generation: total tokens if available, else a constant
/// positive fallback so efficiencies stay well-defined.
fn gen_compute_cost(gen_dir: &str) -> f64 {
    match read_gen_total_tokens(gen_dir) {
        Some(t) if t > 0.0 => t,
        _ => FALLBACK_COMPUTE_COST,
    }
}

// --------------------------------------------------------------------------- //
// 1. Scheduler decision per generation
// --------------------------------------------------------------------------- //

/// Build [`GenerationRecord`]s for generations `0..=current_gen`, run the
/// [`AdaptiveScheduler`], and write `<gen_dir>/scheduler_decision.json`.
///
/// Each record's `score` is the accuracy from that generation's `results.json`,
/// its `compute_cost` is total tokens from `telemetry.json` (or
/// [`FALLBACK_COMPUTE_COST`]), and its `kind` is [`UpdateKind::Harness`] — the
/// base loop has only ever performed harness (prompt/scaffold) updates, so the
/// recorded history is all harness; the scheduler's job is to decide whether
/// the *next* step should switch to a weight update.
///
/// The artifact shape is:
///
/// ```json
/// {
///   "generation": 2,
///   "decision": "harness" | "weight",
///   "recommended_next": "harness" | "weight",
///   "rationale": "…",
///   "harness_efficiency": 0.0001 | null,
///   "weight_efficiency": null,
///   "harness_plateaued": true | false
/// }
/// ```
///
/// `decision` mirrors [`AdaptiveScheduler::decide_next`] (`harness` or
/// `weight`). A combined `"both"` is intentionally not emitted while the loop
/// only performs harness updates (so there is no weight-update history to weigh
/// against harness); [`maybe_run_weight_update`] still accepts `"both"`
/// defensively for a future mixed-history scheduler.
///
/// # Best-effort
///
/// Returns `None` (and writes nothing) on any IO/parse failure or when the
/// current generation has no readable score. Never panics.
pub fn record_scheduler_decision(
    layout: &RunLayout,
    current_gen: i64,
    config: &SchedulerConfig,
) -> Option<Value> {
    if current_gen < 0 {
        return None;
    }

    // The current generation must at least have a readable score, otherwise
    // there is nothing meaningful to decide on yet.
    let current_dir = layout.gen_dir(current_gen);
    let current_score = read_gen_score(&current_dir)?;

    let mut scheduler = AdaptiveScheduler::with_config(config.clone());
    for g in 0..=current_gen {
        let gen_dir = layout.gen_dir(g);
        let score = match read_gen_score(&gen_dir) {
            Some(s) => s,
            None => continue, // skip gens without a score; never panic
        };
        let compute_cost = gen_compute_cost(&gen_dir);
        scheduler.record(GenerationRecord {
            generation: g as u32 + 1,
            kind: UpdateKind::Harness,
            score,
            compute_cost,
        });
    }

    let summary = scheduler.efficiency_summary();
    let plateaued = summary
        .get("harness_plateaued")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // `decide_next()` is the source of truth (Harness|Weight). We do not synthesize
    // a "both" while the loop only records harness updates.
    let decision = match scheduler.decide_next() {
        UpdateKind::Weight => "weight",
        UpdateKind::Harness => "harness",
    };

    let harness_eff = summary
        .get("harness_efficiency")
        .cloned()
        .unwrap_or(Value::Null);
    let weight_eff = summary
        .get("weight_efficiency")
        .cloned()
        .unwrap_or(Value::Null);

    let rationale = build_rationale(
        decision,
        plateaued,
        &harness_eff,
        &weight_eff,
        current_score,
    );

    let artifact = json!({
        "generation": current_gen,
        "decision": decision,
        "recommended_next": summary.get("recommended_next").cloned().unwrap_or(Value::Null),
        "rationale": rationale,
        "harness_efficiency": harness_eff,
        "weight_efficiency": weight_eff,
        "harness_plateaued": plateaued,
    });

    // Best-effort write; on failure still return the computed Value so callers
    // (and the log line) can proceed.
    let out_path = Path::new(&current_dir).join(SCHEDULER_DECISION_JSON);
    if let Ok(text) = serde_json::to_string_pretty(&artifact) {
        let _ = std::fs::write(&out_path, text);
    }

    Some(artifact)
}

/// Compose a short human-readable rationale for the decision artifact.
fn build_rationale(
    decision: &str,
    plateaued: bool,
    harness_eff: &Value,
    weight_eff: &Value,
    current_score: f64,
) -> String {
    let eff_str = |v: &Value| -> String {
        v.as_f64()
            .map(|f| format!("{f:.6}"))
            .unwrap_or_else(|| "n/a".to_string())
    };
    match decision {
        "weight" => format!(
            "Harness improvement has plateaued (score {current_score:.3}); recommending a \
             weight update. harness_efficiency={}, weight_efficiency={}.",
            eff_str(harness_eff),
            eff_str(weight_eff),
        ),
        "both" => format!(
            "Harness series plateaued by the #19 detector yet the most recent harness step \
             still produced a strong gain (score {current_score:.3}); both levers are \
             defensible. harness_efficiency={}, weight_efficiency={}.",
            eff_str(harness_eff),
            eff_str(weight_eff),
        ),
        _ => {
            if plateaued {
                format!(
                    "Still in the early harness phase (score {current_score:.3}); sticking with \
                     harness updates before considering weights. harness_efficiency={}.",
                    eff_str(harness_eff),
                )
            } else {
                format!(
                    "Harness updates are still improving the score ({current_score:.3}); \
                     continuing with the cheap harness lever. harness_efficiency={}.",
                    eff_str(harness_eff),
                )
            }
        }
    }
}

// --------------------------------------------------------------------------- //
// 2. Observable weight-update step (#19)
// --------------------------------------------------------------------------- //

/// When `decision` is `"weight"` or `"both"`, run the CPU reference LoRA on
/// this generation's trajectory and write `<gen_dir>/weight_update.json`.
///
/// The trajectory is read from `<gen_dir>/agent_execution.json` (the single
/// trajectory shape). For a multi-trajectory generation (an `agent_execution/`
/// directory of `execution_q*.json`) the first available trajectory is used; if
/// neither is present the step is skipped. Training examples are extracted with
/// [`extract_training_examples`] using this generation's score as the reward,
/// then [`LoraReferenceUpdater::update`] runs and its [`WeightUpdateOutcome`] is
/// persisted alongside `{ "generation", "kind": "weight" }`.
///
/// # Best-effort
///
/// Returns `None` (writing nothing) when the decision is not weight/both, when
/// no trajectory or score is available, or on any IO failure. Never panics. An
/// empty/odd trajectory yields zero examples and a no-op update outcome (still
/// written, so the UI can show "no examples").
pub fn maybe_run_weight_update(
    layout: &RunLayout,
    current_gen: i64,
    decision: &str,
    config: &WeightUpdateConfig,
) -> Option<WeightUpdateOutcome> {
    if decision != "weight" && decision != "both" {
        return None;
    }
    if current_gen < 0 {
        return None;
    }

    let gen_dir = layout.gen_dir(current_gen);
    let reward = read_gen_score(&gen_dir).unwrap_or(0.0);
    let trajectory = load_trajectory(layout, current_gen)?;

    let examples = extract_training_examples(&trajectory, reward);
    // Default-build backend (issue #139): the dependency-free stub. A
    // `--features weight-updates` build can swap this for `CandleLoRAWeightUpdater`
    // with no other change here — both implement `WeightUpdater`.
    let mut updater = StubWeightUpdater::new(config.clone());
    let outcome = updater.update(&examples);

    let artifact = json!({
        "generation": current_gen,
        "kind": "weight",
        "updater": updater.name(),
        "num_examples": outcome.num_examples,
        "loss_before": outcome.loss_before,
        "loss_after": outcome.loss_after,
        "updated": outcome.updated,
        "details": outcome.details,
    });

    let out_path = Path::new(&gen_dir).join(WEIGHT_UPDATE_JSON);
    if let Ok(text) = serde_json::to_string_pretty(&artifact) {
        let _ = std::fs::write(&out_path, text);
    }

    Some(outcome)
}

// --------------------------------------------------------------------------- //
// 3. Acting on the decision (issue #90)
// --------------------------------------------------------------------------- //

/// What the orchestrator actually *did* this generation in response to the
/// scheduler decision — issue #90 closes the loop so the recommendation drives
/// real control flow instead of only being recorded.
///
/// `harness` runs the meta/feedback harness update (today's behavior); `weight`
/// runs the weight update and **skips** the harness/feedback step; `both` runs
/// both. The variant is derived from the scheduler decision by
/// [`action_for_decision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActedDecision {
    /// Run the harness (meta/feedback) update only.
    Harness,
    /// Run the weight update only; skip the harness/feedback step.
    Weight,
    /// Run both the weight update and the harness update.
    Both,
}

impl ActedDecision {
    /// Stable lower-case label for the artifact / logs.
    pub fn as_str(self) -> &'static str {
        match self {
            ActedDecision::Harness => "harness",
            ActedDecision::Weight => "weight",
            ActedDecision::Both => "both",
        }
    }

    /// Whether the harness (meta/feedback) update should run for this decision.
    pub fn runs_harness(self) -> bool {
        matches!(self, ActedDecision::Harness | ActedDecision::Both)
    }

    /// Whether the weight update should run for this decision.
    pub fn runs_weight(self) -> bool {
        matches!(self, ActedDecision::Weight | ActedDecision::Both)
    }
}

/// Map a scheduler `decision` string to the action the loop takes.
///
/// `"weight"` -> [`ActedDecision::Weight`], `"both"` -> [`ActedDecision::Both`],
/// and **anything else** (including `"harness"`, an unknown spelling, or an empty
/// string) -> [`ActedDecision::Harness`]. Defaulting unknown decisions to harness
/// keeps the safe, cheap lever as the fallback and preserves today's behavior
/// when no usable decision is present.
pub fn action_for_decision(decision: &str) -> ActedDecision {
    match decision {
        "weight" => ActedDecision::Weight,
        "both" => ActedDecision::Both,
        _ => ActedDecision::Harness,
    }
}

/// Annotate this generation's `scheduler_decision.json` with what the loop
/// **acted** on (issue #90), so SIA Studio and `improvement.md` can show the
/// action taken — not just the recommendation.
///
/// Adds keys to the existing artifact (leaving the parity-checked recommendation
/// fields untouched):
///
/// * `acted` — `"harness" | "weight" | "both"`, the lever actually pulled.
/// * `harness_ran` / `weight_ran` — booleans for the two levers.
/// * `weight_update` — a compact `{ updated, num_examples, loss_before,
///   loss_after }` summary when a weight update ran, else `null`.
///
/// Best-effort: if the artifact is missing/unparseable it writes a fresh object
/// carrying just the acted fields; any IO error is swallowed. Never panics.
pub fn record_acted_decision(
    layout: &RunLayout,
    current_gen: i64,
    acted: ActedDecision,
    weight_outcome: Option<&WeightUpdateOutcome>,
) {
    if current_gen < 0 {
        return;
    }
    let gen_dir = layout.gen_dir(current_gen);
    let out_path = Path::new(&gen_dir).join(SCHEDULER_DECISION_JSON);

    let mut artifact = match read_json(&out_path) {
        Some(v @ Value::Object(_)) => v,
        _ => json!({ "generation": current_gen }),
    };

    let weight_summary = match weight_outcome {
        Some(o) => json!({
            "updated": o.updated,
            "num_examples": o.num_examples,
            "loss_before": o.loss_before,
            "loss_after": o.loss_after,
        }),
        None => Value::Null,
    };

    if let Some(obj) = artifact.as_object_mut() {
        obj.insert("acted".to_string(), Value::from(acted.as_str()));
        obj.insert("harness_ran".to_string(), Value::from(acted.runs_harness()));
        obj.insert("weight_ran".to_string(), Value::from(acted.runs_weight()));
        obj.insert("weight_update".to_string(), weight_summary);
    }

    if let Ok(text) = serde_json::to_string_pretty(&artifact) {
        let _ = std::fs::write(&out_path, text);
    }
}

/// Load a single trajectory for the generation: prefer the single
/// `agent_execution.json`, else the first `execution_q*.json` in the
/// `agent_execution/` directory. Returns `None` if neither parses.
fn load_trajectory(layout: &RunLayout, gen: i64) -> Option<Value> {
    let gen_dir = layout.gen_dir(gen);
    let single = Path::new(&gen_dir).join(names::AGENT_EXECUTION_JSON);
    if let Some(v) = read_json(&single) {
        return Some(v);
    }

    let exec_dir = layout.agent_execution_dir(gen);
    let dir = Path::new(&exec_dir);
    if !dir.is_dir() {
        return None;
    }
    let mut candidates: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(names::EXECUTION_GLOB_PREFIX) && n.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    for path in candidates {
        if let Some(v) = read_json(&path) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a temp run with `gen_<i>` directories, each carrying a
    /// `results.json` with the given accuracy. Returns the tempdir (kept alive)
    /// and a [`RunLayout`] rooted at it.
    fn make_run(scores: &[f64]) -> (tempfile::TempDir, RunLayout) {
        let d = tempfile::tempdir().unwrap();
        let run_dir = d.path().join("run_1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let layout = RunLayout::new(run_dir.to_string_lossy().into_owned());
        for (i, &acc) in scores.iter().enumerate() {
            let gen_dir = layout.gen_dir(i as i64);
            std::fs::create_dir_all(&gen_dir).unwrap();
            std::fs::write(
                Path::new(&gen_dir).join(names::RESULTS_JSON),
                json!({"accuracy": acc, "accuracy_percent": acc * 100.0,
                       "correct": (acc * 10.0) as i64, "total": 10})
                .to_string(),
            )
            .unwrap();
        }
        (d, layout)
    }

    fn write_telemetry(layout: &RunLayout, gen: i64, total_tokens: u64) {
        let gen_dir = layout.gen_dir(gen);
        std::fs::write(
            Path::new(&gen_dir).join(TELEMETRY_FILENAME),
            json!({"cumulative": {"input_tokens": total_tokens, "output_tokens": 0}}).to_string(),
        )
        .unwrap();
    }

    fn write_single_trajectory(layout: &RunLayout, gen: i64) {
        let gen_dir = layout.gen_dir(gen);
        std::fs::write(
            Path::new(&gen_dir).join(names::AGENT_EXECUTION_JSON),
            json!([
                {"role": "user", "content": "what is 2+2?"},
                {"role": "assistant", "content": [{"type": "text", "text": "The answer is 4."}]},
                {"role": "user", "content": "and 3+3?"},
                {"role": "assistant", "content": [{"type": "text", "text": "Six."}]}
            ])
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn read_gen_score_normalizes_percent_and_prefers_evaluation_results() {
        let d = tempfile::tempdir().unwrap();
        let gen = d.path().join("gen");
        std::fs::create_dir_all(&gen).unwrap();
        let gen_s = gen.to_string_lossy().into_owned();

        // `accuracy` written on a 0–100 scale (e.g. longcot-chess) normalizes to [0,1].
        std::fs::write(
            gen.join(names::RESULTS_JSON),
            json!({"accuracy": 75.0}).to_string(),
        )
        .unwrap();
        assert_eq!(read_gen_score(&gen_s), Some(0.75));

        // A fractional `accuracy` is returned unchanged.
        std::fs::write(
            gen.join(names::RESULTS_JSON),
            json!({"accuracy": 0.4}).to_string(),
        )
        .unwrap();
        assert_eq!(read_gen_score(&gen_s), Some(0.4));

        // `evaluation_results.json` takes precedence over `results.json`,
        // and `accuracy_percent` takes precedence within a file.
        std::fs::write(
            gen.join("evaluation_results.json"),
            json!({"accuracy_percent": 90.0, "accuracy": 0.1}).to_string(),
        )
        .unwrap();
        assert_eq!(read_gen_score(&gen_s), Some(0.9));
    }

    #[test]
    fn decision_artifact_has_expected_shape() {
        let (_d, layout) = make_run(&[0.2, 0.5]);
        write_telemetry(&layout, 0, 100);
        write_telemetry(&layout, 1, 200);

        let v = record_scheduler_decision(&layout, 1, &SchedulerConfig::default())
            .expect("decision written");
        for key in [
            "generation",
            "decision",
            "recommended_next",
            "rationale",
            "harness_efficiency",
            "weight_efficiency",
            "harness_plateaued",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}: {v}");
        }
        assert_eq!(v["generation"], json!(1));
        // Artifact file was written.
        let path = Path::new(&layout.gen_dir(1)).join(SCHEDULER_DECISION_JSON);
        assert!(path.is_file());
        let on_disk = read_json(&path).unwrap();
        assert_eq!(on_disk["generation"], json!(1));
    }

    #[test]
    fn improving_history_decides_harness() {
        // Steadily improving scores (deltas well above eps) -> harness.
        let (_d, layout) = make_run(&[0.10, 0.30, 0.55]);
        let v =
            record_scheduler_decision(&layout, 2, &SchedulerConfig::default()).expect("decision");
        assert_eq!(v["decision"], json!("harness"));
        assert_eq!(v["harness_plateaued"], json!(false));
    }

    #[test]
    fn plateaued_history_decides_weight() {
        // Early jump then flat: plateaued -> weight (reusing #65/#19 logic).
        let (_d, layout) = make_run(&[0.10, 0.60, 0.605, 0.606, 0.6065]);
        let v =
            record_scheduler_decision(&layout, 4, &SchedulerConfig::default()).expect("decision");
        // decide_next flips to Weight on plateau; the last step is tiny, so the
        // decision is plain "weight" (not "both").
        assert_eq!(v["decision"], json!("weight"));
        assert_eq!(v["harness_plateaued"], json!(true));
    }

    #[test]
    fn weight_update_runs_only_on_weight_decision() {
        let (_d, layout) = make_run(&[0.5]);
        write_single_trajectory(&layout, 0);

        // Harness decision -> no-op, nothing written.
        assert!(
            maybe_run_weight_update(&layout, 0, "harness", &WeightUpdateConfig::default())
                .is_none()
        );
        assert!(!Path::new(&layout.gen_dir(0))
            .join(WEIGHT_UPDATE_JSON)
            .exists());

        // Weight decision -> outcome with loss_after <= loss_before, artifact written.
        let outcome = maybe_run_weight_update(&layout, 0, "weight", &WeightUpdateConfig::default())
            .expect("weight update ran");
        assert!(outcome.updated);
        assert!(outcome.num_examples >= 1);
        assert!(
            outcome.loss_after <= outcome.loss_before,
            "loss must not increase: {} -> {}",
            outcome.loss_before,
            outcome.loss_after
        );
        let path = Path::new(&layout.gen_dir(0)).join(WEIGHT_UPDATE_JSON);
        assert!(path.is_file());
        let on_disk = read_json(&path).unwrap();
        assert_eq!(on_disk["kind"], json!("weight"));
        assert_eq!(on_disk["num_examples"], json!(outcome.num_examples));
    }

    #[test]
    fn weight_update_both_decision_also_runs() {
        let (_d, layout) = make_run(&[0.5]);
        write_single_trajectory(&layout, 0);
        let outcome = maybe_run_weight_update(&layout, 0, "both", &WeightUpdateConfig::default());
        assert!(outcome.is_some());
        assert!(Path::new(&layout.gen_dir(0))
            .join(WEIGHT_UPDATE_JSON)
            .is_file());
    }

    #[test]
    fn missing_files_are_no_panic_none() {
        let d = tempfile::tempdir().unwrap();
        let layout = RunLayout::new(d.path().join("run_9").to_string_lossy().into_owned());
        // No gen dirs at all.
        assert!(record_scheduler_decision(&layout, 0, &SchedulerConfig::default()).is_none());
        assert!(
            maybe_run_weight_update(&layout, 0, "weight", &WeightUpdateConfig::default()).is_none()
        );
        // Negative gen index.
        assert!(record_scheduler_decision(&layout, -1, &SchedulerConfig::default()).is_none());
    }

    #[test]
    fn weight_decision_without_trajectory_is_none() {
        // results.json present (so score reads), but no trajectory -> skip.
        let (_d, layout) = make_run(&[0.4]);
        assert!(
            maybe_run_weight_update(&layout, 0, "weight", &WeightUpdateConfig::default()).is_none()
        );
    }

    #[test]
    fn multi_trajectory_dir_is_used() {
        let (_d, layout) = make_run(&[0.6]);
        let exec_dir = layout.agent_execution_dir(0);
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::write(
            Path::new(&exec_dir).join("execution_q1.json"),
            json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [{"type": "text", "text": "hello there"}]}
            ])
            .to_string(),
        )
        .unwrap();
        let outcome = maybe_run_weight_update(&layout, 0, "weight", &WeightUpdateConfig::default())
            .expect("ran on multi-trajectory");
        assert!(outcome.num_examples >= 1);
    }

    #[test]
    fn compute_cost_falls_back_without_telemetry() {
        // No telemetry -> fallback cost; still produces a valid decision.
        let (_d, layout) = make_run(&[0.2, 0.4, 0.6]);
        let v =
            record_scheduler_decision(&layout, 2, &SchedulerConfig::default()).expect("decision");
        assert_eq!(v["decision"], json!("harness"));
    }

    // -- Acting on the decision (issue #90) ------------------------------------

    #[test]
    fn action_for_decision_maps_each_lever_and_defaults_to_harness() {
        assert_eq!(action_for_decision("weight"), ActedDecision::Weight);
        assert_eq!(action_for_decision("both"), ActedDecision::Both);
        assert_eq!(action_for_decision("harness"), ActedDecision::Harness);
        // Unknown / empty spellings fall back to the cheap, safe harness lever.
        assert_eq!(action_for_decision("nonsense"), ActedDecision::Harness);
        assert_eq!(action_for_decision(""), ActedDecision::Harness);
    }

    #[test]
    fn acted_decision_lever_flags() {
        assert!(ActedDecision::Harness.runs_harness());
        assert!(!ActedDecision::Harness.runs_weight());
        assert!(!ActedDecision::Weight.runs_harness());
        assert!(ActedDecision::Weight.runs_weight());
        assert!(ActedDecision::Both.runs_harness());
        assert!(ActedDecision::Both.runs_weight());
    }

    #[test]
    fn record_acted_decision_annotates_existing_artifact() {
        // Seed an existing recommendation artifact, then record what we acted on.
        let (_d, layout) = make_run(&[0.5]);
        let v =
            record_scheduler_decision(&layout, 0, &SchedulerConfig::default()).expect("decision");
        // Pretend we ran a `both` action with a weight outcome.
        let outcome = WeightUpdateOutcome {
            num_examples: 2,
            loss_before: 0.5,
            loss_after: 0.25,
            updated: true,
            details: "test".to_string(),
        };
        record_acted_decision(&layout, 0, ActedDecision::Both, Some(&outcome));

        let path = Path::new(&layout.gen_dir(0)).join(SCHEDULER_DECISION_JSON);
        let on_disk = read_json(&path).unwrap();
        // Original recommendation fields are preserved.
        assert_eq!(on_disk["decision"], v["decision"]);
        // Acted fields are added.
        assert_eq!(on_disk["acted"], json!("both"));
        assert_eq!(on_disk["harness_ran"], json!(true));
        assert_eq!(on_disk["weight_ran"], json!(true));
        assert_eq!(on_disk["weight_update"]["updated"], json!(true));
        assert_eq!(on_disk["weight_update"]["num_examples"], json!(2));
        assert_eq!(on_disk["weight_update"]["loss_after"], json!(0.25));
    }

    #[test]
    fn record_acted_decision_without_artifact_writes_fresh_object() {
        // No prior scheduler_decision.json (e.g. harness with no weight outcome).
        let (_d, layout) = make_run(&[0.4]);
        record_acted_decision(&layout, 0, ActedDecision::Harness, None);
        let path = Path::new(&layout.gen_dir(0)).join(SCHEDULER_DECISION_JSON);
        let on_disk = read_json(&path).unwrap();
        assert_eq!(on_disk["acted"], json!("harness"));
        assert_eq!(on_disk["harness_ran"], json!(true));
        assert_eq!(on_disk["weight_ran"], json!(false));
        assert!(on_disk["weight_update"].is_null());
    }

    #[test]
    fn record_acted_decision_negative_gen_is_noop() {
        let d = tempfile::tempdir().unwrap();
        let layout = RunLayout::new(d.path().join("run_x").to_string_lossy().into_owned());
        // Must not panic and must not create anything.
        record_acted_decision(&layout, -1, ActedDecision::Weight, None);
    }
}
