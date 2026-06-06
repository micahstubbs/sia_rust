//! Centralized configuration for the SIA framework.
//!
//! Port of `sia/config.py`. `Config` is the single source of truth for all
//! tunable defaults; `Config::from_env` layers `SIA_*` environment overrides on
//! top of the compiled-in defaults (silently keeping the default when a value
//! fails to parse, matching the Python `contextlib.suppress(ValueError, TypeError)`).

/// Single source of truth for all SIA configuration defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    // Agent profile defaults (JSON profiles selected on the CLI).
    pub default_meta_agent_profile: String,
    pub default_target_agent_profile: String,

    // Model defaults.
    pub default_claude_meta_model: String,
    pub default_openhands_meta_model: String,
    pub default_task_model: String,

    // Generation defaults.
    pub default_max_generations: i64,
    pub default_run_id: i64,

    // Agent execution.
    pub default_max_turns: i64,
    pub context_summary_max_turns: i64,
    pub default_agent_impl: String,

    // Truncation limits.
    pub agent_code_preview_limit: usize,
    pub trajectory_preview_limit: usize,
    pub tool_result_preview_limit: usize,
    pub insight_preview_limit: usize,

    // Timeouts (seconds).
    pub shell_timeout: u64,
    pub eval_timeout: u64,

    // Sandbox settings.
    pub sandbox_mode: String,
    pub docker_image: String,
    pub docker_memory_limit: String,
    pub docker_cpu_limit: f64,
    pub docker_timeout: u64,

    // File size limits (bytes).
    pub max_context_file_size: u64,
    pub max_execution_log_size: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_meta_agent_profile: "default-meta".to_string(),
            default_target_agent_profile: "default-target".to_string(),

            default_claude_meta_model: "haiku".to_string(),
            default_openhands_meta_model: "gemini/gemini-3.1-pro-preview".to_string(),
            default_task_model: "claude-haiku-4-5-20251001".to_string(),

            default_max_generations: 3,
            default_run_id: 1,

            default_max_turns: 20,
            context_summary_max_turns: 5,
            default_agent_impl: "claude".to_string(),

            agent_code_preview_limit: 3000,
            trajectory_preview_limit: 1000,
            tool_result_preview_limit: 500,
            insight_preview_limit: 200,

            shell_timeout: 30,
            eval_timeout: 600,

            sandbox_mode: "none".to_string(),
            docker_image: "python:3.11-slim".to_string(),
            docker_memory_limit: "2g".to_string(),
            docker_cpu_limit: 2.0,
            docker_timeout: 3600,

            max_context_file_size: 10_000_000,
            max_execution_log_size: 50_000_000,
        }
    }
}

impl Config {
    /// The baseline virtual-environment packages installed for every run.
    pub const VENV_PACKAGES: &'static [&'static str] = &[
        "anthropic",
        "openai",
        "python-dotenv",
        "google-genai",
        "tqdm",
        "pydantic",
        "scikit-learn",
        "pandas",
        "numpy",
    ];

    /// Create a `Config` with overrides from `SIA_*` environment variables.
    ///
    /// The recognized set mirrors the Python `env_map` **exactly** (8 vars), for
    /// parity: honoring more `SIA_*` keys here than `sia/config.py` does would make
    /// the Rust port diverge from the reference. An unparseable value leaves the
    /// default in place (matching Python's `contextlib.suppress(ValueError, TypeError)`).
    /// Extending the override surface is a separate enhancement that must land in the
    /// Python source first so both stay in lockstep (`tests/config_env.rs`).
    pub fn from_env() -> Config {
        let mut cfg = Config::default();
        if let Some(v) = env_str("SIA_META_AGENT_PROFILE") {
            cfg.default_meta_agent_profile = v;
        }
        if let Some(v) = env_str("SIA_TARGET_AGENT_PROFILE") {
            cfg.default_target_agent_profile = v;
        }
        if let Some(v) = env_str("SIA_META_MODEL") {
            cfg.default_claude_meta_model = v;
        }
        if let Some(v) = env_str("SIA_TASK_MODEL") {
            cfg.default_task_model = v;
        }
        if let Some(v) = env_str("SIA_MAX_GENERATIONS") {
            if let Ok(n) = v.parse::<i64>() {
                cfg.default_max_generations = n;
            }
        }
        if let Some(v) = env_str("SIA_AGENT_IMPL") {
            cfg.default_agent_impl = v;
        }
        if let Some(v) = env_str("SIA_MAX_TURNS") {
            if let Ok(n) = v.parse::<i64>() {
                cfg.default_max_turns = n;
            }
        }
        if let Some(v) = env_str("SIA_SANDBOX_MODE") {
            cfg.sandbox_mode = v;
        }
        cfg
    }
}

fn env_str(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = Config::default();
        assert_eq!(cfg.default_max_generations, 3);
        assert_eq!(cfg.default_agent_impl, "claude");
        assert_eq!(cfg.sandbox_mode, "none");
        assert_eq!(cfg.default_max_turns, 20);
        assert_eq!(cfg.docker_memory_limit, "2g");
        assert_eq!(cfg.max_context_file_size, 10_000_000);
    }

    #[test]
    fn test_default_task_model() {
        assert_eq!(
            Config::default().default_task_model,
            "claude-haiku-4-5-20251001"
        );
    }
}
