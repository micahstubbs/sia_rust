//! Agent-implementation registry package. Port of `sia/agent_impls/`.
//!
//! An *agent impl* runs a meta/feedback agent. In Python each impl wraps an
//! external LLM SDK (Claude Agent SDK / OpenHands / PydanticAI). Those SDKs have
//! no Rust equivalent, so the runners here port the deterministic parts — the
//! registry, dispatch, and model-spec resolution — and treat the actual LLM call
//! as the integration boundary (see each runner's docs).

pub mod base;
pub mod claude;
pub mod openhands;
pub mod pydantic_ai;

pub use base::{available_agent_impls, get_agent_impl, register, run_agent, RunArgs, Runner};
