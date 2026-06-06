//! Claude Code SDK agent impl. Port of `sia/agent_impls/claude.py`.
//!
//! The Claude Agent SDK has no Rust equivalent; `run_agent_claude` is the
//! integration boundary. The `provider` argument is accepted for a uniform
//! signature but ignored (the SDK authenticates against Anthropic natively).

use crate::agent_impls::base::RunArgs;
use crate::error::{SiaError, SiaResult};

pub fn run_agent_claude(_args: &RunArgs) -> SiaResult<()> {
    Err(SiaError::new(
        "the native `claude` agent runner is not yet implemented (tracked in issue #39). \
         The registry, dispatch, and model resolution are ported; the Anthropic Messages \
         API tool-loop is pending, so `sia run`'s meta/feedback agents are not yet end-to-end.",
    ))
}
