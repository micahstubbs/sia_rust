//! Provider/profile -> client mapping layer (issue #49).
//!
//! A single source of truth that maps a [`Provider`] config (plus API-key env
//! resolution) to the correctly constructed LLM transport, so the runners stop
//! hand-rolling transport construction. The mapping understands the
//! `client_kind` distinction (`anthropic` | `openai` | `google`), is extensible,
//! and produces helpful user-facing error messages.
//!
//! The whole module is gated behind the non-default `llm` cargo feature.
//!
//! # Base-URL defaults
//!
//! When a provider declares no `base_url`, [`base_url_for`] supplies a sensible
//! per-`client_kind` default:
//!
//! - `anthropic` -> `https://api.anthropic.com`
//! - `openai`    -> `https://api.openai.com/v1`
//! - `google`    -> `https://generativelanguage.googleapis.com/v1beta/openai`
//!   (Google's OpenAI-compatible endpoint, driven through [`HttpChatTransport`])

use crate::error::{SiaError, SiaResult};
use crate::llm::{HttpChatTransport, HttpMessagesTransport};
use crate::providers::{Provider, VALID_CLIENT_KINDS};

/// Default Anthropic base URL (mirrors `anthropic_api::DEFAULT_BASE_URL`).
const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Default OpenAI base URL (mirrors `openai_api::DEFAULT_BASE_URL`).
const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default Google OpenAI-compatible base URL.
const GOOGLE_DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

/// A constructed client/transport for a provider, dispatched by `client_kind`.
#[derive(Debug, Clone)]
pub enum AgentClient {
    /// Anthropic Messages API transport (`client_kind == "anthropic"`).
    Anthropic(HttpMessagesTransport),
    /// OpenAI-compatible Chat Completions transport (`openai` | `google`).
    Chat(HttpChatTransport),
}

/// Read the provider's API key from its `api_key_env` environment variable.
///
/// On a missing/unset variable, returns a [`SiaError`] naming both the env var
/// and the provider, e.g. `"provider 'nebius' requires the API key in
/// environment variable 'NEBIUS_API_KEY', which is not set"`.
pub fn api_key_for(provider: &Provider) -> SiaResult<String> {
    std::env::var(&provider.api_key_env).map_err(|_| {
        SiaError::new(format!(
            "provider '{}' requires the API key in environment variable '{}', which is not set",
            provider.provider_id, provider.api_key_env
        ))
    })
}

/// Resolve the base URL for a provider: its explicit `base_url` if set, else the
/// per-`client_kind` default (see the module docs). Unknown kinds fall back to
/// the OpenAI default, since the OpenAI-compatible transport is the catch-all.
pub fn base_url_for(provider: &Provider) -> String {
    if let Some(base) = &provider.base_url {
        return base.clone();
    }
    match provider.client_kind.as_str() {
        "anthropic" => ANTHROPIC_DEFAULT_BASE_URL.to_string(),
        "google" => GOOGLE_DEFAULT_BASE_URL.to_string(),
        // "openai" and any other (OpenAI-compatible) kind.
        _ => OPENAI_DEFAULT_BASE_URL.to_string(),
    }
}

/// Defensive validation that `provider.client_kind` is one we map. `load_provider`
/// already enforces this, but the mapping keeps its own guard for direct callers.
fn validate_client_kind(provider: &Provider) -> SiaResult<()> {
    if VALID_CLIENT_KINDS.contains(&provider.client_kind.as_str()) {
        Ok(())
    } else {
        Err(SiaError::new(format!(
            "provider '{}' has invalid client_kind '{}'. Expected one of: {}.",
            provider.provider_id,
            provider.client_kind,
            VALID_CLIENT_KINDS.join(", ")
        )))
    }
}

/// Map a provider to its constructed [`AgentClient`].
///
/// - `anthropic` -> [`AgentClient::Anthropic`] over [`HttpMessagesTransport`]
/// - `openai` | `google` -> [`AgentClient::Chat`] over [`HttpChatTransport`]
///
/// Validates the `client_kind` and resolves the API key (helpful error on a
/// missing env var) and base URL first.
pub fn client_for(provider: &Provider) -> SiaResult<AgentClient> {
    validate_client_kind(provider)?;
    let api_key = api_key_for(provider)?;
    let base_url = base_url_for(provider);
    match provider.client_kind.as_str() {
        "anthropic" => Ok(AgentClient::Anthropic(
            HttpMessagesTransport::with_base_url(api_key, base_url),
        )),
        // "openai" | "google" (validated above).
        _ => Ok(AgentClient::Chat(HttpChatTransport::new(base_url, api_key))),
    }
}

/// Build an Anthropic Messages transport for the `claude` runner.
///
/// The `claude` runner historically authenticates via `ANTHROPIC_API_KEY` and
/// ignores the provider (matching the Python impl, which delegates auth to the
/// Claude Agent SDK):
///
/// - `None`: read `ANTHROPIC_API_KEY`, honoring `ANTHROPIC_BASE_URL` as an
///   override (else the default Anthropic base URL).
/// - `Some(provider)`: use the provider's `api_key_env` + resolved base URL.
pub fn messages_transport_for(provider: Option<&Provider>) -> SiaResult<HttpMessagesTransport> {
    match provider {
        None => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| SiaError::new("ANTHROPIC_API_KEY is not set"))?;
            match std::env::var("ANTHROPIC_BASE_URL") {
                Ok(base) if !base.trim().is_empty() => {
                    Ok(HttpMessagesTransport::with_base_url(api_key, base))
                }
                _ => Ok(HttpMessagesTransport::new(api_key)),
            }
        }
        Some(provider) => {
            let api_key = api_key_for(provider)?;
            let base_url = base_url_for(provider);
            Ok(HttpMessagesTransport::with_base_url(api_key, base_url))
        }
    }
}

/// Build an OpenAI-compatible Chat Completions transport for a provider
/// (`openhands` / `pydantic-ai` runners), resolving its base URL + API key.
pub fn chat_transport_for(provider: &Provider) -> SiaResult<HttpChatTransport> {
    let api_key = api_key_for(provider)?;
    let base_url = base_url_for(provider);
    Ok(HttpChatTransport::new(base_url, api_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::load_provider;

    /// Run `f` with `var` set to `value`, snapshotting + restoring the prior
    /// value so the test is deterministic regardless of host env.
    fn with_env_var<T>(var: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let saved = std::env::var(var).ok();
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        let out = f();
        match saved {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        out
    }

    #[test]
    fn api_key_for_returns_value_when_set() {
        let anthropic = load_provider("anthropic").unwrap();
        let key = with_env_var("ANTHROPIC_API_KEY", Some("sk-test-123"), || {
            api_key_for(&anthropic)
        })
        .unwrap();
        assert_eq!(key, "sk-test-123");
    }

    #[test]
    fn api_key_for_errors_when_missing() {
        let nebius = load_provider("nebius").unwrap();
        let err = with_env_var("NEBIUS_API_KEY", None, || api_key_for(&nebius)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("NEBIUS_API_KEY"), "msg was: {msg}");
        assert!(msg.contains("nebius"), "msg was: {msg}");
    }

    #[test]
    fn base_url_for_uses_provider_base_url_when_set() {
        let nebius = load_provider("nebius").unwrap();
        assert_eq!(
            base_url_for(&nebius),
            "https://api.tokenfactory.us-central1.nebius.com/v1/"
        );
    }

    #[test]
    fn base_url_for_anthropic_default() {
        let anthropic = load_provider("anthropic").unwrap();
        assert_eq!(anthropic.base_url, None);
        assert_eq!(base_url_for(&anthropic), "https://api.anthropic.com");
    }

    #[test]
    fn base_url_for_openai_default() {
        let p = Provider {
            provider_id: "p".to_string(),
            name: "P".to_string(),
            client_kind: "openai".to_string(),
            base_url: None,
            api_key_env: "P_KEY".to_string(),
        };
        assert_eq!(base_url_for(&p), "https://api.openai.com/v1");
    }

    #[test]
    fn base_url_for_google_default() {
        let p = Provider {
            provider_id: "g".to_string(),
            name: "G".to_string(),
            client_kind: "google".to_string(),
            base_url: None,
            api_key_env: "G_KEY".to_string(),
        };
        assert_eq!(
            base_url_for(&p),
            "https://generativelanguage.googleapis.com/v1beta/openai"
        );
    }

    #[test]
    fn client_for_anthropic_returns_anthropic_variant() {
        let anthropic = load_provider("anthropic").unwrap();
        let client = with_env_var("ANTHROPIC_API_KEY", Some("sk-test"), || {
            client_for(&anthropic)
        })
        .unwrap();
        assert!(
            matches!(client, AgentClient::Anthropic(_)),
            "expected Anthropic variant"
        );
    }

    #[test]
    fn client_for_openai_returns_chat_variant() {
        let nebius = load_provider("nebius").unwrap(); // client_kind == "openai"
        let client =
            with_env_var("NEBIUS_API_KEY", Some("nk-test"), || client_for(&nebius)).unwrap();
        assert!(
            matches!(client, AgentClient::Chat(_)),
            "expected Chat variant"
        );
    }

    #[test]
    fn client_for_google_returns_chat_variant() {
        let p = Provider {
            provider_id: "g".to_string(),
            name: "G".to_string(),
            client_kind: "google".to_string(),
            base_url: None,
            api_key_env: "G_KEY_FOR_TEST".to_string(),
        };
        let client = with_env_var("G_KEY_FOR_TEST", Some("gk-test"), || client_for(&p)).unwrap();
        assert!(
            matches!(client, AgentClient::Chat(_)),
            "expected Chat variant"
        );
    }

    #[test]
    fn client_for_errors_on_invalid_client_kind() {
        let p = Provider {
            provider_id: "bad".to_string(),
            name: "Bad".to_string(),
            client_kind: "mystery".to_string(),
            base_url: None,
            api_key_env: "K".to_string(),
        };
        let err = client_for(&p).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mystery"), "msg was: {msg}");
        assert!(msg.contains("anthropic"), "msg was: {msg}");
    }

    #[test]
    fn messages_transport_for_none_reads_anthropic_api_key() {
        let result = with_env_var("ANTHROPIC_API_KEY", Some("sk-anthropic"), || {
            // Ensure no base-url override is in play for this assertion.
            with_env_var("ANTHROPIC_BASE_URL", None, || messages_transport_for(None))
        });
        assert!(result.is_ok());
    }

    #[test]
    fn messages_transport_for_none_errors_when_missing() {
        let err =
            with_env_var("ANTHROPIC_API_KEY", None, || messages_transport_for(None)).unwrap_err();
        assert!(err.to_string().contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn messages_transport_for_some_uses_provider_key() {
        let anthropic = load_provider("anthropic").unwrap();
        let result = with_env_var("ANTHROPIC_API_KEY", Some("sk-prov"), || {
            messages_transport_for(Some(&anthropic))
        });
        assert!(result.is_ok());
    }

    #[test]
    fn chat_transport_for_builds_from_provider() {
        let nebius = load_provider("nebius").unwrap();
        let result = with_env_var("NEBIUS_API_KEY", Some("nk-test"), || {
            chat_transport_for(&nebius)
        });
        assert!(result.is_ok());
    }

    #[test]
    fn chat_transport_for_errors_when_key_missing() {
        let nebius = load_provider("nebius").unwrap();
        let err = with_env_var("NEBIUS_API_KEY", None, || chat_transport_for(&nebius)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("NEBIUS_API_KEY"), "msg was: {msg}");
        assert!(msg.contains("nebius"), "msg was: {msg}");
    }
}
