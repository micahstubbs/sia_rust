//! State-externalizing Workspace for Target Agents — issue #148 (Harness-1).
//!
//! This module ports the core idea of *Harness-1: Reinforcement Learning for
//! Search Agents with State-Externalizing Harnesses*
//! ([arXiv:2606.02373](https://arxiv.org/abs/2606.02373)) into sia_rust: instead
//! of forcing the Target Agent to maintain everything in its transcript, it
//! operates over a structured, recoverable [`Workspace`] and only makes
//! high-level semantic decisions. That makes self-improvement far more effective
//! because failures become *diagnosable*.
//!
//! The five issue-#148 extensions are implemented across the submodules:
//!
//! 1. **Structured workspace / evidence board** — [`board`] ([`Workspace`]) plus
//!    the rig-core/Anthropic tool layer in [`tools`] that lets a Target Agent
//!    drive it (`add_candidate`, `curate_evidence`, `verify_claim`, …).
//! 2. **Feedback agent improves the workspace schema** — [`schema`]
//!    ([`WorkspaceSchema`], [`SchemaProposal`]): the Feedback Agent can propose
//!    new fields and curation / verification rules, not just prompt edits.
//! 3. **Richer credit assignment for the scheduler** — [`diagnostics`]
//!    ([`WorkspaceDiagnostics`]) attributes a generation's outcome to a concrete
//!    failure mode (search / curation / verification) and recommends a lever.
//! 4. **Applied to the legal benchmark task** — [`legal`] configures an
//!    IRAC-style evidence board for `legal-issue-spotting` (#99).
//! 5. **Self-improving workspace** — [`schema::WorkspaceSchema::apply`] evolves
//!    the structure across generations from accepted [`SchemaProposal`]s.
//!
//! The whole subtree is gated behind the non-default `llm` feature.

pub mod board;
pub mod diagnostics;
pub mod legal;
pub mod schema;
pub mod tools;

pub use board::{dedup_key, Candidate, Evidence, SearchRecord, VerificationStatus, Workspace};
pub use diagnostics::{FailureMode, LeverRecommendation, WorkspaceDiagnostics};
pub use schema::{CurationRule, SchemaField, SchemaProposal, VerificationRule, WorkspaceSchema};
pub use tools::{workspace_tool_defs, WorkspaceSession};

/// Filename a workspace snapshot is logged under, alongside `agent_execution.json`.
pub const WORKSPACE_SNAPSHOT_JSON: &str = "workspace.json";
