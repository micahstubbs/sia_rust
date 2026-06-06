//! SIA: Self-Improving AI framework — Rust port of the Python `sia` package.
//!
//! Module layout mirrors `sia/`: see each module's docs for the source it ports.

pub mod agent_impls;
pub mod agent_reference;
pub mod api_keys;
pub mod assets;
pub mod config;
pub mod config_files;
pub mod error;
pub mod io_utils;
pub mod layout;
pub mod profiles;
pub mod providers;
pub mod results;

pub use config::Config;
pub use error::{SiaError, SiaResult};

/// Package version (mirrors `sia.__version__`, sourced from Cargo).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
