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

/// Run a meta/feedback agent using PydanticAI. The SDK is unavailable in the Rust
/// port; this returns an error describing the integration boundary.
pub fn run_agent_pydantic_ai(args: &RunArgs) -> SiaResult<()> {
    let _model = resolve_model(&args.model_name, args.provider.as_ref());
    Err(SiaError::new(
        "the native `pydantic-ai`-style agent runner is not yet implemented (tracked in issue #41). \
         The registry, dispatch, and model resolution are ported; the rig/dspy-rs tool-agent is \
         pending, so `sia run`'s meta/feedback agents are not yet end-to-end.",
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
