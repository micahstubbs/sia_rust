//! Agent profiles — JSON-defined configuration for one agent role. Port of `sia/profiles.py`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::agent_impls::available_agent_impls;
use crate::agent_reference::{parse_agent_reference, AgentReference};
use crate::config_files::{available_names, read_config_text};
use crate::error::{SiaError, SiaResult};
use crate::providers::{load_provider, Provider};

pub const ENV_VAR: &str = "SIA_PROFILES_DIR";
pub const SUBDIR: &str = "profiles";

/// Full configuration for the meta/feedback agent role.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetaAgentProfile {
    pub profile_id: String,
    pub name: String,
    pub agent_impl: String,
    pub model: String,
    pub provider: Provider,
}

/// Full configuration for the target agent role (generated, never run by SIA).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetAgentProfile {
    pub profile_id: String,
    pub name: String,
    pub model: String,
    pub provider: Provider,
    pub agent_reference: AgentReference,
}

/// Names of all profiles discoverable in the bundled + user directories.
pub fn available_profiles() -> Vec<String> {
    available_names(ENV_VAR, SUBDIR)
}

fn load_json(name_or_path: &str) -> SiaResult<(serde_json::Value, String)> {
    let (text, source) = read_config_text(name_or_path, ENV_VAR, SUBDIR, "profile")?;
    let data: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| SiaError::new(format!("Invalid profile JSON at {source}: {e}")))?;
    Ok((data, source))
}

fn require(data: &serde_json::Value, keys: &[&str], source: &str) -> SiaResult<()> {
    let mut missing: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|k| data.get(*k).is_none())
        .collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(SiaError::new(format!(
            "Profile at {source} is missing required keys: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

/// Directory a profile file lives in (for resolving a relative agent_reference).
fn profile_base_dir(source: &str) -> Option<PathBuf> {
    if source.starts_with("<bundled>") {
        None
    } else {
        Path::new(source).parent().map(|p| p.to_path_buf())
    }
}

fn str_field(data: &serde_json::Value, key: &str) -> String {
    data.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Load and validate a meta-agent profile by bundled/user name or path to a `.json` file.
pub fn load_meta_agent_profile(name_or_path: &str) -> SiaResult<MetaAgentProfile> {
    let (data, source) = load_json(name_or_path)?;
    require(
        &data,
        &["profile_id", "name", "agent_impl", "model", "provider_id"],
        &source,
    )?;

    let provider = load_provider(&str_field(&data, "provider_id"))?;
    let profile = MetaAgentProfile {
        profile_id: str_field(&data, "profile_id"),
        name: str_field(&data, "name"),
        agent_impl: str_field(&data, "agent_impl"),
        model: str_field(&data, "model"),
        provider,
    };
    validate_meta(&profile, &source)?;
    Ok(profile)
}

/// Load and validate a target-agent profile by bundled/user name or path to a `.json` file.
pub fn load_target_agent_profile(name_or_path: &str) -> SiaResult<TargetAgentProfile> {
    let (data, source) = load_json(name_or_path)?;
    require(
        &data,
        &["profile_id", "name", "model", "provider_id"],
        &source,
    )?;

    let base_dir = profile_base_dir(&source);
    let agent_reference = parse_agent_reference(data.get("agent_reference"), base_dir.as_deref())?;
    Ok(TargetAgentProfile {
        profile_id: str_field(&data, "profile_id"),
        name: str_field(&data, "name"),
        model: str_field(&data, "model"),
        provider: load_provider(&str_field(&data, "provider_id"))?,
        agent_reference,
    })
}

/// Reject incoherent agent_impl/provider combinations for the meta agent.
fn validate_meta(profile: &MetaAgentProfile, source: &str) -> SiaResult<()> {
    let valid = available_agent_impls();
    if !valid.contains(&profile.agent_impl) {
        return Err(SiaError::new(format!(
            "Profile at {source} has invalid agent_impl '{}'. Expected one of: {}.",
            profile.agent_impl,
            valid.join(", ")
        )));
    }
    if profile.agent_impl == "claude" && profile.provider.client_kind != "anthropic" {
        return Err(SiaError::new(format!(
            "Profile at {source} pairs agent_impl 'claude' with provider '{}' (client_kind={}). \
             The claude agent impl requires an anthropic provider; use the openhands or \
             pydantic-ai agent impl for other providers.",
            profile.provider.name, profile.provider.client_kind
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_profile(dir: &Path, data: serde_json::Value) -> String {
        let path = dir.join("p.json");
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn test_bundled_profiles_present() {
        let names: std::collections::HashSet<String> = available_profiles().into_iter().collect();
        for expected in ["default-meta", "default-target", "kimi-nebius-target"] {
            assert!(names.contains(expected), "missing profile {expected}");
        }
    }

    #[test]
    fn test_default_meta_profile() {
        let p = load_meta_agent_profile("default-meta").unwrap();
        assert_eq!(p.profile_id, "default-meta");
        assert_eq!(p.agent_impl, "claude");
        assert_eq!(p.model, "haiku");
        assert_eq!(p.provider.provider_id, "anthropic");
    }

    #[test]
    fn test_default_target_profile_uses_default_reference() {
        let p = load_target_agent_profile("default-target").unwrap();
        assert_eq!(p.agent_reference.kind, "default");
        assert_eq!(p.model, "claude-haiku-4-5-20251001");
        assert_eq!(p.provider.client_kind, "anthropic");
    }

    #[test]
    fn test_kimi_nebius_target_profile_resolves_provider() {
        let p = load_target_agent_profile("kimi-nebius-target").unwrap();
        assert_eq!(p.agent_reference.kind, "default");
        assert_eq!(p.model, "moonshotai/Kimi-K2.6");
        assert_eq!(p.provider.provider_id, "nebius");
        assert!(p
            .provider
            .base_url
            .as_deref()
            .unwrap()
            .ends_with("nebius.com/v1/"));
    }

    #[test]
    fn test_deepseek_nebius_target_profile_resolves_provider() {
        let p = load_target_agent_profile("deepseek-nebius-target").unwrap();
        assert_eq!(p.agent_reference.kind, "default");
        assert_eq!(p.model, "deepseek-ai/DeepSeek-R1-0528");
        assert_eq!(p.provider.provider_id, "nebius");
        assert_eq!(p.provider.client_kind, "openai");
        assert!(p
            .provider
            .base_url
            .as_deref()
            .unwrap()
            .ends_with("nebius.com/v1/"));
    }

    #[test]
    fn test_gemini_target_profile_resolves_provider() {
        let p = load_target_agent_profile("gemini-target").unwrap();
        assert_eq!(p.agent_reference.kind, "default");
        assert_eq!(p.model, "gemini-2.5-flash");
        assert_eq!(p.provider.provider_id, "gemini");
        assert_eq!(p.provider.client_kind, "google");
    }

    #[test]
    fn test_unknown_profile_raises() {
        assert!(load_meta_agent_profile("nope").is_err());
    }

    #[test]
    fn test_invalid_agent_impl_raises() {
        let d = tempfile::tempdir().unwrap();
        let path = write_profile(
            d.path(),
            serde_json::json!({"profile_id": "p", "name": "p", "agent_impl": "bogus", "model": "m", "provider_id": "anthropic"}),
        );
        assert!(load_meta_agent_profile(&path).is_err());
    }

    #[test]
    fn test_claude_agent_impl_requires_anthropic_provider() {
        let d = tempfile::tempdir().unwrap();
        let path = write_profile(
            d.path(),
            serde_json::json!({"profile_id": "p", "name": "p", "agent_impl": "claude", "model": "m", "provider_id": "nebius"}),
        );
        assert!(load_meta_agent_profile(&path).is_err());
    }

    #[test]
    fn test_openhands_agent_impl_allows_non_anthropic_provider() {
        let d = tempfile::tempdir().unwrap();
        let path = write_profile(
            d.path(),
            serde_json::json!({"profile_id": "p", "name": "p", "agent_impl": "openhands", "model": "m", "provider_id": "nebius"}),
        );
        let profile = load_meta_agent_profile(&path).unwrap();
        assert_eq!(profile.agent_impl, "openhands");
        assert_eq!(profile.provider.provider_id, "nebius");
    }

    #[test]
    fn test_target_profile_defaults_reference_when_omitted() {
        let d = tempfile::tempdir().unwrap();
        let path = write_profile(
            d.path(),
            serde_json::json!({"profile_id": "p", "name": "p", "model": "m", "provider_id": "anthropic"}),
        );
        let profile = load_target_agent_profile(&path).unwrap();
        assert_eq!(profile.agent_reference.kind, "default");
    }

    #[test]
    fn test_target_profile_file_reference() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("my_agent.py"), "print('hi')").unwrap();
        let path = write_profile(
            d.path(),
            serde_json::json!({
                "profile_id": "p", "name": "p", "model": "m", "provider_id": "anthropic",
                "agent_reference": {"source": "./my_agent.py"}
            }),
        );
        let profile = load_target_agent_profile(&path).unwrap();
        assert_eq!(profile.agent_reference.kind, "file");
        assert_eq!(
            profile.agent_reference.source.unwrap().file_name().unwrap(),
            "my_agent.py"
        );
    }
}
