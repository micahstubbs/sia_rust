//! Claude Code SDK agent impl. Port of `sia/agent_impls/claude.py`.
//!
//! The Claude Agent SDK has no Rust equivalent; `run_agent_claude` is the
//! integration boundary. The `provider` argument is accepted for a uniform
//! signature but ignored (the SDK authenticates against Anthropic natively).

use crate::agent_impls::base::RunArgs;
use crate::error::{SiaError, SiaResult};

pub fn run_agent_claude(_args: &RunArgs) -> SiaResult<()> {
    Err(SiaError::new(
        "claude agent impl requires the Claude Agent SDK, which is not available in the Rust port",
    ))
}
