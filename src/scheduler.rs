//! Adaptive harness-vs-weight update scheduler (heuristic) — issue #65.
//!
//! # What this extends
//!
//! The SIA paper's self-improvement loop has two levers for making the agent
//! better between generations:
//!
//! 1. **Harness updates** — edit the agent's *prompt / scaffold* (the only lever
//!    the base loop uses), and
//! 2. **Weight updates** — actually train model weights from high-reward
//!    trajectories (the path opened by issue #19: [`crate::weights`]).
//!
//! The paper lists *"meta-RL over the harness-vs-weight decision policy"* as
//! future work: learning *which* lever to pull next. This module is the
//! **hackathon-sized, honest first step** toward that — a clean, fully-offline
//! **heuristic** scheduler that tracks how much score improvement each lever buys
//! per unit of compute and recommends the next [`UpdateKind`]. It is explicitly
//! **not** meta-RL: there is no learned policy, no environment, no gradient over
//! the decision — just a transparent decision tree plus an
//! "improvement-per-compute" metric. Meta-RL remains future work; this gives the
//! Feedback-Agent a principled, inspectable signal to act on now and a stable
//! seam to swap a learned policy behind later.
//!
//! # The "improvement efficiency" metric
//!
//! For each lever we define **improvement efficiency** as *mean positive score
//! improvement per unit of compute*: looking at consecutive generations of the
//! same [`UpdateKind`], we take each positive score delta `score[i] - score[i-1]`
//! and divide it by the compute the later generation cost
//! ([`GenerationRecord::compute_cost`]), then average. This answers "how much
//! reward did a dollar of compute spent on *this* lever recently buy?", which is
//! exactly the quantity a harness-vs-weight policy wants to compare. See
//! [`AdaptiveScheduler::improvement_efficiency`].
//!
//! # Integration seam (NOT yet wired into the orchestrator)
//!
//! Like #19's [`crate::weights::should_trigger_weight_update`] and #66's verifier
//! work, this ships as a **standalone, tested module that is not yet wired into
//! the orchestrator** — only the seam is documented here. The intended hookup:
//!
//! * After each generation, the Feedback-Agent / orchestrator
//!   ([`crate::orchestrator::run_generation_with`]) constructs a
//!   [`GenerationRecord`] from that generation's score (see
//!   [`crate::results`]) and its compute cost (e.g. total tokens or `duration_ms`
//!   from [`crate::llm::GenerationTelemetry`]) and calls
//!   [`AdaptiveScheduler::record`].
//! * Before launching the next generation it calls
//!   [`AdaptiveScheduler::decide_next`] to choose whether the upcoming generation
//!   should be a harness edit or a weight update, and threads that choice into the
//!   improvement prompt / `improvement.md` it writes for the next gen directory
//!   ([`crate::orchestrator::next_gen_dir`]).
//! * [`AdaptiveScheduler::efficiency_summary`] produces a small JSON blob the SIA
//!   Studio dashboard (#63) can render so a human can see, per lever, how
//!   efficiently compute is being converted into score and what the scheduler
//!   recommends next.
//!
//! No orchestrator behavior is changed by this module; the wiring above is left
//! to a follow-up so the policy can be reviewed and tested in isolation first.
//!
//! # Heuristic, by design
//!
//! The decision tree in [`AdaptiveScheduler::decide_next`] reuses #19's plateau
//! detector and a simple efficiency comparison. It is intentionally simple and
//! deterministic so its behavior is obvious in the dashboard and in tests. A
//! learned (meta-RL) policy would implement the same `decide_next` shape.

use serde_json::{json, Value};

use crate::weights::should_trigger_weight_update;

/// Which lever a generation pulled (or should pull next): a *harness* (prompt /
/// scaffold) edit, or a model *weight* update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateKind {
    /// Edit the agent's prompt / scaffold (the base self-improvement loop).
    Harness,
    /// Train model weights from high-reward trajectories (issue #19 path).
    Weight,
}

impl UpdateKind {
    /// Lower-case stable label for JSON / logging (`"harness"` / `"weight"`).
    fn as_str(self) -> &'static str {
        match self {
            UpdateKind::Harness => "harness",
            UpdateKind::Weight => "weight",
        }
    }
}

/// One generation's outcome: which lever it pulled, the score it achieved, and
/// what it cost.
///
/// `compute_cost` is the amount of compute this generation consumed in a
/// **caller-chosen but consistent** unit — e.g. total tokens
/// (`GenerationTelemetry::total_tokens`) or wall-clock `duration_ms`. The
/// scheduler never interprets the unit; it only divides score improvement by it,
/// so as long as the caller uses the *same* unit for every record the resulting
/// efficiencies are comparable across levers.
#[derive(Debug, Clone)]
pub struct GenerationRecord {
    /// 1-based generation index this record describes.
    pub generation: u32,
    /// Which lever this generation pulled.
    pub kind: UpdateKind,
    /// The generation's score (typically `evaluate.py` accuracy in `[0, 1]`).
    pub score: f64,
    /// Compute this generation consumed (tokens or `duration_ms`; caller-chosen
    /// but consistent across all records).
    pub compute_cost: f64,
}

/// Tuning knobs for [`AdaptiveScheduler`].
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Plateau threshold handed to [`should_trigger_weight_update`]: harness
    /// improvement below this per generation counts as "plateaued".
    pub plateau_eps: f64,
    /// Minimum number of harness generations to run before the scheduler is even
    /// willing to recommend a weight update. Keeps the cheap prompt lever as the
    /// early default while there is little history to judge.
    pub min_harness_gens: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            plateau_eps: 0.01,
            min_harness_gens: 2,
        }
    }
}

/// A heuristic scheduler that records per-generation outcomes and recommends the
/// next [`UpdateKind`] based on improvement-per-compute and a plateau signal.
///
/// See the module docs for the policy, the metric, and the (not-yet-wired)
/// orchestrator integration seam.
pub struct AdaptiveScheduler {
    history: Vec<GenerationRecord>,
    config: SchedulerConfig,
}

impl AdaptiveScheduler {
    /// Create a scheduler with the default [`SchedulerConfig`].
    pub fn new() -> Self {
        Self::with_config(SchedulerConfig::default())
    }

    /// Create a scheduler with an explicit config.
    pub fn with_config(config: SchedulerConfig) -> Self {
        Self {
            history: Vec::new(),
            config,
        }
    }

    /// Append one generation's outcome to the history.
    pub fn record(&mut self, record: GenerationRecord) {
        self.history.push(record);
    }

    /// Borrow the recorded generation history in chronological order.
    pub fn history(&self) -> &[GenerationRecord] {
        &self.history
    }

    /// Mean **improvement per unit compute** for a given lever, or `None` when
    /// there is not enough data to judge it.
    ///
    /// # Definition
    ///
    /// Restrict the history to records of `kind`, in chronological order. For each
    /// consecutive pair `(prev, cur)` of same-kind records, the *improvement* is
    /// the positive score delta `max(cur.score - prev.score, 0.0)` and the *cost*
    /// is `cur.compute_cost`. The efficiency of that step is `improvement / cost`.
    /// The returned value is the **mean** of these per-step efficiencies — "how
    /// much score, on average, one unit of compute spent on this lever recently
    /// bought".
    ///
    /// Negative deltas (regressions) contribute `0.0` rather than a negative
    /// number, so a lever is never credited for making things worse but is fairly
    /// penalized (a zero pulls its mean down).
    ///
    /// # Edge cases
    ///
    /// * Returns `None` if fewer than two records of `kind` exist (no delta to
    ///   measure).
    /// * **Divide-by-zero safe:** a step whose `cur.compute_cost <= 0.0` is
    ///   skipped (it cannot define an "improvement per compute"). If *every* step
    ///   is skipped this way, returns `None` rather than dividing by zero or
    ///   averaging an empty set.
    pub fn improvement_efficiency(&self, kind: UpdateKind) -> Option<f64> {
        let scores: Vec<&GenerationRecord> =
            self.history.iter().filter(|r| r.kind == kind).collect();
        if scores.len() < 2 {
            return None;
        }

        let mut sum = 0.0;
        let mut count = 0usize;
        for pair in scores.windows(2) {
            let (prev, cur) = (pair[0], pair[1]);
            if cur.compute_cost <= 0.0 {
                // Cannot define improvement-per-compute with non-positive cost.
                continue;
            }
            let improvement = (cur.score - prev.score).max(0.0);
            sum += improvement / cur.compute_cost;
            count += 1;
        }

        if count == 0 {
            None
        } else {
            Some(sum / count as f64)
        }
    }

    /// The chronological score series for one lever (used for plateau detection).
    fn scores_for(&self, kind: UpdateKind) -> Vec<f64> {
        self.history
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.score)
            .collect()
    }

    /// Number of recorded harness generations.
    fn harness_gen_count(&self) -> usize {
        self.history
            .iter()
            .filter(|r| r.kind == UpdateKind::Harness)
            .count()
    }

    /// Whether the harness score series has plateaued, reusing #19's
    /// [`should_trigger_weight_update`] with `config.plateau_eps`.
    fn harness_plateaued(&self) -> bool {
        should_trigger_weight_update(
            &self.scores_for(UpdateKind::Harness),
            self.config.plateau_eps,
        )
    }

    /// Recommend the lever the next generation should pull.
    ///
    /// # Decision tree
    ///
    /// 1. **Prefer harness early.** If fewer than `config.min_harness_gens`
    ///    harness generations have been recorded, return [`UpdateKind::Harness`].
    ///    This also covers an empty / very short history, so callers never get a
    ///    weight recommendation before there is evidence the cheap prompt lever
    ///    has been tried. (No panic on empty history.)
    /// 2. **Switch on plateau.** Otherwise, if the harness score series has
    ///    plateaued — i.e. #19's [`should_trigger_weight_update`] fires on the
    ///    harness scores with `config.plateau_eps` — return [`UpdateKind::Weight`]:
    ///    prompt iteration has stopped paying off, so try training weights.
    /// 3. **Otherwise follow efficiency.** If harness is still improving, stick
    ///    with whichever lever has the higher recent
    ///    [`improvement_efficiency`](Self::improvement_efficiency). A lever with
    ///    no measurable efficiency (`None`) loses the comparison. On a tie, or
    ///    when neither lever has a measurable efficiency, fall back to
    ///    [`UpdateKind::Harness`] (the safe, cheap default).
    pub fn decide_next(&self) -> UpdateKind {
        // (1) Prefer the cheap harness lever until we've tried it enough.
        if self.harness_gen_count() < self.config.min_harness_gens {
            return UpdateKind::Harness;
        }

        // (2) Harness improvement has plateaued -> reach for weight updates.
        if self.harness_plateaued() {
            return UpdateKind::Weight;
        }

        // (3) Still improving: follow the more compute-efficient lever.
        let harness_eff = self.improvement_efficiency(UpdateKind::Harness);
        let weight_eff = self.improvement_efficiency(UpdateKind::Weight);
        match (harness_eff, weight_eff) {
            (Some(h), Some(w)) if w > h => UpdateKind::Weight,
            // Tie, harness-better, or only one/zero measurable -> harness default.
            _ => UpdateKind::Harness,
        }
    }

    /// A small JSON summary for the SIA Studio dashboard (#63) and the
    /// Feedback-Agent.
    ///
    /// Shape:
    ///
    /// ```json
    /// {
    ///   "harness_efficiency": 0.0001 | null,
    ///   "weight_efficiency": 0.0002 | null,
    ///   "last_kind": "harness" | "weight" | null,
    ///   "harness_plateaued": true | false,
    ///   "recommended_next": "harness" | "weight"
    /// }
    /// ```
    ///
    /// `*_efficiency` are the [`improvement_efficiency`](Self::improvement_efficiency)
    /// values (`null` when unmeasurable); `last_kind` is the most recently
    /// recorded lever (`null` for empty history); `harness_plateaued` is the #19
    /// plateau signal on the harness series; `recommended_next` is
    /// [`decide_next`](Self::decide_next).
    pub fn efficiency_summary(&self) -> Value {
        let harness_efficiency = self.improvement_efficiency(UpdateKind::Harness);
        let weight_efficiency = self.improvement_efficiency(UpdateKind::Weight);
        let last_kind = self.history.last().map(|r| r.kind.as_str());
        json!({
            "harness_efficiency": harness_efficiency,
            "weight_efficiency": weight_efficiency,
            "last_kind": last_kind,
            "harness_plateaued": self.harness_plateaued(),
            "recommended_next": self.decide_next().as_str(),
        })
    }
}

impl Default for AdaptiveScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(generation: u32, kind: UpdateKind, score: f64, compute_cost: f64) -> GenerationRecord {
        GenerationRecord {
            generation,
            kind,
            score,
            compute_cost,
        }
    }

    // -- improvement_efficiency ------------------------------------------------

    #[test]
    fn efficiency_is_mean_of_positive_deltas_over_cost() {
        let mut s = AdaptiveScheduler::new();
        // Harness: 0.2 -> 0.5 (delta 0.3, cost 100) -> 0.6 (delta 0.1, cost 50).
        s.record(rec(1, UpdateKind::Harness, 0.2, 10.0));
        s.record(rec(2, UpdateKind::Harness, 0.5, 100.0));
        s.record(rec(3, UpdateKind::Harness, 0.6, 50.0));
        // Steps: 0.3/100 = 0.003 ; 0.1/50 = 0.002 ; mean = 0.0025.
        let eff = s.improvement_efficiency(UpdateKind::Harness).unwrap();
        assert!((eff - 0.0025).abs() < 1e-12, "got {eff}");
    }

    #[test]
    fn efficiency_floors_regressions_at_zero() {
        let mut s = AdaptiveScheduler::new();
        // Second step regresses (0.5 -> 0.4); that step contributes 0.0.
        s.record(rec(1, UpdateKind::Harness, 0.2, 10.0));
        s.record(rec(2, UpdateKind::Harness, 0.5, 100.0)); // +0.3/100 = 0.003
        s.record(rec(3, UpdateKind::Harness, 0.4, 50.0)); // regression -> 0.0
        let eff = s.improvement_efficiency(UpdateKind::Harness).unwrap();
        // mean(0.003, 0.0) = 0.0015
        assert!((eff - 0.0015).abs() < 1e-12, "got {eff}");
    }

    #[test]
    fn efficiency_none_on_insufficient_data() {
        let mut s = AdaptiveScheduler::new();
        assert_eq!(s.improvement_efficiency(UpdateKind::Harness), None);
        s.record(rec(1, UpdateKind::Harness, 0.4, 10.0));
        // Only one harness record -> still no delta -> None.
        assert_eq!(s.improvement_efficiency(UpdateKind::Harness), None);
        // A different-kind record doesn't help harness.
        s.record(rec(2, UpdateKind::Weight, 0.9, 10.0));
        assert_eq!(s.improvement_efficiency(UpdateKind::Harness), None);
    }

    #[test]
    fn efficiency_is_divide_by_zero_safe() {
        let mut s = AdaptiveScheduler::new();
        // The only measurable step has zero compute_cost on the later record.
        s.record(rec(1, UpdateKind::Harness, 0.2, 10.0));
        s.record(rec(2, UpdateKind::Harness, 0.5, 0.0));
        // That step is skipped; no measurable step remains -> None (no inf/NaN).
        assert_eq!(s.improvement_efficiency(UpdateKind::Harness), None);

        // Now add a real step; only the positive-cost step should count.
        s.record(rec(3, UpdateKind::Harness, 0.7, 50.0)); // +0.2/50 = 0.004
        let eff = s.improvement_efficiency(UpdateKind::Harness).unwrap();
        assert!(eff.is_finite());
        assert!((eff - 0.004).abs() < 1e-12, "got {eff}");
    }

    // -- decide_next -----------------------------------------------------------

    #[test]
    fn decide_next_empty_history_is_harness_no_panic() {
        let s = AdaptiveScheduler::new();
        assert_eq!(s.decide_next(), UpdateKind::Harness);
    }

    #[test]
    fn decide_next_prefers_harness_below_min_gens() {
        let mut s = AdaptiveScheduler::with_config(SchedulerConfig {
            plateau_eps: 0.01,
            min_harness_gens: 2,
        });
        // Only one harness gen so far (< min_harness_gens) even though it has
        // plateaued by value, we still recommend harness because we haven't tried
        // the cheap lever enough yet.
        s.record(rec(1, UpdateKind::Harness, 0.5, 10.0));
        assert_eq!(s.decide_next(), UpdateKind::Harness);
    }

    #[test]
    fn decide_next_harness_while_still_improving() {
        let mut s = AdaptiveScheduler::new(); // min_harness_gens = 2
                                              // Steadily improving harness scores (deltas well above eps) -> not
                                              // plateaued -> stay on harness (it's the only lever with efficiency).
        s.record(rec(1, UpdateKind::Harness, 0.10, 10.0));
        s.record(rec(2, UpdateKind::Harness, 0.30, 10.0));
        s.record(rec(3, UpdateKind::Harness, 0.55, 10.0));
        assert!(!s.harness_plateaued());
        assert_eq!(s.decide_next(), UpdateKind::Harness);
    }

    #[test]
    fn decide_next_switches_to_weight_on_plateau() {
        let mut s = AdaptiveScheduler::new(); // min_harness_gens = 2, eps 0.01
                                              // Early jump then flat: last deltas all below eps -> plateaued harness.
        s.record(rec(1, UpdateKind::Harness, 0.10, 10.0));
        s.record(rec(2, UpdateKind::Harness, 0.60, 10.0));
        s.record(rec(3, UpdateKind::Harness, 0.605, 10.0));
        s.record(rec(4, UpdateKind::Harness, 0.606, 10.0));
        s.record(rec(5, UpdateKind::Harness, 0.6065, 10.0));
        assert!(s.harness_plateaued());
        assert_eq!(s.decide_next(), UpdateKind::Weight);
    }

    #[test]
    fn decide_next_prefers_higher_efficiency_kind_when_no_plateau() {
        // Harness still improving (no plateau), but weight updates have been far
        // more efficient -> recommend weight.
        let mut s = AdaptiveScheduler::with_config(SchedulerConfig {
            plateau_eps: 0.01,
            min_harness_gens: 2,
        });
        // Harness: improving but expensive (low efficiency).
        s.record(rec(1, UpdateKind::Harness, 0.10, 1000.0));
        s.record(rec(2, UpdateKind::Harness, 0.30, 1000.0)); // +0.2/1000 = 2e-4
        s.record(rec(3, UpdateKind::Harness, 0.55, 1000.0)); // +0.25/1000 = 2.5e-4
                                                             // Weight: cheap and effective (high efficiency).
        s.record(rec(4, UpdateKind::Weight, 0.40, 10.0));
        s.record(rec(5, UpdateKind::Weight, 0.80, 10.0)); // +0.4/10 = 0.04
        assert!(!s.harness_plateaued(), "harness should still be improving");
        let h = s.improvement_efficiency(UpdateKind::Harness).unwrap();
        let w = s.improvement_efficiency(UpdateKind::Weight).unwrap();
        assert!(w > h, "weight {w} should beat harness {h}");
        assert_eq!(s.decide_next(), UpdateKind::Weight);
    }

    #[test]
    fn decide_next_tie_falls_back_to_harness() {
        // Both levers measurable and equally efficient -> harness default.
        let mut s = AdaptiveScheduler::with_config(SchedulerConfig {
            plateau_eps: 0.01,
            min_harness_gens: 2,
        });
        s.record(rec(1, UpdateKind::Harness, 0.10, 10.0));
        s.record(rec(2, UpdateKind::Harness, 0.30, 10.0)); // +0.2/10 = 0.02
        s.record(rec(3, UpdateKind::Harness, 0.50, 10.0)); // +0.2/10 = 0.02
        s.record(rec(4, UpdateKind::Weight, 0.40, 10.0));
        s.record(rec(5, UpdateKind::Weight, 0.60, 10.0)); // +0.2/10 = 0.02
        assert!(!s.harness_plateaued());
        let h = s.improvement_efficiency(UpdateKind::Harness).unwrap();
        let w = s.improvement_efficiency(UpdateKind::Weight).unwrap();
        assert!((h - w).abs() < 1e-12, "expected tie: {h} vs {w}");
        assert_eq!(s.decide_next(), UpdateKind::Harness);
    }

    /// Reuse-of-#19 check: a harness series that makes
    /// [`should_trigger_weight_update`] true must also flip `decide_next` to
    /// `Weight`, proving the scheduler delegates its plateau decision to #19.
    #[test]
    fn plateau_signal_from_issue_19_flips_decision_to_weight() {
        let plateaued = [0.10, 0.60, 0.605, 0.606, 0.6065];
        // #19 directly says this series is a plateau at eps 0.01.
        assert!(should_trigger_weight_update(&plateaued, 0.01));

        let mut s = AdaptiveScheduler::new(); // eps 0.01, min_harness_gens 2
        for (i, &score) in plateaued.iter().enumerate() {
            s.record(rec(i as u32 + 1, UpdateKind::Harness, score, 10.0));
        }
        // Same plateau that #19 flags -> scheduler must pick Weight.
        assert_eq!(s.decide_next(), UpdateKind::Weight);
    }

    // -- efficiency_summary ----------------------------------------------------

    #[test]
    fn efficiency_summary_has_documented_keys_and_reflects_history() {
        let mut s = AdaptiveScheduler::new();
        s.record(rec(1, UpdateKind::Harness, 0.2, 10.0));
        s.record(rec(2, UpdateKind::Harness, 0.5, 100.0)); // +0.3/100 = 0.003
        s.record(rec(3, UpdateKind::Weight, 0.9, 10.0));

        let v = s.efficiency_summary();
        let obj = v.as_object().expect("summary is a JSON object");
        for key in [
            "harness_efficiency",
            "weight_efficiency",
            "last_kind",
            "harness_plateaued",
            "recommended_next",
        ] {
            assert!(obj.contains_key(key), "summary missing key '{key}': {v}");
        }

        // Harness has two records -> measurable; weight has one -> null.
        assert!((v["harness_efficiency"].as_f64().unwrap() - 0.003).abs() < 1e-12);
        assert!(v["weight_efficiency"].is_null());
        // Last recorded lever was the weight gen.
        assert_eq!(v["last_kind"], json!("weight"));
        assert_eq!(v["harness_plateaued"], json!(false));
        // Recommendation matches decide_next.
        assert_eq!(v["recommended_next"], json!(s.decide_next().as_str()));
    }

    #[test]
    fn efficiency_summary_on_empty_history() {
        let s = AdaptiveScheduler::new();
        let v = s.efficiency_summary();
        assert!(v["harness_efficiency"].is_null());
        assert!(v["weight_efficiency"].is_null());
        assert!(v["last_kind"].is_null());
        assert_eq!(v["harness_plateaued"], json!(false));
        // Empty history -> harness default.
        assert_eq!(v["recommended_next"], json!("harness"));
    }

    #[test]
    fn history_and_record_round_trip() {
        let mut s = AdaptiveScheduler::with_config(SchedulerConfig::default());
        assert!(s.history().is_empty());
        s.record(rec(1, UpdateKind::Harness, 0.4, 12.0));
        s.record(rec(2, UpdateKind::Weight, 0.7, 5.0));
        assert_eq!(s.history().len(), 2);
        assert_eq!(s.history()[0].kind, UpdateKind::Harness);
        assert_eq!(s.history()[1].generation, 2);
    }
}
