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

/// Run a meta/feedback agent using OpenHands. The SDK is unavailable in the Rust
/// port; this returns an error describing the integration boundary.
pub fn run_agent_openhands(args: &RunArgs) -> SiaResult<()> {
    let _model = resolve_model(&args.model_name, args.provider.as_ref());
    Err(SiaError::new(
        "the native `openhands`-style agent runner is not yet implemented (tracked in issue #40). \
         The registry, dispatch, and model resolution are ported; the multi-provider tool-loop is \
         pending, so `sia run`'s meta/feedback agents are not yet end-to-end.",
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
