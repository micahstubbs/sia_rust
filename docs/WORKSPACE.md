# State-externalizing Workspace for Target Agents (issue #148)

A Rust port of the core idea from **Harness-1: Reinforcement Learning for Search
Agents with State-Externalizing Harnesses**
([arXiv:2606.02373](https://arxiv.org/abs/2606.02373),
[code](https://github.com/pat-jj/harness-1)).

> Search agents perform much better when you **externalize the working state**
> into a structured harness instead of forcing the model to keep everything in
> its context window. The model only makes high-level semantic decisions (what to
> search next, what to keep/verify, when to stop). Failures become *diagnosable*,
> which makes self-improvement far more effective.

This lives entirely under the non-default `llm` feature in
[`src/llm/workspace/`](../src/llm/workspace/). The Python reference has no
equivalent (its Target Agents are opaque subprocesses), so there is **no
cross-language parity surface** to match here.

## The five extensions from #148

| # | Idea | Where |
|---|------|-------|
| 1 | Structured workspace / evidence board + CRUD tools in the Target Agent | [`board.rs`](../src/llm/workspace/board.rs), [`tools.rs`](../src/llm/workspace/tools.rs) |
| 2 | Feedback Agent proposes **workspace schema** improvements (not just prompts) | [`schema.rs`](../src/llm/workspace/schema.rs) |
| 3 | Richer **credit assignment** for the adaptive scheduler (#84) | [`diagnostics.rs`](../src/llm/workspace/diagnostics.rs) |
| 4 | Applied to the `legal-issue-spotting` benchmark (#99) | [`legal.rs`](../src/llm/workspace/legal.rs) |
| 5 | The workspace **schema itself self-improves** across generations | [`schema.rs`](../src/llm/workspace/schema.rs) `WorkspaceSchema::apply` |

## 1. The board and its tools

[`Workspace`](../src/llm/workspace/board.rs) keeps three pools plus bookkeeping:

- **Candidates** — the raw document/observation pool, deduplicated by normalized
  content (`dedup_key`).
- **Evidence** — curated claims promoted from candidates, each with an importance
  score, a `VerificationStatus` (`unverified`/`verified`/`refuted`), provenance
  links, importance tags, and a bag of schema-evolved custom `fields`.
- **Searches** — the search history (query + result count).

Plus **budget-aware rendering**: `render_within(budget)` emits the goal, then
evidence (importance-descending), candidates, and searches, dropping the least
important material first and leaving a `… (N more … omitted to fit budget)`
marker — the Harness-1 "budget-aware context rendering" idea. The whole thing is
`serde`-serializable (`snapshot()`).

A Target Agent drives it through high-level semantic tools (the model spends its
reasoning on *decisions*, the harness holds the *state*):

```
workspace_add_candidate        workspace_curate_evidence     workspace_verify_claim
workspace_link_evidence        workspace_set_field           workspace_compress_candidate
workspace_record_search        workspace_render
```

[`WorkspaceSession`](../src/llm/workspace/tools.rs) wraps one `Workspace` and
dispatches these calls (errors come back as `Error:`-prefixed result strings so
the model can adapt). To expose them in the native Claude tool-use loop, use
`run_claude_agent_with_workspace(...)` instead of `run_claude_agent(...)`: it adds
the workspace tools alongside the file tools, routes workspace calls to the
session, and after the run persists the final state to **`workspace.json`** next
to `agent_execution.json` (the per-step deltas are already in the trajectory).
The default `run_claude_agent` path is unchanged and exposes no workspace tools.

## 2 & 5. Self-improving schema

The Feedback Agent can propose changes to the *structure* the agent operates
over, not just prompts. `SchemaProposal` is a flat `serde` enum
(`add_field` / `add_curation_rule` / `add_verification_rule`) that embeds directly
in the structured `improvement.json` (#88):

```json
{
  "summary": "tighten verification",
  "workspace_schema_changes": [
    {"kind": "add_field", "name": "jurisdiction", "description": "US state", "required": true},
    {"kind": "add_verification_rule", "name": "statute-check",
     "description": "verify statutes", "applies_to_tag": "statute"}
  ]
}
```

`WorkspaceSchema::proposals_from_improvement(&value)` parses them and
`apply`/`apply_all` accumulate them across generations (duplicates by name are
rejected), so the schema **compounds** over the run (extension 5).
`describe()` renders the active schema as prompt guidance; `validate(&Workspace)`
returns the rule violations.

## 3. Credit assignment

[`WorkspaceDiagnostics::analyze(&workspace, &schema)`](../src/llm/workspace/diagnostics.rs)
classifies a generation's dominant `FailureMode`:

- `insufficient_search` — gathered essentially nothing
- `poor_curation` — gathered candidates but curated no evidence
- `schema_violation` — broke an active schema rule
- `missing_verification` — curated evidence but left too much unverified
- `none` — the workspace looks healthy

`recommend(scheduler_default)` turns that into a harness-vs-weight lever choice
(reusing [`scheduler::UpdateKind`](../src/scheduler.rs)): a concrete,
harness-fixable structural failure pulls back to the cheap **harness** lever,
while a clean, well-formed workspace that still underperforms is corroborating
evidence the **model** is the bottleneck — making the scheduler's meta-decision
more informed.

## 4. Legal issue-spotting preset

[`legal.rs`](../src/llm/workspace/legal.rs) configures the board for
`legal-issue-spotting` (#99) using the **IRAC** structure (Issue, Rule,
Application, Conclusion): an `authority` + `jurisdiction` field, a verification
rule that every `rule`-tagged item must be checked against a controlling statute
or case before it counts, and a curation floor that drops trivial issues. Issue
spotting *is* search + evidence curation + verification, so the diagnosable
structure increases headroom and makes improvements interpretable.

## Tests

Every module is unit-tested inline and the runner integration is covered offline
with a scripted mock transport
(`claude_runner::tests::workspace_tools_drive_session_and_persist_snapshot`). Run:

```bash
cargo test --features llm workspace
cargo test --features llm claude_runner
```
