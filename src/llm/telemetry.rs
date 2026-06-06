//! LLM generation telemetry (issue #64).
//!
//! A small, dependency-free layer that turns the [`RunMetrics`] the runners
//! already capture (token usage, tool-call count, wall-clock timing) into
//! structured per-generation telemetry plus a cumulative summary, and writes it
//! to a `telemetry.json` artifact next to the existing `agent_execution.json`.
//!
//! The whole module is gated behind the non-default `llm` cargo feature.
//!
//! # Reuse, not re-accounting
//!
//! [`GenerationTelemetry::from_metrics`] maps an already-populated
//! [`RunMetrics`] into telemetry, so the runners feed this layer with no new
//! token counting — the API-reported usage captured by
//! [`crate::llm::TrajectoryMiddleware::record_usage`] is the single source of
//! truth.
//!
//! # No dollar costs
//!
//! Mirroring the generator prompt note in [`crate::prompts`]: this layer
//! deliberately records only token counts / call counts / timing. It does **not**
//! compute or emit any dollar cost field, because per-provider pricing is
//! unknown.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::RunMetrics;

/// Filename for the per-run telemetry artifact, written next to
/// `agent_execution.json`.
pub const TELEMETRY_JSON: &str = "telemetry.json";

/// Token / call / timing telemetry for a single generation.
///
/// Deliberately carries **no** dollar-cost field — per-provider pricing is
/// unknown, so only token counts, call counts, and timing are recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationTelemetry {
    /// 1-based generation index (0 for a standalone / cumulative record).
    pub generation: u32,
    /// Input ("prompt") tokens reported by the provider.
    pub input_tokens: u64,
    /// Output ("completion") tokens reported by the provider.
    pub output_tokens: u64,
    /// Number of model API calls (assistant turns) in the generation.
    pub num_api_calls: u32,
    /// Number of tool calls issued during the generation.
    pub num_tool_calls: u32,
    /// Wall-clock duration of the generation in milliseconds.
    pub duration_ms: u128,
}

impl GenerationTelemetry {
    /// Total tokens (input + output).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Build telemetry from the [`RunMetrics`] a runner already captured.
    ///
    /// Reuses the existing accounting: `num_turns` becomes `num_api_calls`,
    /// `num_tool_calls` and `duration_ms` pass through, and the accumulated
    /// [`crate::llm::TokenUsage`] supplies input/output tokens. No tokens are
    /// re-counted here.
    pub fn from_metrics(generation: u32, metrics: &RunMetrics) -> Self {
        Self {
            generation,
            input_tokens: metrics.usage.input_tokens,
            output_tokens: metrics.usage.output_tokens,
            num_api_calls: metrics.num_turns,
            num_tool_calls: metrics.num_tool_calls,
            duration_ms: metrics.duration_ms,
        }
    }
}

/// An append-only log of per-generation telemetry with a cumulative summary.
#[derive(Debug, Clone, Default)]
pub struct TelemetryLog {
    entries: Vec<GenerationTelemetry>,
}

impl TelemetryLog {
    /// Create an empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one generation's telemetry.
    pub fn record(&mut self, entry: GenerationTelemetry) {
        self.entries.push(entry);
    }

    /// Borrow the recorded per-generation entries.
    pub fn entries(&self) -> &[GenerationTelemetry] {
        &self.entries
    }

    /// Cumulative summary: token counts, API calls, tool calls, and duration
    /// summed across every recorded generation.
    ///
    /// The summary's `generation` field is the number of generations recorded.
    pub fn cumulative(&self) -> GenerationTelemetry {
        let mut total = GenerationTelemetry {
            generation: self.entries.len() as u32,
            ..GenerationTelemetry::default()
        };
        for e in &self.entries {
            total.input_tokens += e.input_tokens;
            total.output_tokens += e.output_tokens;
            total.num_api_calls += e.num_api_calls;
            total.num_tool_calls += e.num_tool_calls;
            total.duration_ms += e.duration_ms;
        }
        total
    }

    /// Render the log as `{ "generations": [...], "cumulative": {...} }`.
    ///
    /// No dollar-cost field is emitted at any level.
    pub fn to_json(&self) -> Value {
        json!({
            "generations": self.entries,
            "cumulative": self.cumulative(),
        })
    }

    /// Write `telemetry.json` into `dir`, next to `agent_execution.json`.
    pub fn write_to(&self, dir: &str) -> io::Result<()> {
        let path = Path::new(dir).join(TELEMETRY_JSON);
        let body = serde_json::to_string_pretty(&self.to_json())?;
        fs::write(path, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::TokenUsage;

    fn metrics(input: u64, output: u64, turns: u32, tools: u32, ms: u128) -> RunMetrics {
        RunMetrics {
            num_turns: turns,
            num_tool_calls: tools,
            num_errors: 0,
            usage: TokenUsage {
                input_tokens: input,
                output_tokens: output,
            },
            duration_ms: ms,
        }
    }

    #[test]
    fn from_metrics_maps_every_field() {
        let m = metrics(120, 30, 2, 1, 1500);
        let t = GenerationTelemetry::from_metrics(3, &m);

        assert_eq!(t.generation, 3);
        assert_eq!(t.input_tokens, 120);
        assert_eq!(t.output_tokens, 30);
        assert_eq!(t.num_api_calls, 2);
        assert_eq!(t.num_tool_calls, 1);
        assert_eq!(t.duration_ms, 1500);
        assert_eq!(t.total_tokens(), 150);
    }

    #[test]
    fn cumulative_sums_multiple_generations() {
        let mut log = TelemetryLog::new();
        log.record(GenerationTelemetry::from_metrics(
            1,
            &metrics(100, 20, 1, 0, 500),
        ));
        log.record(GenerationTelemetry::from_metrics(
            2,
            &metrics(200, 50, 3, 2, 700),
        ));

        let c = log.cumulative();
        assert_eq!(c.generation, 2);
        assert_eq!(c.input_tokens, 300);
        assert_eq!(c.output_tokens, 70);
        assert_eq!(c.num_api_calls, 4);
        assert_eq!(c.num_tool_calls, 2);
        assert_eq!(c.duration_ms, 1200);
        assert_eq!(c.total_tokens(), 370);
    }

    #[test]
    fn empty_log_cumulative_is_zeroed() {
        let log = TelemetryLog::new();
        let c = log.cumulative();
        assert_eq!(c, GenerationTelemetry::default());
        assert_eq!(c.total_tokens(), 0);
    }

    #[test]
    fn to_json_has_generations_and_cumulative_shape() {
        let mut log = TelemetryLog::new();
        log.record(GenerationTelemetry::from_metrics(
            1,
            &metrics(10, 5, 1, 0, 100),
        ));
        log.record(GenerationTelemetry::from_metrics(
            2,
            &metrics(20, 5, 1, 1, 200),
        ));

        let v = log.to_json();
        let obj = v.as_object().expect("top-level object");
        assert_eq!(obj.len(), 2, "only generations + cumulative keys");
        assert!(obj.contains_key("generations"));
        assert!(obj.contains_key("cumulative"));

        let gens = v["generations"].as_array().unwrap();
        assert_eq!(gens.len(), 2);
        assert_eq!(v["generations"][0]["generation"], json!(1));
        assert_eq!(v["cumulative"]["input_tokens"], json!(30));
        assert_eq!(v["cumulative"]["output_tokens"], json!(10));
    }

    #[test]
    fn no_dollar_or_cost_field_is_emitted() {
        let mut log = TelemetryLog::new();
        log.record(GenerationTelemetry::from_metrics(
            1,
            &metrics(10, 5, 1, 0, 100),
        ));

        let text = log.to_json().to_string().to_lowercase();
        for forbidden in ["cost", "dollar", "price", "usd", "$"] {
            assert!(
                !text.contains(forbidden),
                "telemetry JSON must not mention '{forbidden}': {text}"
            );
        }
    }

    #[test]
    fn write_to_round_trips_from_disk() {
        let mut log = TelemetryLog::new();
        log.record(GenerationTelemetry::from_metrics(
            1,
            &metrics(100, 20, 1, 0, 500),
        ));
        log.record(GenerationTelemetry::from_metrics(
            2,
            &metrics(200, 50, 3, 2, 700),
        ));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        log.write_to(path).unwrap();

        let on_disk = dir.path().join(TELEMETRY_JSON);
        assert!(
            on_disk.is_file(),
            "telemetry.json written next to artifacts"
        );

        let read: Value = serde_json::from_str(&fs::read_to_string(&on_disk).unwrap()).unwrap();
        assert_eq!(read, log.to_json());

        // Per-generation entries round-trip back into the struct.
        let parsed: Vec<GenerationTelemetry> =
            serde_json::from_value(read["generations"].clone()).unwrap();
        assert_eq!(parsed, log.entries());

        // And the read-back content still carries no cost field.
        let text = fs::read_to_string(&on_disk).unwrap().to_lowercase();
        assert!(!text.contains("cost") && !text.contains("dollar"));
    }
}
