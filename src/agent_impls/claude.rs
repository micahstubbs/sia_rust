//! Claude agent impl. Port of `sia/agent_impls/claude.py`.
//!
//! Under the non-default `llm` cargo feature this is a **native** Anthropic
//! Messages API tool-use loop (issue #39): it POSTs the prompt + tool
//! definitions to `/v1/messages`, executes the returned `tool_use` blocks via
//! the sandboxed [`crate::llm::tools`] executors, feeds back `tool_result`
//! blocks, and repeats until the model ends its turn or `max_turns` is reached.
//! Without the feature it returns the historical integration-boundary error.
//!
//! The `provider` argument is accepted for a uniform signature but ignored:
//! Claude authenticates against Anthropic via `ANTHROPIC_API_KEY`, matching the
//! Python implementation (which delegates auth to the Claude Agent SDK).

use crate::agent_impls::base::RunArgs;
use crate::error::{SiaError, SiaResult};

/// Native Anthropic Messages API tool-loop runner (issue #39, `llm` feature).
#[cfg(feature = "llm")]
pub fn run_agent_claude(args: &RunArgs) -> SiaResult<()> {
    use crate::config::Config;
    use crate::llm::{run_claude_agent, HttpMessagesTransport};

    // Auth via ANTHROPIC_API_KEY (provider is intentionally ignored, matching Python).
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| SiaError::new("ANTHROPIC_API_KEY is not set"))?;

    // `max_turns` arrives as a String on RunArgs; parse to a sensible u32.
    let max_turns: u32 = args.max_turns.trim().parse().map_err(|_| {
        SiaError::new(format!(
            "invalid max_turns '{}': expected a positive integer",
            args.max_turns
        ))
    })?;
    if max_turns == 0 {
        return Err(SiaError::new("max_turns must be at least 1"));
    }

    // Allow an override base URL (used in integration setups); default to the API.
    let transport = match std::env::var("ANTHROPIC_BASE_URL") {
        Ok(base) if !base.trim().is_empty() => HttpMessagesTransport::with_base_url(api_key, base),
        _ => HttpMessagesTransport::new(api_key),
    };

    let config = Config::default();
    run_claude_agent(
        &transport,
        &args.model_name,
        max_turns,
        &args.prompt,
        &args.agent_working_directory,
        &config,
    )?;
    Ok(())
}

/// Default-build boundary error: the native runner requires the `llm` feature.
#[cfg(not(feature = "llm"))]
pub fn run_agent_claude(_args: &RunArgs) -> SiaResult<()> {
    Err(SiaError::new(
        "the native `claude` agent runner is not yet implemented (tracked in issue #39). \
         The registry, dispatch, and model resolution are ported; the Anthropic Messages \
         API tool-loop is pending, so `sia run`'s meta/feedback agents are not yet end-to-end.",
    ))
}
