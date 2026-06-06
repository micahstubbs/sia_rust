//! Environment-variable tests for Config::from_env and provider user-dir override.
//! Rust port of the env-based cases in test_config.py / test_config_injection.py /
//! test_providers.py. These mutate process-global env, so a mutex serializes them.

use std::sync::Mutex;

use sia::config::Config;
use sia::providers::load_provider;

static ENV_LOCK: Mutex<()> = Mutex::new(());

const VARS: &[&str] = &[
    "SIA_MAX_GENERATIONS",
    "SIA_AGENT_IMPL",
    "SIA_SANDBOX_MODE",
    "SIA_META_MODEL",
    "SIA_MAX_TURNS",
    "SIA_TASK_MODEL",
    "SIA_PROVIDERS_DIR",
];

fn clear_vars() {
    for v in VARS {
        std::env::remove_var(v);
    }
}

#[test]
fn test_from_env_reads_sia_vars() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_vars();
    std::env::set_var("SIA_MAX_GENERATIONS", "5");
    std::env::set_var("SIA_AGENT_IMPL", "openhands");
    std::env::set_var("SIA_SANDBOX_MODE", "docker");
    std::env::set_var("SIA_META_MODEL", "opus");

    let cfg = Config::from_env();
    assert_eq!(cfg.default_max_generations, 5);
    assert_eq!(cfg.default_agent_impl, "openhands");
    assert_eq!(cfg.sandbox_mode, "docker");
    assert_eq!(cfg.default_claude_meta_model, "opus");
    clear_vars();
}

#[test]
fn test_from_env_invalid_value_keeps_default() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_vars();
    std::env::set_var("SIA_MAX_GENERATIONS", "not-a-number");
    let cfg = Config::from_env();
    assert_eq!(cfg.default_max_generations, 3);
    clear_vars();
}

#[test]
fn test_from_env_no_vars_returns_defaults() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_vars();
    let cfg = Config::from_env();
    assert_eq!(cfg.default_max_generations, 3);
    assert_eq!(cfg.default_task_model, "claude-haiku-4-5-20251001");
}

#[test]
fn test_from_env_override_reaches_instance() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_vars();
    std::env::set_var("SIA_MAX_TURNS", "99");
    assert_eq!(Config::from_env().default_max_turns, 99);
    clear_vars();
}

#[test]
fn test_user_dir_overrides_bundled() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_vars();
    let d = tempfile::tempdir().unwrap();
    let providers_dir = d.path().join("providers");
    std::fs::create_dir(&providers_dir).unwrap();
    std::fs::write(
        providers_dir.join("nebius.json"),
        serde_json::json!({
            "provider_id": "nebius", "name": "nebius", "client_kind": "openai",
            "base_url": "https://override/v1", "api_key_env": "NEBIUS_API_KEY"
        })
        .to_string(),
    )
    .unwrap();
    std::env::set_var("SIA_PROVIDERS_DIR", &providers_dir);
    assert_eq!(
        load_provider("nebius").unwrap().base_url.as_deref(),
        Some("https://override/v1")
    );
    clear_vars();
}
