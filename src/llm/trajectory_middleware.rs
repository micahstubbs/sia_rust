//! Trajectory logging middleware / observer for rig-core agent runs (issue #51).
//!
//! [`TrajectoryMiddleware`] is a reusable observer that sits alongside an agent
//! execution loop and records structured [`TrajectoryEvent`]s — the user prompt,
//! assistant text turns, tool calls, tool results, and errors — while tracking
//! token usage and wall-clock timing. As each event is recorded it is mirrored
//! into the [`AgentTrajectory`] from issue #50 using its public `push_*`
//! builders, so the middleware reuses the trajectory storage rather than
//! reinventing it.
//!
//! Because the middleware renders into an [`AgentTrajectory`], its output is in
//! the exact Anthropic-style `agent_execution.json` shape consumed by
//! [`crate::orchestrator::load_agent_execution`] / the context manager. Calling
//! [`AgentTrajectory::write_to`] on the finished trajectory drops a file the
//! orchestrator reads back without any adapter (see the integration test).
//!
//! # How a runner drives it
//!
//! A runner such as [`crate::llm::RigAgentRunner`] would:
//!
//! 1. [`TrajectoryMiddleware::start`] before the loop (records the start instant),
//! 2. [`TrajectoryMiddleware::record`] one [`TrajectoryEvent`] per observed step
//!    (prompt, assistant text, tool call, tool result, error),
//! 3. [`TrajectoryMiddleware::record_usage`] whenever the provider reports token
//!    usage (see [`usage_from_rig`] for the rig-core adapter),
//! 4. [`TrajectoryMiddleware::finish`] at the end to stamp `duration_ms` and take
//!    ownership of the rendered [`AgentTrajectory`] plus the [`RunMetrics`].
//!
//! The whole module is gated behind the non-default `llm` cargo feature.

use std::time::Instant;

use serde_json::Value;

use super::AgentTrajectory;

/// A single structured event observed during an agent run.
#[derive(Debug, Clone, PartialEq)]
pub enum TrajectoryEvent {
    /// The user prompt that kicked off (or continued) the run.
    UserPrompt {
        /// The prompt text.
        text: String,
    },
    /// A textual assistant turn.
    AssistantText {
        /// The assistant's text.
        text: String,
    },
    /// The model requested a tool call.
    ToolCall {
        /// Provider-assigned tool-use id.
        id: String,
        /// Tool name.
        name: String,
        /// JSON input passed to the tool.
        input: Value,
    },
    /// A tool returned a result for a prior [`TrajectoryEvent::ToolCall`].
    ToolResult {
        /// The id of the tool call this result answers.
        tool_use_id: String,
        /// JSON content the tool produced.
        content: Value,
        /// Whether the tool reported an error.
        is_error: bool,
    },
    /// A run-level error (e.g. provider failure, loop abort).
    Error {
        /// Human-readable error message.
        message: String,
    },
}

/// Token usage accumulated across a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Input ("prompt") tokens.
    pub input_tokens: u64,
    /// Output ("completion") tokens.
    pub output_tokens: u64,
}

impl TokenUsage {
    /// Total tokens (input + output).
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Accumulate another usage into this one.
    pub fn add(&mut self, other: TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Structured summary of a run, suitable for a Feedback-Agent / context layer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunMetrics {
    /// Number of assistant text turns.
    pub num_turns: u32,
    /// Number of tool calls issued.
    pub num_tool_calls: u32,
    /// Number of errors (run-level errors plus error tool results).
    pub num_errors: u32,
    /// Accumulated token usage.
    pub usage: TokenUsage,
    /// Wall-clock duration of the run in milliseconds (set by [`TrajectoryMiddleware::finish`]).
    pub duration_ms: u128,
}

/// Observer that records structured events into an [`AgentTrajectory`] while
/// tracking metrics and timing.
#[derive(Debug, Clone, Default)]
pub struct TrajectoryMiddleware {
    trajectory: AgentTrajectory,
    events: Vec<TrajectoryEvent>,
    metrics: RunMetrics,
    started: Option<Instant>,
}

impl TrajectoryMiddleware {
    /// Create an empty middleware.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the run start instant. Idempotent: only the first call takes effect.
    pub fn start(&mut self) {
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
    }

    /// Core observer hook: append `event`, mirror it into the [`AgentTrajectory`],
    /// and update [`RunMetrics`].
    pub fn record(&mut self, event: TrajectoryEvent) {
        match &event {
            TrajectoryEvent::UserPrompt { text } => {
                self.trajectory.push_user_text(text.clone());
            }
            TrajectoryEvent::AssistantText { text } => {
                self.trajectory.push_assistant_text(text.clone());
                self.metrics.num_turns += 1;
            }
            TrajectoryEvent::ToolCall { id, name, input } => {
                self.trajectory
                    .push_assistant_tool_use(id.clone(), name.clone(), input.clone());
                self.metrics.num_tool_calls += 1;
            }
            TrajectoryEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                self.trajectory
                    .push_tool_result(tool_use_id.clone(), content.clone());
                if *is_error {
                    self.metrics.num_errors += 1;
                }
            }
            TrajectoryEvent::Error { message } => {
                self.trajectory
                    .push_assistant_text(format!("[error] {message}"));
                self.metrics.num_errors += 1;
            }
        }
        self.events.push(event);
    }

    /// Accumulate observed token usage into the metrics.
    pub fn record_usage(&mut self, usage: TokenUsage) {
        self.metrics.usage.add(usage);
    }

    /// Finish the run: stamp `duration_ms` from the start instant (if [`start`]
    /// was called) and return the rendered trajectory plus metrics.
    ///
    /// [`start`]: TrajectoryMiddleware::start
    pub fn finish(mut self) -> (AgentTrajectory, RunMetrics) {
        if let Some(started) = self.started {
            self.metrics.duration_ms = started.elapsed().as_millis();
        }
        (self.trajectory, self.metrics)
    }

    /// Borrow the rendered trajectory so far.
    pub fn trajectory(&self) -> &AgentTrajectory {
        &self.trajectory
    }

    /// Borrow the recorded events.
    pub fn events(&self) -> &[TrajectoryEvent] {
        &self.events
    }

    /// Borrow the accumulated metrics.
    pub fn metrics(&self) -> &RunMetrics {
        &self.metrics
    }
}

/// Adapt rig-core's [`rig::completion::Usage`] into a [`TokenUsage`].
///
/// In rig-core 0.22.0 the canonical usage type is `rig::completion::Usage`
/// (re-exported from `rig::completion::request::Usage`), with fields:
///
/// ```text
/// pub struct Usage {
///     pub input_tokens: u64,
///     pub output_tokens: u64,
///     pub total_tokens: u64,
/// }
/// ```
///
/// We map `input_tokens` / `output_tokens` directly; `total_tokens` is derived
/// on demand via [`TokenUsage::total`].
pub fn usage_from_rig(u: &rig::completion::Usage) -> TokenUsage {
    TokenUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn user_prompt_records_block_without_metric_changes() {
        let mut mw = TrajectoryMiddleware::new();
        mw.record(TrajectoryEvent::UserPrompt {
            text: "hello".into(),
        });

        assert_eq!(mw.metrics().num_turns, 0);
        assert_eq!(mw.metrics().num_tool_calls, 0);
        assert_eq!(mw.metrics().num_errors, 0);
        assert_eq!(mw.events().len(), 1);
        assert_eq!(
            mw.trajectory().to_agent_execution_json(),
            json!([{"role": "user", "content": "hello"}]),
        );
    }

    #[test]
    fn assistant_text_increments_turns() {
        let mut mw = TrajectoryMiddleware::new();
        mw.record(TrajectoryEvent::AssistantText {
            text: "hi there".into(),
        });

        assert_eq!(mw.metrics().num_turns, 1);
        assert_eq!(
            mw.trajectory().to_agent_execution_json(),
            json!([{"role": "assistant", "content": [{"type": "text", "text": "hi there"}]}]),
        );
    }

    #[test]
    fn tool_call_increments_tool_calls() {
        let mut mw = TrajectoryMiddleware::new();
        mw.record(TrajectoryEvent::ToolCall {
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: json!({"path": "a.txt"}),
        });

        assert_eq!(mw.metrics().num_tool_calls, 1);
        assert_eq!(
            mw.trajectory().to_agent_execution_json(),
            json!([{"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "a.txt"}}
            ]}]),
        );
    }

    #[test]
    fn tool_result_ok_does_not_increment_errors() {
        let mut mw = TrajectoryMiddleware::new();
        mw.record(TrajectoryEvent::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: json!("file contents"),
            is_error: false,
        });

        assert_eq!(mw.metrics().num_errors, 0);
        assert_eq!(
            mw.trajectory().to_agent_execution_json(),
            json!([{"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "file contents"}
            ]}]),
        );
    }

    #[test]
    fn tool_result_error_increments_errors() {
        let mut mw = TrajectoryMiddleware::new();
        mw.record(TrajectoryEvent::ToolResult {
            tool_use_id: "toolu_2".into(),
            content: json!("boom"),
            is_error: true,
        });

        assert_eq!(mw.metrics().num_errors, 1);
    }

    #[test]
    fn error_event_increments_errors_and_renders_note() {
        let mut mw = TrajectoryMiddleware::new();
        mw.record(TrajectoryEvent::Error {
            message: "provider failed".into(),
        });

        assert_eq!(mw.metrics().num_errors, 1);
        assert_eq!(
            mw.trajectory().to_agent_execution_json(),
            json!([{"role": "assistant", "content": [
                {"type": "text", "text": "[error] provider failed"}
            ]}]),
        );
    }

    #[test]
    fn token_usage_accumulates_and_totals() {
        let mut mw = TrajectoryMiddleware::new();
        mw.record_usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
        });
        mw.record_usage(TokenUsage {
            input_tokens: 3,
            output_tokens: 7,
        });

        assert_eq!(mw.metrics().usage.input_tokens, 13);
        assert_eq!(mw.metrics().usage.output_tokens, 12);
        assert_eq!(mw.metrics().usage.total(), 25);
    }

    #[test]
    fn timing_is_populated_after_start_sleep_finish() {
        let mut mw = TrajectoryMiddleware::new();
        mw.start();
        std::thread::sleep(Duration::from_millis(2));
        let (_traj, metrics) = mw.finish();

        assert!(
            metrics.duration_ms >= 1,
            "duration_ms should be >= 1 after a 2ms sleep, got {}",
            metrics.duration_ms,
        );
    }

    #[test]
    fn start_is_idempotent() {
        let mut mw = TrajectoryMiddleware::new();
        mw.start();
        std::thread::sleep(Duration::from_millis(2));
        // A second start must not reset the clock.
        mw.start();
        let (_traj, metrics) = mw.finish();
        assert!(metrics.duration_ms >= 1);
    }

    #[test]
    fn finish_without_start_leaves_duration_default() {
        let mw = TrajectoryMiddleware::new();
        let (_traj, metrics) = mw.finish();
        assert_eq!(metrics.duration_ms, 0);
    }

    #[test]
    fn mocked_multi_turn_loop_feeds_load_agent_execution() {
        // Simulate a full mocked model response: a multi-turn tool loop.
        let mut mw = TrajectoryMiddleware::new();
        mw.start();

        mw.record(TrajectoryEvent::UserPrompt {
            text: "Read a.txt then summarize".into(),
        });
        mw.record(TrajectoryEvent::AssistantText {
            text: "I'll read the file.".into(),
        });
        mw.record(TrajectoryEvent::ToolCall {
            id: "toolu_42".into(),
            name: "read_file".into(),
            input: json!({"path": "a.txt"}),
        });
        mw.record(TrajectoryEvent::ToolResult {
            tool_use_id: "toolu_42".into(),
            content: json!("hello world"),
            is_error: false,
        });
        mw.record(TrajectoryEvent::AssistantText {
            text: "The file says hello world.".into(),
        });
        mw.record_usage(TokenUsage {
            input_tokens: 120,
            output_tokens: 30,
        });

        // Metrics reflect the scripted loop.
        assert_eq!(mw.metrics().num_turns, 2);
        assert_eq!(mw.metrics().num_tool_calls, 1);
        assert_eq!(mw.metrics().num_errors, 0);
        assert_eq!(mw.metrics().usage.total(), 150);

        let expected_json = mw.trajectory().to_agent_execution_json();

        let dir = tempfile::tempdir().unwrap();
        let gen_dir = dir.path().to_str().unwrap();

        let (trajectory, _metrics) = mw.finish();
        trajectory.write_to(gen_dir).unwrap();

        let (value, is_multi) =
            crate::orchestrator::load_agent_execution(gen_dir, &Config::default());

        assert!(!is_multi, "single agent_execution.json must not be multi");
        assert_eq!(value, expected_json);
    }
}
