//! Self-improving **workspace schema** — issue #148 extensions 2 & 5.
//!
//! Extension 2 lets the Feedback Agent propose not just prompt / code edits but
//! changes to the *workspace schema itself*: new evidence fields and new
//! curation / verification rules. Extension 5 makes that loop compounding — the
//! [`WorkspaceSchema`] accumulates accepted [`SchemaProposal`]s across
//! generations, so the structure the Target Agent operates over evolves over the
//! run.
//!
//! Proposals are plain `serde` types so they can be embedded directly in the
//! structured `improvement.json` the Feedback Agent emits (issue #88) — see
//! [`WorkspaceSchema::proposals_from_improvement`]. Applying a schema both
//! changes the rendered guidance the Target Agent sees ([`WorkspaceSchema::describe`])
//! and the rules the [`super::diagnostics`] credit-assignment checks against
//! ([`WorkspaceSchema::validate`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::board::{VerificationStatus, Workspace};

/// A schema-evolved custom field the Target Agent may set on evidence
/// (issue #148 ext. 5), e.g. `jurisdiction` for the legal task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaField {
    /// Field key (must be unique within the schema).
    pub name: String,
    /// Human description shown in the Target Agent's guidance.
    pub description: String,
    /// Whether every curated evidence item must set this field.
    #[serde(default)]
    pub required: bool,
}

/// A curation rule governing what belongs in the evidence set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurationRule {
    /// Rule name (unique).
    pub name: String,
    /// Human description.
    pub description: String,
    /// Optional minimum importance; evidence below it violates the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_importance: Option<f64>,
}

/// A verification rule: evidence carrying `applies_to_tag` must be verified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationRule {
    /// Rule name (unique).
    pub name: String,
    /// Human description.
    pub description: String,
    /// Evidence tagged with this tag must reach [`VerificationStatus::Verified`].
    pub applies_to_tag: String,
}

/// A single schema change proposed by the Feedback Agent.
///
/// Internally tagged by `kind` so it serializes flat inside `improvement.json`:
/// `{"kind": "add_field", "name": "jurisdiction", "description": "...", "required": true}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaProposal {
    /// Introduce a new custom evidence field.
    AddField(SchemaField),
    /// Introduce a new curation rule.
    AddCurationRule(CurationRule),
    /// Introduce a new verification rule.
    AddVerificationRule(VerificationRule),
}

/// The active workspace schema: custom fields plus curation / verification rules.
///
/// Starts empty ([`WorkspaceSchema::base`]) and grows as proposals are accepted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSchema {
    /// Custom evidence fields.
    pub fields: Vec<SchemaField>,
    /// Curation rules.
    pub curation_rules: Vec<CurationRule>,
    /// Verification rules.
    pub verification_rules: Vec<VerificationRule>,
}

impl WorkspaceSchema {
    /// The empty base schema (only the built-in board fields exist).
    pub fn base() -> Self {
        Self::default()
    }

    /// Apply one proposal, evolving the schema. Returns `Err` (without mutating)
    /// if it would duplicate an existing field / rule name.
    pub fn apply(&mut self, proposal: SchemaProposal) -> Result<(), String> {
        match proposal {
            SchemaProposal::AddField(f) => {
                if self.fields.iter().any(|x| x.name == f.name) {
                    return Err(format!("field '{}' already exists", f.name));
                }
                self.fields.push(f);
            }
            SchemaProposal::AddCurationRule(r) => {
                if self.curation_rules.iter().any(|x| x.name == r.name) {
                    return Err(format!("curation rule '{}' already exists", r.name));
                }
                self.curation_rules.push(r);
            }
            SchemaProposal::AddVerificationRule(r) => {
                if self.verification_rules.iter().any(|x| x.name == r.name) {
                    return Err(format!("verification rule '{}' already exists", r.name));
                }
                self.verification_rules.push(r);
            }
        }
        Ok(())
    }

    /// Apply many proposals; returns the count applied and a list of rejection
    /// messages for those that duplicated existing names. Order-preserving.
    pub fn apply_all(&mut self, proposals: Vec<SchemaProposal>) -> (usize, Vec<String>) {
        let mut applied = 0usize;
        let mut rejected = Vec::new();
        for p in proposals {
            match self.apply(p) {
                Ok(()) => applied += 1,
                Err(e) => rejected.push(e),
            }
        }
        (applied, rejected)
    }

    /// Parse schema proposals out of an `improvement.json` value, reading the
    /// `workspace_schema_changes` array (issue #88). Malformed entries are
    /// skipped; a missing key yields an empty vec.
    pub fn proposals_from_improvement(improvement: &Value) -> Vec<SchemaProposal> {
        improvement
            .get("workspace_schema_changes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| serde_json::from_value(item.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Render the schema as guidance text for the Target Agent's prompt.
    pub fn describe(&self) -> String {
        let mut out = String::from("## Workspace schema\n");
        if self.fields.is_empty()
            && self.curation_rules.is_empty()
            && self.verification_rules.is_empty()
        {
            out.push_str("(base schema — no custom fields or rules)\n");
            return out;
        }
        if !self.fields.is_empty() {
            out.push_str("Custom evidence fields:\n");
            for f in &self.fields {
                let req = if f.required { " (required)" } else { "" };
                out.push_str(&format!("- {}{}: {}\n", f.name, req, f.description));
            }
        }
        if !self.curation_rules.is_empty() {
            out.push_str("Curation rules:\n");
            for r in &self.curation_rules {
                let thr = r
                    .min_importance
                    .map(|m| format!(" (min importance {m:.2})"))
                    .unwrap_or_default();
                out.push_str(&format!("- {}{}: {}\n", r.name, thr, r.description));
            }
        }
        if !self.verification_rules.is_empty() {
            out.push_str("Verification rules:\n");
            for r in &self.verification_rules {
                out.push_str(&format!(
                    "- {} (tag '{}'): {}\n",
                    r.name, r.applies_to_tag, r.description
                ));
            }
        }
        out
    }

    /// Validate a workspace against this schema, returning a list of human-readable
    /// violations (empty when the workspace fully conforms). Used by the
    /// diagnostics credit-assignment (issue #148 ext. 3).
    pub fn validate(&self, ws: &Workspace) -> Vec<String> {
        let mut violations = Vec::new();
        for ev in &ws.evidence {
            // Required custom fields.
            for f in &self.fields {
                if f.required && !ev.fields.contains_key(&f.name) {
                    violations.push(format!("{} is missing required field '{}'", ev.id, f.name));
                }
            }
            // Curation rules (min importance).
            for r in &self.curation_rules {
                if let Some(min) = r.min_importance {
                    if ev.importance < min {
                        violations.push(format!(
                            "{} importance {:.2} is below curation rule '{}' minimum {:.2}",
                            ev.id, ev.importance, r.name, min
                        ));
                    }
                }
            }
            // Verification rules (tagged evidence must be verified).
            for r in &self.verification_rules {
                if ev.tags.iter().any(|t| t == &r.applies_to_tag)
                    && ev.verification != VerificationStatus::Verified
                {
                    violations.push(format!(
                        "{} is tagged '{}' but not verified (rule '{}')",
                        ev.id, r.applies_to_tag, r.name
                    ));
                }
            }
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_field_then_reject_duplicate() {
        let mut schema = WorkspaceSchema::base();
        schema
            .apply(SchemaProposal::AddField(SchemaField {
                name: "jurisdiction".into(),
                description: "US state".into(),
                required: true,
            }))
            .unwrap();
        assert_eq!(schema.fields.len(), 1);
        let err = schema
            .apply(SchemaProposal::AddField(SchemaField {
                name: "jurisdiction".into(),
                description: "dup".into(),
                required: false,
            }))
            .unwrap_err();
        assert!(err.contains("already exists"));
        assert_eq!(schema.fields.len(), 1, "duplicate must not mutate");
    }

    #[test]
    fn proposal_serializes_flat_with_kind_tag() {
        let p = SchemaProposal::AddField(SchemaField {
            name: "authority".into(),
            description: "statute or case".into(),
            required: false,
        });
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "add_field");
        assert_eq!(v["name"], "authority");
        // Round trips.
        let back: SchemaProposal = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn proposals_parsed_from_improvement_json() {
        let improvement = json!({
            "summary": "tighten verification",
            "workspace_schema_changes": [
                {"kind": "add_verification_rule", "name": "statute-check", "description": "verify statutes", "applies_to_tag": "statute"},
                {"kind": "add_curation_rule", "name": "min-importance", "description": "drop trivia", "min_importance": 0.3},
                {"kind": "bogus", "x": 1}
            ]
        });
        let proposals = WorkspaceSchema::proposals_from_improvement(&improvement);
        assert_eq!(proposals.len(), 2, "malformed entry should be skipped");
    }

    #[test]
    fn validate_flags_missing_required_field_and_unverified_tag() {
        let mut schema = WorkspaceSchema::base();
        schema
            .apply(SchemaProposal::AddField(SchemaField {
                name: "jurisdiction".into(),
                description: "".into(),
                required: true,
            }))
            .unwrap();
        schema
            .apply(SchemaProposal::AddVerificationRule(VerificationRule {
                name: "statute-check".into(),
                description: "".into(),
                applies_to_tag: "statute".into(),
            }))
            .unwrap();

        let mut ws = Workspace::new();
        // Evidence tagged 'statute', unverified, missing jurisdiction -> 2 violations.
        ws.curate_evidence("a claim", 0.9, vec![], vec!["statute".into()]);
        let violations = schema.validate(&ws);
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(violations.iter().any(|v| v.contains("jurisdiction")));
        assert!(violations.iter().any(|v| v.contains("not verified")));
    }

    #[test]
    fn validate_clean_when_conforming() {
        let mut schema = WorkspaceSchema::base();
        schema
            .apply(SchemaProposal::AddCurationRule(CurationRule {
                name: "min-imp".into(),
                description: "".into(),
                min_importance: Some(0.5),
            }))
            .unwrap();
        let mut ws = Workspace::new();
        ws.curate_evidence("ok claim", 0.7, vec![], vec![]);
        assert!(schema.validate(&ws).is_empty());
    }

    #[test]
    fn schema_evolves_across_generations_compounding() {
        // Extension 5: each generation the Feedback Agent emits more changes and
        // the schema accumulates them.
        let mut schema = WorkspaceSchema::base();
        let gen1 = json!({"workspace_schema_changes": [
            {"kind": "add_field", "name": "jurisdiction", "description": "state", "required": false}
        ]});
        let gen2 = json!({"workspace_schema_changes": [
            {"kind": "add_verification_rule", "name": "vr", "description": "x", "applies_to_tag": "statute"},
            {"kind": "add_field", "name": "jurisdiction", "description": "dup", "required": false}
        ]});
        let (a1, _) = schema.apply_all(WorkspaceSchema::proposals_from_improvement(&gen1));
        let (a2, rejected2) = schema.apply_all(WorkspaceSchema::proposals_from_improvement(&gen2));
        assert_eq!(a1, 1);
        assert_eq!(a2, 1, "the duplicate field across generations is rejected");
        assert_eq!(rejected2.len(), 1);
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.verification_rules.len(), 1);
    }

    #[test]
    fn describe_renders_base_and_evolved() {
        let base = WorkspaceSchema::base();
        assert!(base.describe().contains("base schema"));
        let mut schema = WorkspaceSchema::base();
        schema
            .apply(SchemaProposal::AddField(SchemaField {
                name: "authority".into(),
                description: "statute/case".into(),
                required: true,
            }))
            .unwrap();
        let d = schema.describe();
        assert!(d.contains("authority"));
        assert!(d.contains("(required)"));
    }
}
