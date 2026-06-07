//! Legal-issue-spotting preset for the Harness-1 workspace — issue #148
//! extension 4 (and task #99).
//!
//! Issue spotting is a near-perfect fit for the state-externalizing pattern:
//! it *is* search (find the relevant facts) + evidence curation (the spotted
//! issues) + verification (check each issue against the controlling statute or
//! case law). This module configures the generic [`Workspace`] / [`WorkspaceSchema`]
//! for that task using the classic **IRAC** structure (Issue, Rule, Application,
//! Conclusion):
//!
//! * Evidence is tagged with an IRAC role ([`tags`]).
//! * A [`SchemaField`] records the controlling `authority` (statute / case) and
//!   `jurisdiction` for each spotted issue.
//! * A [`VerificationRule`] requires every `rule`-tagged item to be verified
//!   against an authority before it counts — exactly the discipline that makes
//!   the agent's failures diagnosable.

use super::board::Workspace;
use super::schema::{CurationRule, SchemaField, VerificationRule, WorkspaceSchema};

/// The bundled benchmark task this preset targets (#99).
pub const LEGAL_ISSUE_SPOTTING_TASK: &str = "legal-issue-spotting";

/// IRAC evidence-role tags used to categorize curated evidence.
pub mod tags {
    /// A spotted legal issue.
    pub const ISSUE: &str = "issue";
    /// The controlling rule / statute / holding.
    pub const RULE: &str = "rule";
    /// Application of the rule to the facts.
    pub const APPLICATION: &str = "application";
    /// The conclusion for the issue.
    pub const CONCLUSION: &str = "conclusion";
}

/// The workspace schema for legal issue spotting: an `authority` and
/// `jurisdiction` field plus a verification rule that every `rule`-tagged item
/// must be checked against its authority, and a curation floor that keeps
/// trivial issues out of the evidence set.
pub fn legal_schema() -> WorkspaceSchema {
    let mut schema = WorkspaceSchema::base();
    // These applies are infallible on a fresh base schema (unique names).
    schema
        .apply(super::schema::SchemaProposal::AddField(SchemaField {
            name: "authority".into(),
            description: "Controlling authority for a rule: a statute citation or case name."
                .into(),
            required: false,
        }))
        .expect("authority field is unique on base schema");
    schema
        .apply(super::schema::SchemaProposal::AddField(SchemaField {
            name: "jurisdiction".into(),
            description: "Jurisdiction the authority applies in (e.g. CA, 9th Cir., federal)."
                .into(),
            required: false,
        }))
        .expect("jurisdiction field is unique on base schema");
    schema
        .apply(super::schema::SchemaProposal::AddVerificationRule(
            VerificationRule {
                name: "rule-must-cite-authority".into(),
                description:
                    "Every spotted rule must be verified against a controlling statute or \
                              case before it counts as evidence."
                        .into(),
                applies_to_tag: tags::RULE.into(),
            },
        ))
        .expect("verification rule is unique on base schema");
    schema
        .apply(super::schema::SchemaProposal::AddCurationRule(
            CurationRule {
                name: "drop-trivial-issues".into(),
                description: "Keep only issues with material importance to the outcome.".into(),
                min_importance: Some(0.2),
            },
        ))
        .expect("curation rule is unique on base schema");
    schema
}

/// The complete legal-issue-spotting preset: a fresh workspace seeded with the
/// task `goal`, paired with the matching [`legal_schema`].
///
/// Returning both together is deliberate — the schema (the IRAC fields and the
/// verification / curation rules) is what makes the workspace "legal", so a
/// caller can't accidentally build the board and forget to validate it against
/// the rules. Build a session with
/// `WorkspaceSession::with_workspace(workspace)` and keep the schema for
/// [`super::diagnostics::WorkspaceDiagnostics::analyze`].
pub fn legal_preset(goal: impl Into<String>) -> (Workspace, WorkspaceSchema) {
    (Workspace::with_goal(goal), legal_schema())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::workspace::board::VerificationStatus;
    use crate::llm::workspace::diagnostics::{FailureMode, WorkspaceDiagnostics};

    #[test]
    fn legal_schema_has_irac_authority_and_verification_rule() {
        let schema = legal_schema();
        assert!(schema.fields.iter().any(|f| f.name == "authority"));
        assert!(schema.fields.iter().any(|f| f.name == "jurisdiction"));
        assert!(schema
            .verification_rules
            .iter()
            .any(|r| r.applies_to_tag == tags::RULE));
        // The description renders into prompt guidance.
        assert!(schema.describe().contains("authority"));
    }

    #[test]
    fn unverified_rule_is_flagged_then_clears_when_verified() {
        let (mut ws, schema) = legal_preset("spot the issues in the fact pattern");
        ws.add_candidate("Plaintiff slipped on an unmarked wet floor.", None);
        let issue = ws.curate_evidence(
            "Premises liability for failure to warn",
            0.9,
            vec!["cand-1".into()],
            vec![tags::ISSUE.into()],
        );
        let rule = ws.curate_evidence(
            "A possessor of land owes a duty to warn invitees of known dangers",
            0.9,
            vec![],
            vec![tags::RULE.into()],
        );
        ws.set_field(
            &rule,
            "authority",
            serde_json::json!("Restatement (Second) of Torts § 343"),
        )
        .unwrap();
        let _ = issue;

        // Before verification: the rule-tagged item violates the schema.
        let before = WorkspaceDiagnostics::analyze(&ws, &schema);
        assert_eq!(before.primary_failure, FailureMode::SchemaViolation);

        // Verify the rule against its authority -> violation clears.
        ws.verify_claim(
            &rule,
            VerificationStatus::Verified,
            Some("checked § 343".into()),
        )
        .unwrap();
        let after = WorkspaceDiagnostics::analyze(&ws, &schema);
        assert_eq!(after.schema_violations, 0);
        assert_ne!(after.primary_failure, FailureMode::SchemaViolation);
    }

    #[test]
    fn trivial_issue_below_curation_floor_is_flagged() {
        let (mut ws, schema) = legal_preset("g");
        ws.add_candidate("doc", None);
        // Importance below the 0.2 curation floor.
        ws.curate_evidence("a trivial aside", 0.1, vec![], vec![tags::ISSUE.into()]);
        let violations = schema.validate(&ws);
        assert!(violations.iter().any(|v| v.contains("drop-trivial-issues")));
    }
}
