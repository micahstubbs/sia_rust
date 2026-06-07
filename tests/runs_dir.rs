//! Tests for the `sia run` runs-output override (`--runs-dir` / `SIA_RUNS_DIR`).
//!
//! Verifies the flag > env > `./runs` precedence, that the resolved root flows
//! into `RunLayout::for_run_id` (the directory the run writes to), and that the
//! same root is what the background dashboard would serve. Env-mutating cases are
//! serialized with a mutex (see `tests/config_env.rs`) to avoid cross-test races.

use std::sync::Mutex;

use sia::layout::{names, RunLayout};
use sia::run::resolve_runs_dir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_flag_wins_over_env_and_default() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var("SIA_RUNS_DIR", "/env/runs");
    // Explicit flag takes precedence over the env var.
    assert_eq!(resolve_runs_dir(Some("/flag/runs")), "/flag/runs");
    std::env::remove_var("SIA_RUNS_DIR");
}

#[test]
fn test_env_used_when_flag_absent() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var("SIA_RUNS_DIR", "/env/runs");
    assert_eq!(resolve_runs_dir(None), "/env/runs");
    std::env::remove_var("SIA_RUNS_DIR");
}

#[test]
fn test_empty_env_falls_back_to_default() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var("SIA_RUNS_DIR", "");
    assert_eq!(resolve_runs_dir(None), names::RUNS_ROOT);
    std::env::remove_var("SIA_RUNS_DIR");
}

#[test]
fn test_default_when_neither_set() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::remove_var("SIA_RUNS_DIR");
    assert_eq!(resolve_runs_dir(None), "./runs");
    assert_eq!(resolve_runs_dir(None), names::RUNS_ROOT);
}

#[test]
fn test_resolved_root_flows_into_run_layout() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::remove_var("SIA_RUNS_DIR");

    // Default: run writes under ./runs.
    let default_root = resolve_runs_dir(None);
    let default_layout = RunLayout::for_run_id(1, &default_root);
    assert_eq!(default_layout.run_dir, "./runs/run_1");

    // Flag override: the run directory (and thus all artifacts) move with it.
    let flag_root = resolve_runs_dir(Some("/custom/output"));
    let flag_layout = RunLayout::for_run_id(7, &flag_root);
    assert_eq!(flag_layout.run_dir, "/custom/output/run_7");
    assert_eq!(flag_layout.context_md(), "/custom/output/run_7/context.md");
}

#[test]
fn test_dashboard_root_matches_run_root() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var("SIA_RUNS_DIR", "/shared/runs");

    // The background dashboard is started with this exact root, and the run
    // directory is created under the same root, so the dashboard serves precisely
    // the directory the run writes to.
    let runs_root = resolve_runs_dir(None);
    let layout = RunLayout::for_run_id(3, &runs_root);
    let dashboard_root = runs_root.clone();

    assert_eq!(dashboard_root, "/shared/runs");
    assert!(layout.run_dir.starts_with(&dashboard_root));
    assert_eq!(layout.run_dir, "/shared/runs/run_3");

    std::env::remove_var("SIA_RUNS_DIR");
}
