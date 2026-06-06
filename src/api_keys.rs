//! Model-name -> provider API key resolution. Port of `sia/api_keys.py`.

/// Return the provider-specific API key for `model_name` from the environment.
///
/// Precedence (matches the original `run_agent_openhands` logic):
///   - claude / anthropic -> ANTHROPIC_API_KEY
///   - gemini / google    -> GOOGLE_API_KEY or GEMINI_API_KEY
///   - gpt / openai       -> OPENAI_API_KEY
///   - anything else      -> LLM_API_KEY
///
/// Returns `None` when the matched variable is unset (the caller may then fall back).
pub fn resolve_api_key(model_name: &str) -> Option<String> {
    let name = model_name.to_lowercase();
    if name.contains("claude") || name.contains("anthropic") {
        return std::env::var("ANTHROPIC_API_KEY").ok();
    }
    if name.contains("gemini") || name.contains("google") {
        return std::env::var("GOOGLE_API_KEY")
            .ok()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok());
    }
    if name.contains("gpt") || name.contains("openai") {
        return std::env::var("OPENAI_API_KEY").ok();
    }
    std::env::var("LLM_API_KEY").ok()
}
