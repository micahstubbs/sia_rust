//! [`RigAgentRunner`]: an [`AgentRunner`] backed by `rig-core`'s Anthropic client.
//!
//! The runner builds a `rig` Anthropic agent for the requested model, sends a
//! single prompt, and records the user prompt + assistant response into an
//! [`AgentTrajectory`]. The public [`AgentRunner`] trait is synchronous to match
//! the existing `Runner` seam; the sync impl blocks on the inherent
//! [`RigAgentRunner::run_agent_async`] via a current-thread tokio runtime.

use rig::client::completion::CompletionClient;
use rig::completion::Prompt;
use rig::providers::anthropic;

use crate::error::{SiaError, SiaResult};

use super::{AgentRunOutcome, AgentRunner, AgentTrajectory, TrajectoryContext};

/// An [`AgentRunner`] that drives `rig-core`'s Anthropic provider.
#[derive(Debug, Clone)]
pub struct RigAgentRunner {
    api_key: String,
}

impl RigAgentRunner {
    /// Construct a runner with an explicit Anthropic API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }

    /// Construct a runner, reading the key from `ANTHROPIC_API_KEY`.
    ///
    /// Returns a [`SiaError`] (mirroring the crate's user-facing error style) if
    /// the variable is unset.
    pub fn from_env() -> SiaResult<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| SiaError::new("ANTHROPIC_API_KEY is not set"))?;
        Ok(Self::new(api_key))
    }

    /// Run the agent asynchronously: build the Anthropic agent, send the prompt,
    /// and capture the prompt/response into an [`AgentTrajectory`].
    ///
    /// Phase 1 captures a single prompt/response turn; the trajectory type already
    /// supports tool_use/tool_result blocks for the issue #51 tool-loop.
    pub async fn run_agent_async(
        &self,
        ctx: &TrajectoryContext,
        prompt: &str,
    ) -> SiaResult<AgentRunOutcome> {
        let client = anthropic::Client::new(&self.api_key);

        let mut builder = client
            .agent(&ctx.model_name)
            .max_tokens(ctx.max_turns as u64);
        if let Some(system) = ctx.system.as_deref() {
            builder = builder.preamble(system);
        }
        let agent = builder.build();

        let final_text = agent
            .prompt(prompt)
            .await
            .map_err(|e| SiaError::new(format!("rig prompt failed: {e}")))?;

        let mut trajectory = AgentTrajectory::new();
        trajectory.push_user_text(prompt);
        trajectory.push_assistant_text(final_text.clone());

        Ok(AgentRunOutcome {
            final_text,
            trajectory,
        })
    }
}

impl AgentRunner for RigAgentRunner {
    fn run_agent(&self, ctx: &TrajectoryContext, prompt: &str) -> SiaResult<AgentRunOutcome> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SiaError::new(format!("failed to build tokio runtime: {e}")))?;
        runtime.block_on(self.run_agent_async(ctx, prompt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_errors_when_missing() {
        // Snapshot + clear the var so the test is deterministic regardless of host env.
        let saved = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::remove_var("ANTHROPIC_API_KEY");

        let result = RigAgentRunner::from_env();

        if let Some(v) = saved {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }

        assert!(result.is_err());
    }

    #[test]
    fn new_holds_explicit_key() {
        let runner = RigAgentRunner::new("test-key");
        assert_eq!(runner.api_key, "test-key");
    }

    /// Live test against the real Anthropic API. Ignored so CI never needs a key.
    #[test]
    #[ignore = "requires ANTHROPIC_API_KEY and network access"]
    fn live_single_turn() {
        let runner = RigAgentRunner::from_env().expect("ANTHROPIC_API_KEY must be set");
        let ctx = TrajectoryContext {
            agent_working_directory: ".".to_string(),
            model_name: "claude-3-5-haiku-latest".to_string(),
            max_turns: 1024,
            system: Some("Be concise.".to_string()),
        };
        let outcome = runner
            .run_agent(&ctx, "Reply with the single word: pong")
            .expect("live run should succeed");
        assert!(!outcome.final_text.is_empty());
        assert_eq!(outcome.trajectory.messages().len(), 2);
    }
}
