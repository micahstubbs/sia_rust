//! Native LLM client (issue #50, Phase 1 of the native LLM client).
//!
//! This module is gated behind the non-default `llm` cargo feature so the
//! published default build stays lean. The existing Python-bridge runners in
//! [`crate::agent_impls`] are left untouched.
//!
//! # Overview
//!
//! - [`AgentRunner`] is a small synchronous trait — one method, [`AgentRunner::run_agent`]
//!   — matching the existing `Runner` seam. Implementations run an agent to
//!   completion and return the final text plus a captured [`AgentTrajectory`].
//! - [`AgentTrajectory`] accumulates the conversation in the exact
//!   Anthropic-style `agent_execution.json` shape (a JSON array of
//!   `{"role", "content"}` messages with `text` / `tool_use` / `tool_result`
//!   content blocks). It round-trips through
//!   [`crate::orchestrator::load_agent_execution`] and the `crate::web::runs`
//!   consumers without any adapter.
//! - [`RigAgentRunner`] is the initial implementation, built on `rig-core`'s
//!   Anthropic provider. The trait impl is synchronous and blocks on an inherent
//!   async method via a current-thread tokio runtime.
//!
//! # Orchestrator seam
//!
//! Run an agent and persist its trajectory where the orchestrator expects it:
//!
//! ```no_run
//! # use sia::llm::{AgentRunner, RigAgentRunner, TrajectoryContext};
//! let runner = RigAgentRunner::from_env()?;
//! let ctx = TrajectoryContext {
//!     agent_working_directory: ".".into(),
//!     model_name: "claude-3-5-haiku-latest".into(),
//!     max_turns: 1024,
//!     system: Some("Be concise.".into()),
//! };
//! let outcome = runner.run_agent(&ctx, "Summarize the README.")?;
//! outcome.trajectory.write_to(&ctx.agent_working_directory)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Reuse by issue #51
//!
//! The [`AgentTrajectory`] `push_user_text` / `push_assistant_text` /
//! `push_assistant_tool_use` / `push_tool_result` builders are public and
//! ergonomic specifically so issue #51's middleware can record full tool-use
//! loops into the same JSON shape.

pub mod anthropic_api;
pub mod claude_runner;
pub mod openai_api;
pub mod openhands_runner;
pub mod provider_mapping;
pub mod pydantic_ai_runner;
pub mod retry;
mod rig_runner;
pub mod structured;
pub mod telemetry;
pub mod tools;
mod trajectory;
pub mod trajectory_middleware;

pub use anthropic_api::{
    ApiMessage, ApiUsage, ContentBlock, HttpMessagesTransport, MessagesRequest, MessagesResponse,
    MessagesTransport, ToolDef,
};
pub use claude_runner::run_claude_agent;
pub use openai_api::{
    ChatMessage, ChatRequest, ChatResponse, ChatTool, ChatTransport, ChatUsage, Choice,
    FunctionCall, FunctionDef, HttpChatTransport, ToolCall,
};
pub use openhands_runner::{run_openhands_agent, OpenHandsEventLog, OpenHandsRunSummary};
pub use provider_mapping::{
    api_key_for, base_url_for, chat_transport_for, client_for, messages_transport_for, AgentClient,
};
pub use pydantic_ai_runner::run_pydantic_ai_agent;
pub use retry::{
    backoff_delay_ms, is_transient_error, run_with_retry, RetryChatTransport,
    RetryMessagesTransport, RetryPolicy,
};
pub use rig_runner::RigAgentRunner;
pub use structured::{
    extract_answer, extract_json_value, extract_struct, Answer, RigStructuredExtractor,
};
pub use telemetry::{GenerationTelemetry, TelemetryLog, TELEMETRY_JSON};
pub use trajectory::AgentTrajectory;
pub use trajectory_middleware::{
    usage_from_rig, RunMetrics, TokenUsage, TrajectoryEvent, TrajectoryMiddleware,
};

/// Inputs describing how an agent run should be executed.
#[derive(Debug, Clone)]
pub struct TrajectoryContext {
    /// Working directory the agent operates in.
    pub agent_working_directory: String,
    /// Provider model identifier (e.g. `"claude-3-5-haiku-latest"`).
    pub model_name: String,
    /// Upper bound on agent turns / generation length.
    pub max_turns: u32,
    /// Optional system preamble.
    pub system: Option<String>,
}

/// Result of an agent run: the final assistant text and the captured trajectory.
#[derive(Debug, Clone)]
pub struct AgentRunOutcome {
    /// The agent's final textual response.
    pub final_text: String,
    /// The full captured conversation.
    pub trajectory: AgentTrajectory,
}

/// Runs an agent to completion, capturing its trajectory.
///
/// Synchronous on purpose, to match the existing `Runner` seam; async-backed
/// implementations (like [`RigAgentRunner`]) block internally.
pub trait AgentRunner {
    /// Run the agent to completion, returning the final text + captured trajectory.
    fn run_agent(
        &self,
        ctx: &TrajectoryContext,
        prompt: &str,
    ) -> crate::error::SiaResult<AgentRunOutcome>;
}
