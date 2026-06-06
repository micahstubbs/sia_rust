//! OpenHands agent impl. Port of `sia/agent_impls/openhands.py`.
//!
//! The OpenHands SDK has no Rust equivalent; `run_agent_openhands` is the
//! integration boundary. The deterministic `resolve_model` logic (litellm model
//! prefixing) is fully ported and tested.

use crate::agent_impls::base::RunArgs;
use crate::error::SiaError;
use crate::error::SiaResult;
use crate::providers::Provider;

/// Resolve the litellm model spec OpenHands' LLM should use.
///
/// For an OpenAI-compatible endpoint (`client_kind == "openai"` with a `base_url`),
/// the model must carry an explicit `openai/` prefix so litellm routes to that
/// `base_url`. Already-prefixed and native (anthropic) specs pass through unchanged.
pub fn resolve_model(model_name: &str, provider: Option<&Provider>) -> String {
    match provider {
        None => model_name.to_string(),
        Some(p) => {
            if p.client_kind == "openai"
                && p.base_url.is_some()
                && !model_name.starts_with("openai/")
            {
                format!("openai/{model_name}")
            } else {
                model_name.to_string()
            }
        }
    }
}

/// Run a meta/feedback agent using a native OpenHands-style multi-provider loop.
///
/// Routes a model spec to an OpenAI-compatible chat-completions endpoint via the
/// provider's `base_url` + `resolve_model` prefixing (the role litellm played in
/// the Python impl), exposes terminal + file-editor tools, and persists
/// OpenHands-style events under
/// `agent_working_directory/openhands_trajectory/<session>/events/`.
#[cfg(feature = "llm")]
pub fn run_agent_openhands(args: &RunArgs) -> SiaResult<()> {
    use crate::config::Config;
    use crate::llm::{provider_mapping, run_openhands_agent, HttpChatTransport};

    let model = resolve_model(&args.model_name, args.provider.as_ref());

    // Resolve base_url + API key. With a provider, build the chat transport via the
    // mapping (single source of truth). Without one, the historical fallback applies:
    // the OpenAI public endpoint + OPENAI_API_KEY, with its own missing-key error.
    let transport = match args.provider.as_ref() {
        Some(provider) => provider_mapping::chat_transport_for(provider)?,
        None => {
            let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
                SiaError::new(
                    "no provider supplied and OPENAI_API_KEY is not set for the openhands runner",
                )
            })?;
            HttpChatTransport::new(crate::llm::openai_api::DEFAULT_BASE_URL, api_key)
        }
    };

    let max_turns: u32 = args.max_turns.trim().parse().map_err(|_| {
        SiaError::new(format!(
            "invalid max_turns '{}' for openhands runner",
            args.max_turns
        ))
    })?;

    // A stable, single-session id per run (the Python SDK used one conversation).
    let session = "session_0";

    let config = Config::from_env();
    run_openhands_agent(
        &transport,
        &model,
        max_turns,
        &args.prompt,
        &args.agent_working_directory,
        session,
        &config,
    )
    .map(|_summary| ())
}

/// Run a meta/feedback agent using OpenHands. Without the `llm` feature the
/// native multi-provider tool-loop is unavailable; this returns an error
/// describing the integration boundary.
#[cfg(not(feature = "llm"))]
pub fn run_agent_openhands(args: &RunArgs) -> SiaResult<()> {
    let _model = resolve_model(&args.model_name, args.provider.as_ref());
    Err(SiaError::new(
        "the native `openhands`-style agent runner requires the `llm` cargo feature (issue #40). \
         The registry, dispatch, and model resolution are ported; build with `--features llm` to \
         enable the multi-provider tool-loop.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::load_provider;

    #[test]
    fn test_openhands_model_gets_openai_prefix_for_compatible_provider() {
        let nebius = load_provider("nebius").unwrap();
        assert_eq!(
            resolve_model("moonshotai/Kimi-K2.6", Some(&nebius)),
            "openai/moonshotai/Kimi-K2.6"
        );
        assert_eq!(
            resolve_model("openai/gpt-4o", Some(&nebius)),
            "openai/gpt-4o"
        );
    }

    #[test]
    fn test_openhands_model_passthrough_without_compatible_provider() {
        assert_eq!(
            resolve_model("claude-sonnet-4-5", None),
            "claude-sonnet-4-5"
        );
        let anthropic = load_provider("anthropic").unwrap();
        assert_eq!(
            resolve_model("claude-sonnet-4-5", Some(&anthropic)),
            "claude-sonnet-4-5"
        );
    }
}
