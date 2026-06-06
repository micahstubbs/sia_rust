//! PydanticAI agent impl. Port of `sia/agent_impls/pydantic_ai.py`.
//!
//! PydanticAI has no Rust equivalent; `run_agent_pydantic_ai` is the integration
//! boundary. The `resolve_model` passthrough logic is ported and tested.

use crate::agent_impls::base::RunArgs;
use crate::error::{SiaError, SiaResult};
use crate::providers::Provider;

/// Resolve the model spec for PydanticAI.
///
/// Without a provider the value is passed through to PydanticAI's native parsing.
/// For an OpenAI-compatible provider with a base_url, Python builds an
/// `OpenAIChatModel`; the Rust port (which cannot construct that object) passes the
/// model name through.
pub fn resolve_model(model_name: &str, _provider: Option<&Provider>) -> String {
    model_name.to_string()
}

/// Run a meta/feedback agent using a native PydanticAI-style tool loop (issue #41).
///
/// Mirrors `sia/agent_impls/pydantic_ai.py`: builds a chat-completions agent with
/// the three PydanticAI tools (`write_file` / `read_file` / `bash`) operating
/// relative to `agent_working_directory`, and caps the run at `max_turns`
/// *requests* (the `UsageLimits(request_limit=max_turns)` equivalent). The run is
/// captured into an `AgentTrajectory` and written to
/// `agent_working_directory/agent_execution.json`.
///
/// The model spec passes through `resolve_model`. For an OpenAI-compatible
/// provider with a `base_url` the client is pointed at that endpoint (the role
/// Python's `OpenAIChatModel` / `OpenAIProvider` plays); the model name is used
/// as-is (PydanticAI does not prefix it). Without a provider, an OpenAI-compatible
/// base + `OPENAI_API_KEY` is assumed.
#[cfg(feature = "llm")]
pub fn run_agent_pydantic_ai(args: &RunArgs) -> SiaResult<()> {
    use crate::config::Config;
    use crate::llm::{run_pydantic_ai_agent, HttpChatTransport};

    let model = resolve_model(&args.model_name, args.provider.as_ref());

    // Resolve base_url + API key from the provider. The base_url defaults to the
    // OpenAI public endpoint when the provider declares none (matching Python's
    // OpenAIProvider, which targets an OpenAI-compatible endpoint).
    let provider = args.provider.as_ref();
    let base_url = provider
        .and_then(|p| p.base_url.clone())
        .unwrap_or_else(|| crate::llm::openai_api::DEFAULT_BASE_URL.to_string());

    let api_key = match provider {
        Some(p) => std::env::var(&p.api_key_env).map_err(|_| {
            SiaError::new(format!(
                "API key env var '{}' for provider '{}' is not set",
                p.api_key_env, p.provider_id
            ))
        })?,
        None => std::env::var("OPENAI_API_KEY").map_err(|_| {
            SiaError::new(
                "no provider supplied and OPENAI_API_KEY is not set for the pydantic-ai runner",
            )
        })?,
    };

    let max_turns: u32 = args.max_turns.trim().parse().map_err(|_| {
        SiaError::new(format!(
            "invalid max_turns '{}' for pydantic-ai runner",
            args.max_turns
        ))
    })?;

    let transport = HttpChatTransport::new(base_url, api_key);
    let config = Config::from_env();
    run_pydantic_ai_agent(
        &transport,
        &model,
        max_turns,
        &args.prompt,
        &args.agent_working_directory,
        &config,
    )
    .map(|_outcome| ())
}

/// Run a meta/feedback agent using PydanticAI. Without the `llm` feature the
/// native chat-completions tool-loop is unavailable; this returns an error
/// describing the integration boundary.
#[cfg(not(feature = "llm"))]
pub fn run_agent_pydantic_ai(args: &RunArgs) -> SiaResult<()> {
    let _model = resolve_model(&args.model_name, args.provider.as_ref());
    Err(SiaError::new(
        "the native `pydantic-ai`-style agent runner requires the `llm` cargo feature (issue #41). \
         The registry, dispatch, and model resolution are ported; build with `--features llm` to \
         enable the chat-completions tool-loop.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pydantic_ai_model_passthrough() {
        assert_eq!(resolve_model("openai:gpt-4o", None), "openai:gpt-4o");
        assert_eq!(
            resolve_model("anthropic:claude-sonnet-4-5", None),
            "anthropic:claude-sonnet-4-5"
        );
    }
}
