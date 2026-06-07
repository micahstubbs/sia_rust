//! The state-externalizing [`Workspace`] (a.k.a. *evidence board*) — issue #148.
//!
//! Harness-1 ([arXiv:2606.02373](https://arxiv.org/abs/2606.02373)) shows search
//! agents perform markedly better when their working state is *externalized* into
//! a structured harness instead of being smeared across the context window. This
//! type is that harness for sia_rust Target Agents: a recoverable, inspectable
//! store the model mutates through a small set of high-level semantic actions
//! (add a candidate, curate evidence, verify a claim, …) rather than juggling raw
//! text.
//!
//! The board keeps three pools and some bookkeeping:
//!
//! * **Candidates** — the raw document / observation pool, dedup-keyed so the same
//!   observation is never stored twice.
//! * **Evidence** — curated claims promoted from candidates, each carrying an
//!   importance score, a [`VerificationStatus`], provenance links, tags, and
//!   (for schema evolution, issue #148 ext. 5) a bag of custom `fields`.
//! * **Searches** — the search history (query + result count) for credit
//!   assignment (issue #148 ext. 3, [`super::diagnostics`]).
//!
//! Everything is `serde`-serializable so a snapshot can be logged alongside the
//! trajectory ([`Workspace::snapshot`]), and renders to a **budget-aware** text
//! view ([`Workspace::render_within`]) that drops the least important material
//! first — the Harness-1 "budget-aware context rendering" idea.
//!
//! This whole subtree is gated behind the non-default `llm` feature; the Python
//! reference has no equivalent (its Target Agents are opaque subprocesses), so
//! there is no cross-language parity surface to match here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Verification state of a curated [`Evidence`] item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    /// Curated but not yet checked against an authority.
    Unverified,
    /// Confirmed against a source / authority.
    Verified,
    /// Checked and found false; kept so the agent does not re-derive it.
    Refuted,
}

impl VerificationStatus {
    /// Stable lower-case label used in rendered context and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            VerificationStatus::Unverified => "unverified",
            VerificationStatus::Verified => "verified",
            VerificationStatus::Refuted => "refuted",
        }
    }
}

/// A raw item in the candidate pool: a retrieved document, a search snippet, or
/// any observation the agent might later promote into curated evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Stable identifier, e.g. `"cand-1"`.
    pub id: String,
    /// The candidate's text (possibly compressed via
    /// [`Workspace::compress_candidate`]).
    pub content: String,
    /// Optional provenance link (URL / doc id) the candidate came from.
    pub source: Option<String>,
    /// Normalized key used to deduplicate candidates with the same content.
    pub dedup_key: String,
}

/// A curated claim promoted from the candidate pool, with the metadata the
/// Harness-1 evidence set tracks: importance, verification, provenance, tags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Stable identifier, e.g. `"ev-1"`.
    pub id: String,
    /// The curated claim text.
    pub claim: String,
    /// Importance score in `[0.0, 1.0]`; higher survives budget pressure.
    pub importance: f64,
    /// Whether the claim has been verified against an authority.
    pub verification: VerificationStatus,
    /// Supporting links: candidate ids (`"cand-3"`) and/or external URLs.
    pub provenance: Vec<String>,
    /// Free-form importance / category tags (Harness-1 "importance tags").
    pub tags: Vec<String>,
    /// Optional reviewer note (e.g. the reason a claim was refuted).
    pub note: Option<String>,
    /// Schema-evolved custom attributes (issue #148 ext. 5). A [`BTreeMap`] so
    /// the serialized order is deterministic. Skipped when empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
}

/// One recorded search action and the number of results it returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRecord {
    /// The query string the agent searched for.
    pub query: String,
    /// How many results the search yielded.
    pub results: usize,
}

/// The state-externalizing workspace a Target Agent operates over.
///
/// Construct with [`Workspace::new`] (or [`Workspace::with_goal`]), mutate through
/// the semantic methods, and render with [`Workspace::render`] /
/// [`Workspace::render_within`]. All ids are allocated monotonically and never
/// reused, so provenance links stay stable across the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Optional task goal shown at the top of the rendered context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// The raw candidate pool, in insertion order.
    pub candidates: Vec<Candidate>,
    /// Curated evidence, in insertion order (rendering sorts by importance).
    pub evidence: Vec<Evidence>,
    /// Search history, in chronological order.
    pub searches: Vec<SearchRecord>,
    /// Next candidate ordinal (monotonic; ids are `cand-<n>`).
    next_candidate: u64,
    /// Next evidence ordinal (monotonic; ids are `ev-<n>`).
    next_evidence: u64,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalize candidate text into a dedup key: trimmed, lower-cased, with internal
/// runs of whitespace collapsed to single spaces.
pub fn dedup_key(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl Workspace {
    /// Create an empty workspace with no goal.
    pub fn new() -> Self {
        Self {
            goal: None,
            candidates: Vec::new(),
            evidence: Vec::new(),
            searches: Vec::new(),
            next_candidate: 1,
            next_evidence: 1,
        }
    }

    /// Create an empty workspace with the given task goal.
    pub fn with_goal(goal: impl Into<String>) -> Self {
        let mut ws = Self::new();
        ws.goal = Some(goal.into());
        ws
    }

    /// Add a candidate to the pool, deduplicating by normalized content.
    ///
    /// Returns the id of the candidate. If an existing candidate has the same
    /// [`dedup_key`], no new candidate is stored and the **existing** id is
    /// returned (and its `source` is filled in if it was previously `None`).
    pub fn add_candidate(&mut self, content: impl Into<String>, source: Option<String>) -> String {
        let content = content.into();
        let key = dedup_key(&content);
        if let Some(existing) = self.candidates.iter_mut().find(|c| c.dedup_key == key) {
            if existing.source.is_none() {
                existing.source = source;
            }
            return existing.id.clone();
        }
        let id = format!("cand-{}", self.next_candidate);
        self.next_candidate += 1;
        self.candidates.push(Candidate {
            id: id.clone(),
            content,
            source,
            dedup_key: key,
        });
        id
    }

    /// Record a search action (query + result count) in the history.
    pub fn record_search(&mut self, query: impl Into<String>, results: usize) {
        self.searches.push(SearchRecord {
            query: query.into(),
            results,
        });
    }

    /// Curate a new piece of evidence, returning its id.
    ///
    /// `importance` is clamped to `[0.0, 1.0]`. The evidence starts
    /// [`VerificationStatus::Unverified`].
    pub fn curate_evidence(
        &mut self,
        claim: impl Into<String>,
        importance: f64,
        provenance: Vec<String>,
        tags: Vec<String>,
    ) -> String {
        let id = format!("ev-{}", self.next_evidence);
        self.next_evidence += 1;
        self.evidence.push(Evidence {
            id: id.clone(),
            claim: claim.into(),
            importance: importance.clamp(0.0, 1.0),
            verification: VerificationStatus::Unverified,
            provenance,
            tags,
            note: None,
            fields: BTreeMap::new(),
        });
        id
    }

    /// Look up a mutable evidence item by id.
    pub fn evidence_mut(&mut self, id: &str) -> Option<&mut Evidence> {
        self.evidence.iter_mut().find(|e| e.id == id)
    }

    /// Look up an evidence item by id.
    pub fn evidence_by_id(&self, id: &str) -> Option<&Evidence> {
        self.evidence.iter().find(|e| e.id == id)
    }

    /// Look up a candidate by id.
    pub fn candidate_by_id(&self, id: &str) -> Option<&Candidate> {
        self.candidates.iter().find(|c| c.id == id)
    }

    /// Set the verification status (and optional note) of an evidence item.
    ///
    /// Returns `Err` with a message if the id is unknown.
    pub fn verify_claim(
        &mut self,
        id: &str,
        status: VerificationStatus,
        note: Option<String>,
    ) -> Result<(), String> {
        match self.evidence_mut(id) {
            Some(ev) => {
                ev.verification = status;
                if note.is_some() {
                    ev.note = note;
                }
                Ok(())
            }
            None => Err(format!("unknown evidence id '{id}'")),
        }
    }

    /// Append a provenance link to an evidence item, avoiding duplicates.
    ///
    /// Returns `Err` if the id is unknown.
    pub fn link_evidence(&mut self, id: &str, link: impl Into<String>) -> Result<(), String> {
        let link = link.into();
        match self.evidence_mut(id) {
            Some(ev) => {
                if !ev.provenance.contains(&link) {
                    ev.provenance.push(link);
                }
                Ok(())
            }
            None => Err(format!("unknown evidence id '{id}'")),
        }
    }

    /// Set a schema-evolved custom field on an evidence item (issue #148 ext. 5).
    ///
    /// Returns `Err` if the id is unknown.
    pub fn set_field(
        &mut self,
        id: &str,
        key: impl Into<String>,
        value: Value,
    ) -> Result<(), String> {
        match self.evidence_mut(id) {
            Some(ev) => {
                ev.fields.insert(key.into(), value);
                Ok(())
            }
            None => Err(format!("unknown evidence id '{id}'")),
        }
    }

    /// Replace a candidate's content with a shorter summary (Harness-1
    /// "compress_observation"), preserving its id, source, and dedup key.
    ///
    /// Returns `Err` if the id is unknown.
    pub fn compress_candidate(
        &mut self,
        id: &str,
        summary: impl Into<String>,
    ) -> Result<(), String> {
        match self.candidates.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                c.content = summary.into();
                Ok(())
            }
            None => Err(format!("unknown candidate id '{id}'")),
        }
    }

    /// Number of curated evidence items whose status is
    /// [`VerificationStatus::Verified`].
    pub fn verified_count(&self) -> usize {
        self.evidence
            .iter()
            .filter(|e| e.verification == VerificationStatus::Verified)
            .count()
    }

    /// Evidence items sorted for rendering: importance descending, ties broken by
    /// ascending id ordinal (stable, deterministic).
    fn evidence_by_priority(&self) -> Vec<&Evidence> {
        let mut refs: Vec<&Evidence> = self.evidence.iter().collect();
        refs.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| id_ordinal(&a.id).cmp(&id_ordinal(&b.id)))
        });
        refs
    }

    /// Render the full workspace as human-readable context (no budget limit).
    pub fn render(&self) -> String {
        self.render_within(usize::MAX)
    }

    /// Render the workspace as context, dropping the least important material to
    /// keep the result within `budget` characters (a token proxy).
    ///
    /// Priority order, highest first: the goal header, curated evidence (by
    /// importance descending), the candidate pool, then the search history. When
    /// items are dropped, a short `… (N more … omitted)` marker is emitted in
    /// their place. The returned string never exceeds `budget` characters
    /// (assuming `budget` is at least large enough for the header).
    pub fn render_within(&self, budget: usize) -> String {
        let mut out = String::new();
        out.push_str("# Workspace\n");
        if let Some(goal) = &self.goal {
            push_if_fits(&mut out, &format!("Goal: {goal}\n"), budget);
        }

        // Curated evidence, importance-ordered (most important first).
        let evidence_lines: Vec<String> = self
            .evidence_by_priority()
            .iter()
            .map(|ev| {
                let mut line = format!(
                    "- [{}, importance {:.2}] {}",
                    ev.verification.as_str(),
                    ev.importance,
                    ev.claim
                );
                if !ev.provenance.is_empty() {
                    line.push_str(&format!(" (sources: {})", ev.provenance.join(", ")));
                }
                if !ev.tags.is_empty() {
                    line.push_str(&format!(" [tags: {}]", ev.tags.join(", ")));
                }
                line.push('\n');
                line
            })
            .collect();
        if !render_section(
            &mut out,
            budget,
            "\n## Evidence\n",
            &evidence_lines,
            "evidence",
        ) {
            return out;
        }

        // Candidate pool.
        let candidate_lines: Vec<String> = self
            .candidates
            .iter()
            .map(|c| {
                let src = c
                    .source
                    .as_deref()
                    .map(|s| format!(" <{s}>"))
                    .unwrap_or_default();
                format!("- {}: {}{}\n", c.id, c.content, src)
            })
            .collect();
        if !render_section(
            &mut out,
            budget,
            "\n## Candidates\n",
            &candidate_lines,
            "candidates",
        ) {
            return out;
        }

        // Search history.
        let search_lines: Vec<String> = self
            .searches
            .iter()
            .map(|s| format!("- \"{}\" → {} results\n", s.query, s.results))
            .collect();
        render_section(
            &mut out,
            budget,
            "\n## Searches\n",
            &search_lines,
            "searches",
        );

        out
    }

    /// Serialize the full workspace to a JSON value (for trajectory logging).
    pub fn snapshot(&self) -> Value {
        serde_json::to_value(self).expect("Workspace serializes to JSON")
    }
}

/// Parse the trailing ordinal out of an id like `"ev-12"` (used for stable
/// tie-breaking). Returns `u64::MAX` for ids without a numeric suffix so they
/// sort last deterministically.
fn id_ordinal(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

/// Append `line` to `out` only if doing so keeps `out` within `budget`
/// characters. Returns whether the line was appended.
fn push_if_fits(out: &mut String, line: &str, budget: usize) -> bool {
    if out.len() + line.len() <= budget {
        out.push_str(line);
        true
    } else {
        false
    }
}

/// Bytes reserved for a truncation marker so it always fits once we decide to
/// omit items. Comfortably larger than the longest `… (N more <noun> omitted to
/// fit budget)\n` marker we emit.
const MARKER_RESERVE: usize = 80;

/// Render one `## …` section into `out` within `budget`.
///
/// Emits the `header`, then as many `lines` (already newline-terminated, in
/// priority order) as fit — reserving [`MARKER_RESERVE`] bytes while items
/// remain so a `… (N more <noun> omitted to fit budget)` marker can always be
/// appended when something is dropped. Returns `false` if the header itself did
/// not fit (the caller should stop rendering further sections).
fn render_section(
    out: &mut String,
    budget: usize,
    header: &str,
    lines: &[String],
    noun: &str,
) -> bool {
    if !push_if_fits(out, header, budget) {
        return false;
    }
    if lines.is_empty() {
        push_if_fits(out, "(none)\n", budget);
        return true;
    }
    let mut shown = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        let remaining_after = lines.len() - idx - 1;
        let reserve = if remaining_after > 0 {
            MARKER_RESERVE
        } else {
            0
        };
        if out.len() + line.len() + reserve > budget {
            break;
        }
        out.push_str(line);
        shown += 1;
    }
    let omitted = lines.len() - shown;
    if omitted > 0 {
        push_if_fits(
            out,
            &format!("… ({omitted} more {noun} omitted to fit budget)\n"),
            budget,
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn add_candidate_allocates_stable_monotonic_ids() {
        let mut ws = Workspace::new();
        let a = ws.add_candidate("first doc", None);
        let b = ws.add_candidate("second doc", Some("http://x".into()));
        assert_eq!(a, "cand-1");
        assert_eq!(b, "cand-2");
        assert_eq!(ws.candidates.len(), 2);
        assert_eq!(
            ws.candidate_by_id("cand-2").unwrap().source.as_deref(),
            Some("http://x")
        );
    }

    #[test]
    fn add_candidate_dedups_by_normalized_content() {
        let mut ws = Workspace::new();
        let a = ws.add_candidate("The Quick Brown Fox", None);
        // Same content modulo whitespace + case -> same id, no new candidate.
        let b = ws.add_candidate("  the   quick brown   fox ", Some("src".into()));
        assert_eq!(a, b);
        assert_eq!(ws.candidates.len(), 1);
        // Source backfilled onto the original.
        assert_eq!(
            ws.candidate_by_id("cand-1").unwrap().source.as_deref(),
            Some("src")
        );
    }

    #[test]
    fn curate_evidence_clamps_importance_and_starts_unverified() {
        let mut ws = Workspace::new();
        let id = ws.curate_evidence("claim", 5.0, vec!["cand-1".into()], vec!["key".into()]);
        let ev = ws.evidence_by_id(&id).unwrap();
        assert_eq!(ev.importance, 1.0);
        assert_eq!(ev.verification, VerificationStatus::Unverified);
        let id2 = ws.curate_evidence("c2", -3.0, vec![], vec![]);
        assert_eq!(ws.evidence_by_id(&id2).unwrap().importance, 0.0);
    }

    #[test]
    fn verify_claim_updates_status_and_note() {
        let mut ws = Workspace::new();
        let id = ws.curate_evidence("c", 0.5, vec![], vec![]);
        ws.verify_claim(
            &id,
            VerificationStatus::Verified,
            Some("checked vs statute".into()),
        )
        .unwrap();
        let ev = ws.evidence_by_id(&id).unwrap();
        assert_eq!(ev.verification, VerificationStatus::Verified);
        assert_eq!(ev.note.as_deref(), Some("checked vs statute"));
        assert_eq!(ws.verified_count(), 1);
    }

    #[test]
    fn verify_unknown_id_errors() {
        let mut ws = Workspace::new();
        let err = ws
            .verify_claim("ev-99", VerificationStatus::Verified, None)
            .unwrap_err();
        assert!(err.contains("ev-99"));
    }

    #[test]
    fn link_evidence_appends_without_duplicates() {
        let mut ws = Workspace::new();
        let id = ws.curate_evidence("c", 0.5, vec!["cand-1".into()], vec![]);
        ws.link_evidence(&id, "cand-2").unwrap();
        ws.link_evidence(&id, "cand-2").unwrap(); // dup ignored
        assert_eq!(
            ws.evidence_by_id(&id).unwrap().provenance,
            vec!["cand-1", "cand-2"]
        );
    }

    #[test]
    fn set_field_stores_custom_attribute() {
        let mut ws = Workspace::new();
        let id = ws.curate_evidence("c", 0.5, vec![], vec![]);
        ws.set_field(&id, "jurisdiction", json!("CA")).unwrap();
        assert_eq!(
            ws.evidence_by_id(&id).unwrap().fields["jurisdiction"],
            json!("CA")
        );
    }

    #[test]
    fn compress_candidate_replaces_content_in_place() {
        let mut ws = Workspace::new();
        let id = ws.add_candidate("a very long original observation with lots of detail", None);
        ws.compress_candidate(&id, "short summary").unwrap();
        assert_eq!(ws.candidate_by_id(&id).unwrap().content, "short summary");
    }

    #[test]
    fn render_orders_evidence_by_importance_desc() {
        let mut ws = Workspace::with_goal("spot the issues");
        ws.curate_evidence("low importance", 0.2, vec![], vec![]);
        ws.curate_evidence("high importance", 0.9, vec![], vec![]);
        let out = ws.render();
        let hi = out.find("high importance").unwrap();
        let lo = out.find("low importance").unwrap();
        assert!(
            hi < lo,
            "high importance evidence must render before low\n{out}"
        );
        assert!(out.contains("Goal: spot the issues"));
    }

    #[test]
    fn render_within_budget_drops_least_important_and_stays_under_budget() {
        let mut ws = Workspace::new();
        // Many low-importance items, one high-importance item.
        ws.curate_evidence("KEEP ME", 1.0, vec![], vec![]);
        for i in 0..50 {
            ws.curate_evidence(
                format!("filler evidence number {i} with some length"),
                0.1,
                vec![],
                vec![],
            );
        }
        let budget = 300;
        let out = ws.render_within(budget);
        assert!(
            out.len() <= budget,
            "render exceeded budget: {} > {budget}",
            out.len()
        );
        assert!(
            out.contains("KEEP ME"),
            "highest-importance evidence was dropped:\n{out}"
        );
        assert!(
            out.contains("omitted to fit budget"),
            "expected a truncation marker:\n{out}"
        );
    }

    #[test]
    fn snapshot_round_trips_through_serde() {
        let mut ws = Workspace::with_goal("g");
        let c = ws.add_candidate("doc", Some("url".into()));
        let e = ws.curate_evidence("claim", 0.7, vec![c.clone()], vec!["tag".into()]);
        ws.verify_claim(&e, VerificationStatus::Verified, None)
            .unwrap();
        ws.record_search("query", 3);
        ws.set_field(&e, "k", json!(1)).unwrap();

        let snap = ws.snapshot();
        let restored: Workspace = serde_json::from_value(snap.clone()).unwrap();
        assert_eq!(restored, ws);
        // Stable id allocation survives the round trip.
        assert_eq!(restored.snapshot(), snap);
    }

    #[test]
    fn dedup_key_normalizes_whitespace_and_case() {
        assert_eq!(dedup_key("  Hello   World\n"), "hello world");
    }
}
