//! LLM provider registry — JSON-defined endpoints/credentials. Port of `sia/providers.py`.

use serde::Serialize;

use crate::config_files::{available_names, read_config_text};
use crate::error::{SiaError, SiaResult};

pub const ENV_VAR: &str = "SIA_PROVIDERS_DIR";
pub const SUBDIR: &str = "providers";

/// SDK family the generated/meta agent should use to reach the model.
pub const VALID_CLIENT_KINDS: &[&str] = &["anthropic", "openai", "google"];

/// How to reach a model provider's API.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provider {
    pub provider_id: String,
    pub name: String,
    pub client_kind: String,
    pub base_url: Option<String>,
    pub api_key_env: String,
}

/// Names of all providers discoverable in the bundled + user directories.
pub fn available_providers() -> Vec<String> {
    available_names(ENV_VAR, SUBDIR)
}

/// Load and validate a provider by bundled/user name or by path to a `.json` file.
pub fn load_provider(name_or_path: &str) -> SiaResult<Provider> {
    let (text, source) = read_config_text(name_or_path, ENV_VAR, SUBDIR, "provider")?;
    let data: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| SiaError::new(format!("Invalid provider JSON at {source}: {e}")))?;

    let required = ["provider_id", "name", "client_kind", "api_key_env"];
    let mut missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|k| data.get(*k).is_none())
        .collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(SiaError::new(format!(
            "Provider at {source} is missing required keys: {}",
            missing.join(", ")
        )));
    }

    let client_kind = data["client_kind"].as_str().unwrap_or("").to_string();
    if !VALID_CLIENT_KINDS.contains(&client_kind.as_str()) {
        return Err(SiaError::new(format!(
            "Provider at {source} has invalid client_kind '{client_kind}'. Expected one of: {}.",
            VALID_CLIENT_KINDS.join(", ")
        )));
    }

    let base_url = match data.get("base_url") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        _ => None,
    };

    Ok(Provider {
        provider_id: data["provider_id"].as_str().unwrap_or("").to_string(),
        name: data["name"].as_str().unwrap_or("").to_string(),
        client_kind,
        base_url,
        api_key_env: data["api_key_env"].as_str().unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundled_providers_present() {
        let names: std::collections::HashSet<String> = available_providers().into_iter().collect();
        for expected in ["anthropic", "gemini", "openai", "together", "nebius"] {
            assert!(names.contains(expected), "missing provider {expected}");
        }
    }

    #[test]
    fn test_load_anthropic_provider() {
        let p = load_provider("anthropic").unwrap();
        assert_eq!(p.provider_id, "anthropic");
        assert_eq!(p.client_kind, "anthropic");
        assert_eq!(p.base_url, None);
        assert_eq!(p.api_key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn test_load_nebius_provider() {
        let p = load_provider("nebius").unwrap();
        assert_eq!(p.client_kind, "openai");
        assert_eq!(
            p.base_url.as_deref(),
            Some("https://api.tokenfactory.us-central1.nebius.com/v1/")
        );
        assert_eq!(p.api_key_env, "NEBIUS_API_KEY");
    }

    #[test]
    fn test_unknown_provider_name_raises() {
        assert!(load_provider("does-not-exist").is_err());
    }

    #[test]
    fn test_load_provider_from_path() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("custom.json");
        std::fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "provider_id": "custom", "name": "Custom", "client_kind": "openai",
                "base_url": "https://x/v1", "api_key_env": "X_KEY"
            }))
            .unwrap(),
        )
        .unwrap();
        let p = load_provider(path.to_str().unwrap()).unwrap();
        assert_eq!(p.provider_id, "custom");
        assert_eq!(p.base_url.as_deref(), Some("https://x/v1"));
    }

    #[test]
    fn test_missing_path_raises() {
        assert!(load_provider("/no/such/provider.json").is_err());
    }

    #[test]
    fn test_invalid_client_kind_raises() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("bad.json");
        std::fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "provider_id": "bad", "name": "Bad", "client_kind": "mystery", "api_key_env": "K"
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(load_provider(path.to_str().unwrap()).is_err());
    }
}
