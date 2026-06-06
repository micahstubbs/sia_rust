//! SIA: Self-Improving AI framework — Rust port of the Python `sia` package.
//!
//! Module layout mirrors `sia/`: see each module's docs for the source it ports.

pub mod agent_impls;
pub mod agent_reference;
pub mod api_keys;
pub mod assets;
pub mod cli;
pub mod config;
pub mod config_files;
pub mod context_manager;
pub mod error;
pub mod io_utils;
pub mod layout;
#[cfg(feature = "llm")]
pub mod llm;
pub mod orchestrator;
pub mod profiles;
pub mod prompts;
pub mod providers;
pub mod pyfmt;
pub mod pyjson;
pub mod results;
pub mod run;
pub mod run_setup;
pub mod sandbox;
pub mod task_files;
pub mod verifier;
pub mod web;

pub use task_files::TaskFiles;

pub use config::Config;
pub use error::{SiaError, SiaResult};

/// Package version (mirrors `sia.__version__`, sourced from Cargo).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
