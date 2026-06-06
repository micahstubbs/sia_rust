//! Native Rust `Verifier` trait + reusable verifiers (issue #66).
//!
//! # Relationship to the Python `evaluate.py` contract
//!
//! Today a task is scored by an external `evaluate.py` script: the orchestrator
//! runs it as a subprocess (see [`crate::orchestrator::run_evaluation`]), the
//! script writes a `results.json`, and downstream code reads an `accuracy` field
//! in `[0, 1]` (correct / attempted; see the GPQA `evaluate.py`). That bridge is
//! the *system-of-record* and stays authoritative.
//!
//! This module is a **complementary, native** scoring layer. It defines a small
//! [`Verifier`] trait and a handful of reusable, fully-offline implementations
//! that score a single `(submission, reference)` pair without spawning Python.
//! Its [`VerifierOutcome::score`] uses the **same `[0, 1]` semantics** as the
//! Python `accuracy` so the two are directly comparable (1.0 == fully correct).
//!
//! The native path is deliberately scoped to *unit-level* checks (one answer vs.
//! one reference). It does **not** replace `evaluate.py` for whole-dataset
//! aggregation, per-domain breakdowns, or token/cost accounting. A future native
//! evaluation path *could* grow to deprecate the Python bridge for simple tasks,
//! but that is not claimed here — for now the two are intended to coexist: a task
//! author can use a [`Verifier`] for in-Rust tests / fast iteration and keep
//! `evaluate.py` as the canonical grader.
//!
//! # Verifier robustness and the Goodhart risk
//!
//! The SIA paper repeatedly flags **verifier quality** as a key limitation: a
//! self-improving loop optimizes against whatever the verifier rewards, so a weak
//! or brittle verifier is a direct source of *Goodhart's law* failure ("when a
//! measure becomes a target, it ceases to be a good measure"). A verifier that
//! accepts `"B"` but rejects `"The answer is B."` does not measure correctness —
//! it measures format compliance, and the optimizer will learn to game the format
//! instead of the task.
//!
//! To make that risk testable, this module ships [`adversarial_variants`] (which
//! produces semantically-equivalent perturbations of a submission: whitespace,
//! case, trailing punctuation, wrapping prose) and [`is_stable`] (which asserts a
//! verifier returns the *same* outcome across all of those variants). A robust
//! verifier should be stable; a brittle, format-only verifier will not be, and
//! the difference is asserted directly in this module's tests.
//!
//! # Error handling
//!
//! Every verifier is **panic-free** on malformed input. When a submission cannot
//! be interpreted, the verifier returns a *failing* outcome (`passed = false`,
//! `score = 0.0`) with a human-readable `details` string rather than erroring.

/// Outcome of verifying a single submission against a reference.
///
/// `score` is in `[0, 1]` with the same semantics as the Python `evaluate.py`
/// `accuracy` field (1.0 == fully correct, 0.0 == fully incorrect). `passed` is
/// the boolean pass/fail decision (for binary verifiers `passed == (score == 1.0)`;
/// partial-credit verifiers may pass at a threshold). `details` is a
/// human-readable explanation, always populated.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifierOutcome {
    /// Whether the submission is accepted.
    pub passed: bool,
    /// Score in `[0, 1]`, mirroring `evaluate.py` accuracy semantics.
    pub score: f64,
    /// Human-readable explanation of the outcome (never empty).
    pub details: String,
}

impl VerifierOutcome {
    /// A full pass (`score = 1.0`).
    fn pass(details: impl Into<String>) -> Self {
        VerifierOutcome {
            passed: true,
            score: 1.0,
            details: details.into(),
        }
    }

    /// A full fail (`score = 0.0`). Also used for all malformed-input paths.
    fn fail(details: impl Into<String>) -> Self {
        VerifierOutcome {
            passed: false,
            score: 0.0,
            details: details.into(),
        }
    }
}

/// A native scorer for a single `(submission, reference)` pair.
///
/// Implementations must be **panic-free** on arbitrary input and must produce a
/// [`VerifierOutcome`] whose `score` lies in `[0, 1]`.
pub trait Verifier {
    /// Stable identifier for the verifier (used in logs / details).
    fn name(&self) -> &str;

    /// Verify `submission` against `reference`, returning a scored outcome.
    fn verify(&self, submission: &str, reference: &str) -> VerifierOutcome;
}

// --------------------------------------------------------------------------- //
// ExactMatchVerifier
// --------------------------------------------------------------------------- //

/// Exact string equality, optionally trimming whitespace and ignoring case.
///
/// Scores `1.0`/`0.0`. With both `trim` and `case_insensitive` enabled this is a
/// robust default for short canonical answers.
#[derive(Debug, Clone, PartialEq)]
pub struct ExactMatchVerifier {
    /// Strip leading/trailing whitespace from both sides before comparing.
    pub trim: bool,
    /// Compare case-insensitively.
    pub case_insensitive: bool,
}

impl ExactMatchVerifier {
    /// Strict exact match: no trimming, case-sensitive.
    pub fn strict() -> Self {
        ExactMatchVerifier {
            trim: false,
            case_insensitive: false,
        }
    }

    /// Lenient exact match: trimmed and case-insensitive (the robust default).
    pub fn lenient() -> Self {
        ExactMatchVerifier {
            trim: true,
            case_insensitive: true,
        }
    }

    fn normalize(&self, s: &str) -> String {
        let s = if self.trim { s.trim() } else { s };
        if self.case_insensitive {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    }
}

impl Verifier for ExactMatchVerifier {
    fn name(&self) -> &str {
        "exact_match"
    }

    fn verify(&self, submission: &str, reference: &str) -> VerifierOutcome {
        if self.normalize(submission) == self.normalize(reference) {
            VerifierOutcome::pass(format!(
                "exact match (trim={}, ci={})",
                self.trim, self.case_insensitive
            ))
        } else {
            VerifierOutcome::fail(format!(
                "no match: submission {:?} != reference {:?}",
                submission, reference
            ))
        }
    }
}

// --------------------------------------------------------------------------- //
// MultipleChoiceVerifier
// --------------------------------------------------------------------------- //

/// Multiple-choice (A–D) verifier mirroring the GPQA `Answer` semantics.
///
/// Extracts the chosen letter from `submission` and compares it to the reference
/// letter. Scores `1.0`/`0.0`; an unparseable submission fails with `0.0`.
///
/// ## Note on the extractor
///
/// The canonical letter-extraction logic lives in [`crate::llm::structured`]
/// (`normalize_letter` / `Answer::letter`), but that module is gated behind the
/// non-default `llm` cargo feature. Since this verifier module is on the
/// **default** build, we cannot depend on it. We therefore re-implement a small,
/// standalone A–D extractor here ([`extract_choice_letter`]) that mirrors the
/// Python reference `parse_answer_letter` tail **exactly**:
///
/// 1. `.trim().to_uppercase()` the text;
/// 2. return it verbatim if it is exactly one of `A`/`B`/`C`/`D`;
/// 3. otherwise return the first of `A`,`B`,`C`,`D` (in that order) that appears
///    anywhere in the uppercased text;
/// 4. otherwise return `""`.
///
/// This is byte-for-byte the same algorithm as `structured::normalize_letter`;
/// keeping a copy here is the explicit trade-off for staying on the default build
/// with no `llm` dependency.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MultipleChoiceVerifier;

/// Extract a canonical A–D choice letter from arbitrary text (or `""`).
///
/// Mirrors the tail of the Python `parse_answer_letter` reference exactly (see
/// [`MultipleChoiceVerifier`] docs). Panic-free; returns `""` when no A–D letter
/// is present.
pub fn extract_choice_letter(text: &str) -> String {
    let upper = text.trim().to_uppercase();
    if matches!(upper.as_str(), "A" | "B" | "C" | "D") {
        return upper;
    }
    for letter in ['A', 'B', 'C', 'D'] {
        if upper.contains(letter) {
            return letter.to_string();
        }
    }
    String::new()
}

impl Verifier for MultipleChoiceVerifier {
    fn name(&self) -> &str {
        "multiple_choice"
    }

    fn verify(&self, submission: &str, reference: &str) -> VerifierOutcome {
        let got = extract_choice_letter(submission);
        let want = extract_choice_letter(reference);
        if want.is_empty() {
            return VerifierOutcome::fail(format!(
                "reference {:?} contains no A–D letter",
                reference
            ));
        }
        if got.is_empty() {
            return VerifierOutcome::fail(format!(
                "could not extract an A–D letter from submission {:?}",
                submission
            ));
        }
        if got == want {
            VerifierOutcome::pass(format!("chose {got} == reference {want}"))
        } else {
            VerifierOutcome::fail(format!("chose {got} != reference {want}"))
        }
    }
}

// --------------------------------------------------------------------------- //
// NumericToleranceVerifier
// --------------------------------------------------------------------------- //

/// Numeric verifier that passes when `|a - b| <= tol`.
///
/// Parses the first float out of both `submission` and `reference` (robust to
/// surrounding text such as `"The answer is 42.0 units"`), then compares within
/// the absolute tolerance. Scores `1.0`/`0.0`. Unparseable input fails with
/// `0.0` rather than panicking. A non-finite or negative `tol` is treated as
/// `0.0`.
#[derive(Debug, Clone, PartialEq)]
pub struct NumericToleranceVerifier {
    /// Absolute tolerance; the comparison passes iff `|a - b| <= tol`.
    pub tol: f64,
}

impl NumericToleranceVerifier {
    /// Construct with an absolute tolerance.
    pub fn new(tol: f64) -> Self {
        NumericToleranceVerifier { tol }
    }
}

/// Parse the first floating-point number embedded anywhere in `text`.
///
/// Recognizes an optional leading sign, digits with an optional decimal point,
/// and an optional `e`/`E` exponent. Returns `None` if no number is present.
/// Panic-free.
pub fn parse_first_float(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // A number candidate starts at a digit, a '.', or a sign immediately
        // followed by a digit or '.'.
        let starts = b.is_ascii_digit()
            || b == b'.'
            || ((b == b'+' || b == b'-')
                && i + 1 < bytes.len()
                && (bytes[i + 1].is_ascii_digit() || bytes[i + 1] == b'.'));
        if !starts {
            i += 1;
            continue;
        }
        let start = i;
        if b == b'+' || b == b'-' {
            i += 1;
        }
        let mut seen_digit = false;
        let mut seen_dot = false;
        while i < bytes.len() {
            let c = bytes[i];
            if c.is_ascii_digit() {
                seen_digit = true;
                i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                i += 1;
            } else {
                break;
            }
        }
        // Optional exponent: e[+/-]?digits
        if seen_digit && i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
            let exp_start = i;
            i += 1;
            if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                i += 1;
            }
            let mut exp_digits = false;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                exp_digits = true;
                i += 1;
            }
            // Roll back a malformed exponent (e.g. trailing "e" with no digits).
            if !exp_digits {
                i = exp_start;
            }
        }
        if seen_digit {
            if let Ok(value) = text[start..i].parse::<f64>() {
                return Some(value);
            }
        }
        // Not a valid number; advance past this candidate.
        i = (start + 1).max(i);
    }
    None
}

impl Verifier for NumericToleranceVerifier {
    fn name(&self) -> &str {
        "numeric_tolerance"
    }

    fn verify(&self, submission: &str, reference: &str) -> VerifierOutcome {
        let tol = if self.tol.is_finite() && self.tol >= 0.0 {
            self.tol
        } else {
            0.0
        };
        let a = match parse_first_float(submission) {
            Some(v) => v,
            None => {
                return VerifierOutcome::fail(format!(
                    "no number found in submission {:?}",
                    submission
                ))
            }
        };
        let b = match parse_first_float(reference) {
            Some(v) => v,
            None => {
                return VerifierOutcome::fail(format!(
                    "no number found in reference {:?}",
                    reference
                ))
            }
        };
        let diff = (a - b).abs();
        if diff.is_finite() && diff <= tol {
            VerifierOutcome::pass(format!("|{a} - {b}| = {diff} <= tol {tol}"))
        } else {
            VerifierOutcome::fail(format!("|{a} - {b}| = {diff} > tol {tol}"))
        }
    }
}

// --------------------------------------------------------------------------- //
// ContainsVerifier (partial credit)
// --------------------------------------------------------------------------- //

/// Substring / keyword verifier supporting partial credit.
///
/// The `reference` is split into keywords on whitespace; the score is the
/// fraction of keywords found in `submission` (each keyword matched at most once).
/// `passed` is true when `score >= pass_threshold`. Matching is case-insensitive.
/// With a single reference keyword this is a plain substring check.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainsVerifier {
    /// Minimum score (fraction of keywords present) required to pass, in `[0, 1]`.
    pub pass_threshold: f64,
}

impl ContainsVerifier {
    /// Require every reference keyword to be present (threshold `1.0`).
    pub fn all() -> Self {
        ContainsVerifier {
            pass_threshold: 1.0,
        }
    }

    /// Construct with a custom pass threshold (clamped to `[0, 1]`).
    pub fn with_threshold(pass_threshold: f64) -> Self {
        ContainsVerifier {
            pass_threshold: pass_threshold.clamp(0.0, 1.0),
        }
    }
}

impl Verifier for ContainsVerifier {
    fn name(&self) -> &str {
        "contains"
    }

    fn verify(&self, submission: &str, reference: &str) -> VerifierOutcome {
        let haystack = submission.to_lowercase();
        let keywords: Vec<String> = reference
            .split_whitespace()
            .map(|k| k.to_lowercase())
            .collect();
        if keywords.is_empty() {
            return VerifierOutcome::fail("reference has no keywords to match".to_string());
        }
        let found = keywords
            .iter()
            .filter(|k| haystack.contains(k.as_str()))
            .count();
        let score = found as f64 / keywords.len() as f64;
        let threshold = self.pass_threshold.clamp(0.0, 1.0);
        let passed = score >= threshold;
        VerifierOutcome {
            passed,
            score,
            details: format!(
                "matched {found}/{} keyword(s) (score {score:.3}, threshold {threshold:.3})",
                keywords.len()
            ),
        }
    }
}

// --------------------------------------------------------------------------- //
// Robustness / adversarial hooks
// --------------------------------------------------------------------------- //

/// Produce semantically-equivalent perturbations of `submission`.
///
/// These variants ("the same answer, worded/formatted differently") are used to
/// probe a verifier's stability. A correctness-measuring verifier should score
/// every variant identically; a brittle, format-only verifier will not — that
/// instability is exactly the Goodhart risk the SIA paper warns about for weak
/// verifiers (see the module docs).
///
/// The returned list always includes the original submission first, followed by:
/// extra surrounding whitespace, a case flip, a trailing period, and the answer
/// wrapped in explanatory prose.
pub fn adversarial_variants(submission: &str) -> Vec<String> {
    let mut variants = vec![submission.to_string()];

    // Extra surrounding/internal whitespace.
    variants.push(format!("  {submission}  \n"));

    // Case flip (uppercase letters become lowercase and vice versa).
    let flipped: String = submission
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    variants.push(flipped);

    // Trailing punctuation.
    variants.push(format!("{submission}."));

    // Wrapped in prose.
    variants.push(format!("The answer is {submission}, I'm confident."));

    variants
}

/// Whether `v` produces an *invariant* outcome across [`adversarial_variants`].
///
/// Returns `true` iff the verifier's [`VerifierOutcome`] is identical (same
/// `passed` and `score`) for every adversarial variant of `submission` when
/// scored against the same `reference`. `details` strings are intentionally
/// ignored (they legitimately echo the perturbed input). A robust verifier is
/// stable; use this in tests to guard against format-gaming regressions.
pub fn is_stable<V: Verifier>(v: &V, submission: &str, reference: &str) -> bool {
    let mut variants = adversarial_variants(submission).into_iter();
    let baseline = match variants.next() {
        Some(first) => v.verify(&first, reference),
        None => return true,
    };
    variants.all(|variant| {
        let outcome = v.verify(&variant, reference);
        outcome.passed == baseline.passed && outcome.score == baseline.score
    })
}

// --------------------------------------------------------------------------- //
// Tests — offline, default build.
// --------------------------------------------------------------------------- //
#[cfg(test)]
mod tests {
    use super::*;

    // ----- ExactMatchVerifier -----

    #[test]
    fn exact_match_pass_lenient() {
        let v = ExactMatchVerifier::lenient();
        let out = v.verify("  Hello ", "hello");
        assert!(out.passed);
        assert_eq!(out.score, 1.0);
    }

    #[test]
    fn exact_match_fail() {
        let v = ExactMatchVerifier::strict();
        let out = v.verify("hello", "world");
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
    }

    #[test]
    fn exact_match_malformed_no_panic() {
        // Empty strings, control chars, and non-ASCII must not panic.
        let v = ExactMatchVerifier::lenient();
        let out = v.verify("\0\u{1F600}", "");
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
        // Empty == empty is a legitimate pass.
        assert!(v.verify("", "").passed);
    }

    // ----- MultipleChoiceVerifier -----

    #[test]
    fn multiple_choice_pass_from_prose() {
        // "The answer is B" -> uppercased "THE ANSWER IS B" — note "ANSWER"
        // contains an 'A', so the FIRST A–D letter is 'A', mirroring the GPQA
        // reference. To genuinely test "B" extraction we use a phrase whose
        // first A–D letter really is B.
        let v = MultipleChoiceVerifier;
        let out = v.verify("My pick is B", "B");
        assert!(out.passed);
        assert_eq!(out.score, 1.0);

        // Bare letter still works.
        assert!(v.verify("B", "B").passed);

        // PARITY SUBTLETY: the extractor is a substring scan over A,B,C,D in
        // order (it does NOT JSON-parse), exactly like the GPQA reference's
        // `parse_answer_letter` tail. So `{"answer": "B"}` uppercases to a string
        // whose first A–D letter is 'A' (from "ANSWER"), and it would NOT match a
        // "B" reference. We assert that documented behavior rather than pretend
        // the scan understands JSON.
        assert_eq!(extract_choice_letter(r#"{"answer": "B"}"#), "A");
        assert!(!v.verify(r#"{"answer": "B"}"#, "B").passed);
    }

    #[test]
    fn multiple_choice_mismatch_fails() {
        let v = MultipleChoiceVerifier;
        let out = v.verify("C", "B");
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
        assert!(out.details.contains("!="));
    }

    #[test]
    fn multiple_choice_malformed_no_panic() {
        let v = MultipleChoiceVerifier;
        // No A–D letter in submission -> fail, no panic.
        let out = v.verify("none of the options fit", "B");
        // "none of the options fit" upper -> first of A,B,C,D present? none of
        // A/B/C/D letters appear, so extraction is "" -> fail.
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
        // Reference with no letter -> fail, no panic.
        assert!(!v.verify("B", "the reference").passed);
        // Empty inputs -> fail, no panic.
        assert!(!v.verify("", "").passed);
    }

    #[test]
    fn multiple_choice_mirrors_gpqa_letter_semantics() {
        // Parity with structured::normalize_letter: first of A,B,C,D in order.
        assert_eq!(extract_choice_letter("  d  "), "D");
        assert_eq!(extract_choice_letter("D or B?"), "B");
        assert_eq!(extract_choice_letter("answer: C"), "A"); // 'A' in "ANSWER"
        assert_eq!(extract_choice_letter("nope"), "");
    }

    // ----- NumericToleranceVerifier -----

    #[test]
    fn numeric_within_tolerance_passes() {
        let v = NumericToleranceVerifier::new(0.1);
        let out = v.verify("42.05", "42.0");
        assert!(out.passed);
        assert_eq!(out.score, 1.0);
    }

    #[test]
    fn numeric_outside_tolerance_fails() {
        let v = NumericToleranceVerifier::new(0.1);
        let out = v.verify("42.5", "42.0");
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
    }

    #[test]
    fn numeric_text_embedded_numbers() {
        let v = NumericToleranceVerifier::new(0.001);
        // Numbers embedded in surrounding prose.
        let out = v.verify("The answer is 3.14159 units.", "pi is about 3.1416");
        assert!(out.passed, "details: {}", out.details);
        // Scientific notation.
        assert!(v.verify("result = 1e-4", "0.0001").passed);
        // Negative numbers.
        assert!(v.verify("delta is -2.0", "-2.0").passed);
    }

    #[test]
    fn numeric_malformed_no_panic() {
        let v = NumericToleranceVerifier::new(0.1);
        // No number in submission -> fail, no panic.
        let out = v.verify("not a number", "42.0");
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
        // No number in reference -> fail, no panic.
        assert!(!v.verify("42.0", "nope").passed);
        // Lone signs / dots must not panic and yield no number.
        assert!(!v.verify("+ - .", "1.0").passed);
        // NaN tolerance is treated as 0.0; exact match still passes.
        let v_nan = NumericToleranceVerifier::new(f64::NAN);
        assert!(v_nan.verify("1.0", "1.0").passed);
        assert!(!v_nan.verify("1.0", "1.1").passed);
    }

    // ----- ContainsVerifier -----

    #[test]
    fn contains_full_pass() {
        let v = ContainsVerifier::all();
        let out = v.verify("the quick brown fox", "quick fox");
        assert!(out.passed);
        assert_eq!(out.score, 1.0);
    }

    #[test]
    fn contains_partial_credit_and_fail() {
        let v = ContainsVerifier::all();
        // Only one of two keywords present -> score 0.5, below threshold 1.0.
        let out = v.verify("the quick brown fox", "quick zebra");
        assert!(!out.passed);
        assert_eq!(out.score, 0.5);

        // A lenient threshold accepts the same partial match.
        let lenient = ContainsVerifier::with_threshold(0.5);
        assert!(lenient.verify("the quick brown fox", "quick zebra").passed);
    }

    #[test]
    fn contains_malformed_no_panic() {
        let v = ContainsVerifier::all();
        // Empty reference (no keywords) -> fail, no panic.
        let out = v.verify("anything", "   ");
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
        // Empty submission -> 0 matches, no panic.
        assert!(!v.verify("", "keyword").passed);
    }

    // ----- Robustness hooks -----

    #[test]
    fn adversarial_variants_includes_original_and_perturbations() {
        let variants = adversarial_variants("B");
        assert_eq!(variants[0], "B");
        // Whitespace, case flip, punctuation, prose -> at least 5 variants.
        assert!(variants.len() >= 5);
        assert!(variants.iter().any(|v| v.contains("The answer is")));
        assert!(variants.iter().any(|v| v.ends_with('.')));
    }

    #[test]
    fn robust_verifier_is_stable_strict_is_not() {
        // A robust, *extracting* verifier should be invariant across ALL
        // adversarial variants (whitespace, case, trailing punctuation, AND prose
        // wrapping). The NumericToleranceVerifier extracts the first number from
        // its input, so every variant of "42" — including
        // "The answer is 42, I'm confident." — still extracts 42 and scores the
        // same. This is the property a Goodhart-resistant verifier must have.
        let num = NumericToleranceVerifier::new(0.001);
        assert!(
            is_stable(&num, "42", "42"),
            "numeric extraction should be invariant across all adversarial variants"
        );

        // The strict, format-sensitive ExactMatch is NOT stable across variants:
        // a case flip, extra whitespace, trailing punctuation, or prose wrapping
        // all break raw string equality. This is exactly the brittle,
        // Goodhart-prone verifier the paper warns about — and `is_stable` catches
        // it.
        let strict = ExactMatchVerifier::strict();
        assert!(
            !is_stable(&strict, "Hello", "Hello"),
            "strict exact match must be unstable across adversarial variants"
        );
    }

    #[test]
    fn lenient_exact_match_stable_for_whitespace_case_only() {
        // Demonstrate the lenient ExactMatch IS stable when the perturbations are
        // limited to whitespace + case (it normalizes both away). We check the
        // first three variants (original, whitespace, case flip) directly.
        let v = ExactMatchVerifier::lenient();
        let variants = adversarial_variants("Hello");
        let baseline = v.verify(&variants[0], "hello");
        for variant in &variants[..3] {
            let out = v.verify(variant, "hello");
            assert_eq!(out.passed, baseline.passed);
            assert_eq!(out.score, baseline.score);
        }
        assert!(baseline.passed);
    }

    #[test]
    fn outcome_scores_stay_in_unit_interval() {
        // Sanity: every verifier keeps score in [0, 1] across a range of inputs.
        let inputs = [("B", "B"), ("C", "B"), ("3.0", "3.0"), ("x", "y"), ("", "")];
        let mc = MultipleChoiceVerifier;
        let num = NumericToleranceVerifier::new(0.5);
        let exact = ExactMatchVerifier::lenient();
        let contains = ContainsVerifier::with_threshold(0.5);
        for (sub, refr) in inputs {
            for out in [
                mc.verify(sub, refr),
                num.verify(sub, refr),
                exact.verify(sub, refr),
                contains.verify(sub, refr),
            ] {
                assert!(
                    (0.0..=1.0).contains(&out.score),
                    "score {} out of [0,1] for {:?}",
                    out.score,
                    (sub, refr)
                );
                assert!(!out.details.is_empty());
            }
        }
    }
}
