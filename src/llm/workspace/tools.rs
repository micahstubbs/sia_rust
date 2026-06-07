//! The rig-core / Anthropic **tool layer** over a [`Workspace`] — issue #148
//! extension 1 (CRUD tools in the Target Agent) + extension 2 (state logged per
//! step).
//!
//! A [`WorkspaceSession`] wraps a single [`Workspace`] and dispatches the
//! workspace tool calls a Target Agent emits. The tools are deliberately
//! *high-level semantic actions* — `workspace_add_candidate`,
//! `workspace_curate_evidence`, `workspace_verify_claim`, … — so the model spends
//! its reasoning on *what* to keep / verify / search, while the recoverable state
//! lives in the harness (the Harness-1 thesis).
//!
//! Each dispatch returns a plain result string (errors prefixed with
//! [`super::super::tools::ERROR_PREFIX`]) so the native Claude loop can wrap it in
//! a `tool_result` block exactly like the file tools, and the session can be
//! snapshotted to `workspace.json` after the run for the Feedback Agent and the
//! adaptive scheduler to read.

use serde_json::{json, Value};

use super::super::anthropic_api::ToolDef;
use super::super::tools::ERROR_PREFIX;
use super::board::{VerificationStatus, Workspace};

/// Holds a [`Workspace`] and applies the workspace tool calls a Target Agent
/// makes against it.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSession {
    workspace: Workspace,
}

impl WorkspaceSession {
    /// Create a session over a fresh, empty workspace.
    pub fn new() -> Self {
        Self {
            workspace: Workspace::new(),
        }
    }

    /// Create a session over an existing (e.g. task-preset) workspace.
    pub fn with_workspace(workspace: Workspace) -> Self {
        Self { workspace }
    }

    /// Borrow the underlying workspace (e.g. to snapshot it).
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Consume the session, returning the accumulated workspace.
    pub fn into_workspace(self) -> Workspace {
        self.workspace
    }

    /// Whether `name` is one of the workspace tools this session dispatches.
    pub fn handles(name: &str) -> bool {
        workspace_tool_defs().iter().any(|d| d.name == name)
    }

    /// Apply one workspace tool call, mutating state and returning a result
    /// string (errors are `Error:`-prefixed). Unknown tools and malformed inputs
    /// return an error string rather than panicking, so the model can adapt.
    pub fn dispatch(&mut self, name: &str, input: &Value) -> String {
        match name {
            "workspace_add_candidate" => {
                let content = match req_str(input, "content") {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                let source = opt_str(input, "source");
                let existing_before = self.workspace.candidates.len();
                let id = self.workspace.add_candidate(content, source);
                if self.workspace.candidates.len() == existing_before {
                    format!("Candidate already present as {id} (deduplicated)")
                } else {
                    format!("Added candidate {id}")
                }
            }
            "workspace_record_search" => {
                let query = match req_str(input, "query") {
                    Ok(q) => q,
                    Err(e) => return e,
                };
                let results = input.get("results").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                self.workspace.record_search(query, results);
                format!("Recorded search ({results} results)")
            }
            "workspace_curate_evidence" => {
                let claim = match req_str(input, "claim") {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                let importance = input
                    .get("importance")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5);
                let provenance = str_array(input, "provenance");
                let tags = str_array(input, "tags");
                let id = self
                    .workspace
                    .curate_evidence(claim, importance, provenance, tags);
                format!("Curated evidence {id}")
            }
            "workspace_verify_claim" => {
                let id = match req_str(input, "id") {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let status = match input.get("status").and_then(|v| v.as_str()) {
                    Some("verified") => VerificationStatus::Verified,
                    Some("refuted") => VerificationStatus::Refuted,
                    Some("unverified") => VerificationStatus::Unverified,
                    Some(other) => {
                        return format!(
                            "{ERROR_PREFIX} invalid status '{other}'; expected verified, refuted, or unverified"
                        )
                    }
                    None => {
                        return format!("{ERROR_PREFIX} workspace_verify_claim requires a 'status'")
                    }
                };
                let note = opt_str(input, "note");
                match self.workspace.verify_claim(&id, status, note) {
                    Ok(()) => format!("Marked {id} as {}", status.as_str()),
                    Err(e) => format!("{ERROR_PREFIX} {e}"),
                }
            }
            "workspace_link_evidence" => {
                let id = match req_str(input, "id") {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let link = match req_str(input, "link") {
                    Ok(l) => l,
                    Err(e) => return e,
                };
                match self.workspace.link_evidence(&id, link) {
                    Ok(()) => format!("Linked provenance onto {id}"),
                    Err(e) => format!("{ERROR_PREFIX} {e}"),
                }
            }
            "workspace_set_field" => {
                let id = match req_str(input, "id") {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let key = match req_str(input, "key") {
                    Ok(k) => k,
                    Err(e) => return e,
                };
                let value = input.get("value").cloned().unwrap_or(Value::Null);
                match self.workspace.set_field(&id, key, value) {
                    Ok(()) => format!("Set field on {id}"),
                    Err(e) => format!("{ERROR_PREFIX} {e}"),
                }
            }
            "workspace_compress_candidate" => {
                let id = match req_str(input, "id") {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let summary = match req_str(input, "summary") {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                match self.workspace.compress_candidate(&id, summary) {
                    Ok(()) => format!("Compressed {id}"),
                    Err(e) => format!("{ERROR_PREFIX} {e}"),
                }
            }
            "workspace_render" => {
                let budget = input
                    .get("budget")
                    .and_then(|v| v.as_u64())
                    .map(|b| b as usize)
                    .unwrap_or(usize::MAX);
                self.workspace.render_within(budget)
            }
            other => format!("{ERROR_PREFIX} unknown workspace tool '{other}'"),
        }
    }
}

/// Require a non-null string field, returning an `Error:` string if absent.
fn req_str(input: &Value, key: &str) -> Result<String, String> {
    match input.get(key).and_then(|v| v.as_str()) {
        Some(s) => Ok(s.to_string()),
        None => Err(format!("{ERROR_PREFIX} missing required '{key}' string")),
    }
}

/// Read an optional string field.
fn opt_str(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Read an array-of-strings field, defaulting to empty. Non-string elements are
/// skipped.
fn str_array(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The Anthropic tool definitions for the workspace actions, exposed to the model
/// alongside the file tools.
pub fn workspace_tool_defs() -> Vec<ToolDef> {
    let s = |name: &str, description: &str, schema: Value| ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
    };
    vec![
        s(
            "workspace_add_candidate",
            "Add a raw document or observation to the candidate pool. Deduplicates by normalized \
             content. Returns the candidate id (e.g. cand-1) to use as provenance.",
            json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "The candidate's text."},
                    "source": {"type": "string", "description": "Optional provenance link (URL or document id)."}
                },
                "required": ["content"]
            }),
        ),
        s(
            "workspace_record_search",
            "Record a search you performed and how many results it returned, for history and \
             credit assignment.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The query you searched for."},
                    "results": {"type": "integer", "description": "Number of results returned."}
                },
                "required": ["query"]
            }),
        ),
        s(
            "workspace_curate_evidence",
            "Promote a curated claim into the evidence set with an importance score in [0,1], \
             provenance candidate ids, and optional importance tags. Returns the evidence id.",
            json!({
                "type": "object",
                "properties": {
                    "claim": {"type": "string", "description": "The curated claim."},
                    "importance": {"type": "number", "description": "Importance 0.0–1.0 (higher survives budget pressure)."},
                    "provenance": {"type": "array", "items": {"type": "string"}, "description": "Supporting candidate ids or URLs."},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Importance/category tags."}
                },
                "required": ["claim"]
            }),
        ),
        s(
            "workspace_verify_claim",
            "Set the verification status of an evidence item after checking it against an authority.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Evidence id, e.g. ev-1."},
                    "status": {"type": "string", "enum": ["verified", "refuted", "unverified"], "description": "Verification outcome."},
                    "note": {"type": "string", "description": "Optional rationale (e.g. the statute cited)."}
                },
                "required": ["id", "status"]
            }),
        ),
        s(
            "workspace_link_evidence",
            "Attach an additional provenance link (candidate id or URL) to an evidence item.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Evidence id, e.g. ev-1."},
                    "link": {"type": "string", "description": "Candidate id or URL to attach."}
                },
                "required": ["id", "link"]
            }),
        ),
        s(
            "workspace_set_field",
            "Set a custom field on an evidence item (used by schema-evolved attributes such as \
             jurisdiction or authority type).",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Evidence id, e.g. ev-1."},
                    "key": {"type": "string", "description": "Field name."},
                    "value": {"description": "Field value (any JSON value)."}
                },
                "required": ["id", "key", "value"]
            }),
        ),
        s(
            "workspace_compress_candidate",
            "Replace a candidate's content with a shorter summary to save context budget, keeping \
             its id and provenance.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Candidate id, e.g. cand-1."},
                    "summary": {"type": "string", "description": "The compressed content."}
                },
                "required": ["id", "summary"]
            }),
        ),
        s(
            "workspace_render",
            "Render the current workspace as text, optionally truncated to a character budget \
             (least important material is dropped first).",
            json!({
                "type": "object",
                "properties": {
                    "budget": {"type": "integer", "description": "Optional character budget for the rendered view."}
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_candidate_returns_id_and_dedups() {
        let mut s = WorkspaceSession::new();
        let r1 = s.dispatch("workspace_add_candidate", &json!({"content": "a doc"}));
        assert!(r1.contains("cand-1"), "{r1}");
        // Same content -> deduped to the same id, no growth.
        let r2 = s.dispatch(
            "workspace_add_candidate",
            &json!({"content": "  A DOC ", "source": "http://x"}),
        );
        assert!(r2.contains("cand-1"), "{r2}");
        assert_eq!(s.workspace().candidates.len(), 1);
    }

    #[test]
    fn curate_then_verify_then_link_flow() {
        let mut s = WorkspaceSession::new();
        s.dispatch("workspace_add_candidate", &json!({"content": "doc one"}));
        let r = s.dispatch(
            "workspace_curate_evidence",
            &json!({"claim": "the sky is blue", "importance": 0.8, "provenance": ["cand-1"], "tags": ["fact"]}),
        );
        assert!(r.contains("ev-1"), "{r}");
        let ev = s.workspace().evidence_by_id("ev-1").unwrap();
        assert_eq!(ev.importance, 0.8);
        assert_eq!(ev.provenance, vec!["cand-1"]);

        let v = s.dispatch(
            "workspace_verify_claim",
            &json!({"id": "ev-1", "status": "verified", "note": "checked"}),
        );
        assert!(!v.starts_with(ERROR_PREFIX), "{v}");
        assert_eq!(
            s.workspace().evidence_by_id("ev-1").unwrap().verification,
            VerificationStatus::Verified
        );

        let l = s.dispatch(
            "workspace_link_evidence",
            &json!({"id": "ev-1", "link": "cand-2"}),
        );
        assert!(!l.starts_with(ERROR_PREFIX), "{l}");
        assert!(s
            .workspace()
            .evidence_by_id("ev-1")
            .unwrap()
            .provenance
            .contains(&"cand-2".to_string()));
    }

    #[test]
    fn verify_unknown_id_returns_error_string_not_panic() {
        let mut s = WorkspaceSession::new();
        let r = s.dispatch(
            "workspace_verify_claim",
            &json!({"id": "ev-9", "status": "verified"}),
        );
        assert!(r.starts_with(ERROR_PREFIX), "{r}");
        assert!(r.contains("ev-9"));
    }

    #[test]
    fn invalid_status_is_rejected() {
        let mut s = WorkspaceSession::new();
        s.dispatch(
            "workspace_curate_evidence",
            &json!({"claim": "c", "importance": 0.5}),
        );
        let r = s.dispatch(
            "workspace_verify_claim",
            &json!({"id": "ev-1", "status": "maybe"}),
        );
        assert!(r.starts_with(ERROR_PREFIX), "{r}");
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let mut s = WorkspaceSession::new();
        let r = s.dispatch("workspace_add_candidate", &json!({"source": "x"}));
        assert!(r.starts_with(ERROR_PREFIX), "{r}");
        assert!(r.contains("content"));
    }

    #[test]
    fn record_search_and_compress_and_set_field() {
        let mut s = WorkspaceSession::new();
        s.dispatch(
            "workspace_record_search",
            &json!({"query": "q", "results": 3}),
        );
        assert_eq!(s.workspace().searches.len(), 1);

        let cid = s.dispatch(
            "workspace_add_candidate",
            &json!({"content": "a long original observation"}),
        );
        assert!(cid.contains("cand-1"));
        let c = s.dispatch(
            "workspace_compress_candidate",
            &json!({"id": "cand-1", "summary": "short"}),
        );
        assert!(!c.starts_with(ERROR_PREFIX), "{c}");
        assert_eq!(
            s.workspace().candidate_by_id("cand-1").unwrap().content,
            "short"
        );

        s.dispatch(
            "workspace_curate_evidence",
            &json!({"claim": "c", "importance": 0.5}),
        );
        let f = s.dispatch(
            "workspace_set_field",
            &json!({"id": "ev-1", "key": "jurisdiction", "value": "CA"}),
        );
        assert!(!f.starts_with(ERROR_PREFIX), "{f}");
        assert_eq!(
            s.workspace().evidence_by_id("ev-1").unwrap().fields["jurisdiction"],
            json!("CA")
        );
    }

    #[test]
    fn render_tool_returns_context_text() {
        let mut s = WorkspaceSession::new();
        s.dispatch(
            "workspace_curate_evidence",
            &json!({"claim": "important claim", "importance": 0.9}),
        );
        let r = s.dispatch("workspace_render", &json!({"budget": 500}));
        assert!(r.contains("# Workspace"), "{r}");
        assert!(r.contains("important claim"), "{r}");
    }

    #[test]
    fn unknown_tool_returns_error() {
        let mut s = WorkspaceSession::new();
        let r = s.dispatch("workspace_bogus", &json!({}));
        assert!(r.starts_with(ERROR_PREFIX), "{r}");
    }

    #[test]
    fn handles_recognizes_workspace_tools_only() {
        assert!(WorkspaceSession::handles("workspace_add_candidate"));
        assert!(!WorkspaceSession::handles("Bash"));
    }

    #[test]
    fn tool_defs_are_well_formed_and_namespaced() {
        let defs = workspace_tool_defs();
        assert!(defs.len() >= 8, "expected the full workspace tool set");
        for d in &defs {
            assert!(
                d.name.starts_with("workspace_"),
                "tool not namespaced: {}",
                d.name
            );
            assert_eq!(d.input_schema["type"], "object");
            assert!(d.input_schema.get("properties").is_some());
        }
    }
}
