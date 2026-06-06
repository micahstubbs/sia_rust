//! Web visualizer for the SIA `runs/` directory. Port of `sia/web/`.
//!
//! `runs` is the pure data layer; `server` is the axum HTTP app + launchers.

pub mod runs;
pub mod server;

pub use server::{create_app, serve, serve_in_background};
