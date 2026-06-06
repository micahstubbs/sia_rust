//! Error type modeling Python's `SystemExit`.
//!
//! Many SIA functions raise `SystemExit(msg)` on user-facing configuration
//! errors: the process prints `msg` to stderr and exits 1. In the Rust port
//! these return `Err(SiaError(msg))`; the binary entry point prints the message
//! and exits with status 1, reproducing the behavior.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct SiaError(pub String);

impl SiaError {
    pub fn new(msg: impl Into<String>) -> Self {
        SiaError(msg.into())
    }
}

impl fmt::Display for SiaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SiaError {}

pub type SiaResult<T> = Result<T, SiaError>;
