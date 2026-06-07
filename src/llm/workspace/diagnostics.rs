//! Workspace-driven **credit assignment** for the adaptive scheduler — issue #148
//! extension 3.
//!
//! Because the Target Agent's working state is externalized in the [`Workspace`],
//! a generation's failure is *diagnosable*: we can tell whether it gathered too
//! little ([`FailureMode::InsufficientSearch`]), gathered but failed to curate
//! ([`FailureMode::PoorCuration`]), curated but never verified
//! ([`FailureMode::MissingVerification`]), or broke its own schema rules
//! ([`FailureMode::SchemaViolation`]). [`WorkspaceDiagnostics::analyze`] computes
//! these signals; [`WorkspaceDiagnostics::recommend`] turns them into a
//! harness-vs-weight recommendation, making the scheduler's
//! ([`crate::scheduler`]) meta-decision more informed: a clear, harness-fixable
//! structural failure argues for another cheap *harness* edit, while a clean,
//! well-formed workspace that still underperforms is evidence the ceiling is the
//! model, favouring a *weight* update.

use serde::{Deserialize, Serialize};

use crate::scheduler::UpdateKind;

use super::board::Workspace;
use super::schema::WorkspaceSchema;

/// Minimum fraction of curated evidence that must be verified before the
/// workspace is considered to have a verification problem.
const VERIFIED_RATIO_FLOOR: f64 = 0.5;

/// The dominant workspace-level failure mode for a generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    /// Gathered essentially no material (no searches / candidates).
    InsufficientSearch,
    /// Gathered candidates but curated none into evidence.
    PoorCuration,
    /// Curated evidence but left too much of it unverified.
    MissingVerification,
    /// Violated the active workspace schema's rules.
    SchemaViolation,
    /// No clear workspace-level failure — the structure looks healthy.
    None,
}

impl FailureMode {
    /// Stable lower-case label for JSON / logging.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureMode::InsufficientSearch => "insufficient_search",
            FailureMode::PoorCuration => "poor_curation",
            FailureMode::MissingVerification => "missing_verification",
            FailureMode::SchemaViolation => "schema_violation",
            FailureMode::None => "none",
        }
    }

    /// Whether this failure is addressable by a *harness* (prompt / rule / scaffold)
    /// edit rather than a model weight update.
    pub fn is_harness_addressable(self) -> bool {
        !matches!(self, FailureMode::None)
    }
}

/// A harness-vs-weight lever recommendation derived from workspace diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct LeverRecommendation {
    /// The recommended lever.
    pub kind: UpdateKind,
    /// Human-readable justification (for logs / the scheduler trace).
    pub rationale: String,
}

/// Computed workspace signals for one generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceDiagnostics {
    /// Number of recorded searches.
    pub num_searches: usize,
    /// Number of candidates in the pool.
    pub num_candidates: usize,
    /// Number of curated evidence items.
    pub num_evidence: usize,
    /// Number of verified evidence items.
    pub num_verified: usize,
    /// `num_verified / num_evidence` (0.0 when no evidence).
    pub verified_ratio: f64,
    /// `num_evidence / num_candidates` (0.0 when no candidates).
    pub curation_ratio: f64,
    /// Count of active-schema violations.
    pub schema_violations: usize,
    /// The dominant failure mode.
    pub primary_failure: FailureMode,
    /// Supporting human-readable notes.
    pub notes: Vec<String>,
}

impl WorkspaceDiagnostics {
    /// Analyze a workspace against its active schema and classify the dominant
    /// failure mode.
    pub fn analyze(ws: &Workspace, schema: &WorkspaceSchema) -> Self {
        let num_searches = ws.searches.len();
        let num_candidates = ws.candidates.len();
        let num_evidence = ws.evidence.len();
        let num_verified = ws.verified_count();
        let verified_ratio = if num_evidence > 0 {
            num_verified as f64 / num_evidence as f64
        } else {
            0.0
        };
        let curation_ratio = if num_candidates > 0 {
            num_evidence as f64 / num_candidates as f64
        } else {
            0.0
        };
        let violations = schema.validate(ws);
        let schema_violations = violations.len();

        let mut notes = Vec::new();
        // Classify, most fundamental failure first.
        let primary_failure = if num_candidates == 0 && num_evidence == 0 {
            notes.push(format!(
                "only {num_searches} search(es) and no candidates gathered"
            ));
            FailureMode::InsufficientSearch
        } else if num_evidence == 0 {
            notes.push(format!(
                "{num_candidates} candidate(s) gathered but none curated into evidence"
            ));
            FailureMode::PoorCuration
        } else if schema_violations > 0 {
            notes.extend(violations.into_iter().take(5));
            FailureMode::SchemaViolation
        } else if verified_ratio < VERIFIED_RATIO_FLOOR {
            notes.push(format!(
                "only {num_verified}/{num_evidence} evidence verified (ratio {verified_ratio:.2})"
            ));
            FailureMode::MissingVerification
        } else {
            notes.push("workspace is well-formed (gathered, curated, verified)".to_string());
            FailureMode::None
        };

        Self {
            num_searches,
            num_candidates,
            num_evidence,
            num_verified,
            verified_ratio,
            curation_ratio,
            schema_violations,
            primary_failure,
            notes,
        }
    }

    /// Turn the diagnosis into a harness-vs-weight recommendation, given what the
    /// plateau-based scheduler would otherwise pick.
    ///
    /// * A harness-addressable failure overrides to [`UpdateKind::Harness`] — the
    ///   cheap lever can still fix a concrete structural problem (e.g. add a
    ///   verification rule, tighten curation guidance).
    /// * Otherwise (a clean workspace) we defer to the scheduler's default; if
    ///   that default is [`UpdateKind::Weight`], the clean workspace is corroborating
    ///   evidence that the model — not the harness — is the bottleneck.
    pub fn recommend(&self, scheduler_default: UpdateKind) -> LeverRecommendation {
        if self.primary_failure.is_harness_addressable() {
            let fix = match self.primary_failure {
                FailureMode::InsufficientSearch => {
                    "broaden the search strategy so the agent gathers more candidates"
                }
                FailureMode::PoorCuration => {
                    "improve curation guidance so gathered candidates become evidence"
                }
                FailureMode::MissingVerification => {
                    "add / strengthen verification rules so claims are checked"
                }
                FailureMode::SchemaViolation => {
                    "address the workspace schema violations before changing the model"
                }
                FailureMode::None => unreachable!("None is not harness-addressable"),
            };
            LeverRecommendation {
                kind: UpdateKind::Harness,
                rationale: format!(
                    "workspace failure '{}': {fix}",
                    self.primary_failure.as_str()
                ),
            }
        } else {
            let rationale = match scheduler_default {
                UpdateKind::Weight => {
                    "workspace is well-formed yet performance lags; the model is the likely \
                     bottleneck — following the scheduler toward a weight update"
                        .to_string()
                }
                UpdateKind::Harness => {
                    "no workspace-level failure detected; following the scheduler's harness default"
                        .to_string()
                }
            };
            LeverRecommendation {
                kind: scheduler_default,
                rationale,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::workspace::board::VerificationStatus;
    use crate::llm::workspace::schema::{SchemaProposal, VerificationRule};

    fn empty_schema() -> WorkspaceSchema {
        WorkspaceSchema::base()
    }

    #[test]
    fn empty_workspace_is_insufficient_search() {
        let ws = Workspace::new();
        let d = WorkspaceDiagnostics::analyze(&ws, &empty_schema());
        assert_eq!(d.primary_failure, FailureMode::InsufficientSearch);
    }

    #[test]
    fn candidates_without_evidence_is_poor_curation() {
        let mut ws = Workspace::new();
        ws.record_search("q", 3);
        ws.add_candidate("doc a", None);
        ws.add_candidate("doc b", None);
        let d = WorkspaceDiagnostics::analyze(&ws, &empty_schema());
        assert_eq!(d.primary_failure, FailureMode::PoorCuration);
        assert_eq!(d.num_candidates, 2);
        assert_eq!(d.num_evidence, 0);
    }

    #[test]
    fn unverified_evidence_is_missing_verification() {
        let mut ws = Workspace::new();
        ws.add_candidate("doc", None);
        ws.curate_evidence("c1", 0.8, vec![], vec![]);
        ws.curate_evidence("c2", 0.8, vec![], vec![]);
        // 0 of 2 verified -> ratio 0.0 < floor.
        let d = WorkspaceDiagnostics::analyze(&ws, &empty_schema());
        assert_eq!(d.primary_failure, FailureMode::MissingVerification);
        assert_eq!(d.verified_ratio, 0.0);
    }

    #[test]
    fn schema_violation_takes_priority_over_verification() {
        let mut schema = WorkspaceSchema::base();
        schema
            .apply(SchemaProposal::AddVerificationRule(VerificationRule {
                name: "vr".into(),
                description: "".into(),
                applies_to_tag: "statute".into(),
            }))
            .unwrap();
        let mut ws = Workspace::new();
        ws.add_candidate("doc", None);
        // tagged statute, unverified -> schema violation (and also unverified).
        ws.curate_evidence("c", 0.9, vec![], vec!["statute".into()]);
        let d = WorkspaceDiagnostics::analyze(&ws, &schema);
        assert_eq!(d.primary_failure, FailureMode::SchemaViolation);
        assert_eq!(d.schema_violations, 1);
    }

    #[test]
    fn well_formed_workspace_has_no_failure() {
        let mut ws = Workspace::new();
        ws.record_search("q", 5);
        ws.add_candidate("doc", None);
        let e = ws.curate_evidence("c", 0.9, vec!["cand-1".into()], vec![]);
        ws.verify_claim(&e, VerificationStatus::Verified, None)
            .unwrap();
        let d = WorkspaceDiagnostics::analyze(&ws, &empty_schema());
        assert_eq!(d.primary_failure, FailureMode::None);
        assert_eq!(d.verified_ratio, 1.0);
    }

    #[test]
    fn recommend_overrides_to_harness_on_structural_failure() {
        let ws = Workspace::new(); // insufficient search
        let d = WorkspaceDiagnostics::analyze(&ws, &empty_schema());
        // Even if the scheduler wanted a weight update, a fixable structural
        // failure pulls back to the cheap harness lever.
        let rec = d.recommend(UpdateKind::Weight);
        assert_eq!(rec.kind, UpdateKind::Harness);
        assert!(rec.rationale.contains("insufficient_search"));
    }

    #[test]
    fn recommend_defers_to_scheduler_when_clean() {
        let mut ws = Workspace::new();
        ws.record_search("q", 5);
        ws.add_candidate("doc", None);
        let e = ws.curate_evidence("c", 0.9, vec!["cand-1".into()], vec![]);
        ws.verify_claim(&e, VerificationStatus::Verified, None)
            .unwrap();
        let d = WorkspaceDiagnostics::analyze(&ws, &empty_schema());
        assert_eq!(d.recommend(UpdateKind::Weight).kind, UpdateKind::Weight);
        assert_eq!(d.recommend(UpdateKind::Harness).kind, UpdateKind::Harness);
    }

    #[test]
    fn diagnostics_serialize_to_json() {
        let ws = Workspace::new();
        let d = WorkspaceDiagnostics::analyze(&ws, &empty_schema());
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["primary_failure"], "insufficient_search");
        assert!(v.get("verified_ratio").is_some());
    }
}
