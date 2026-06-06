//! Top-level `run` / `web` dispatch for the `sia` binary.
//!
//! The full orchestration loop and the axum web server are completed in issue #9;
//! these entry points read the parsed CLI args and drive the library.

use clap::ArgMatches;

use crate::config::Config;
use crate::error::{SiaError, SiaResult};

/// `sia web`: serve the runs visualizer (wired in #9).
pub fn run_web(_args: &ArgMatches) -> SiaResult<()> {
    Err(SiaError::new("`sia web` is not yet wired (Rust port issue #9)."))
}

/// `sia run`: the self-improvement loop (wired in #9).
pub fn run_orchestrator(_args: &ArgMatches, _env_config: &Config) -> SiaResult<()> {
    Err(SiaError::new("`sia run` is not yet wired (Rust port issue #9)."))
}
