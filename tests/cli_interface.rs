//! Verify the CLI interface: run/web sub-commands and backward-compatible default.
//! Rust port of `tests/test_cli_interface.py`, driving the built `sia` binary.

use std::process::Command;

fn sia(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sia"))
        .args(args)
        .output()
        .expect("run sia")
}

#[test]
fn test_top_level_help_lists_subcommands() {
    let out = sia(&["--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("run"));
    assert!(stdout.contains("web"));
}

#[test]
fn test_run_help_exposes_orchestrator_flags() {
    let out = sia(&["run", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--max_gen",
        "--task",
        "--task_dir",
        "--meta-agent-profile",
        "--target-agent-profile",
        "--sandbox",
    ] {
        assert!(stdout.contains(flag), "missing {flag} in run --help");
    }
}

#[test]
fn test_web_help_exposes_server_flags() {
    let out = sia(&["web", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for flag in ["--host", "--port", "--runs-dir"] {
        assert!(stdout.contains(flag), "missing {flag} in web --help");
    }
}

#[test]
fn test_no_args_exits_nonzero() {
    assert!(!sia(&[]).status.success());
}

#[test]
fn test_default_subcommand_is_run() {
    // `sia --task nonexistent` is treated as `sia run --task nonexistent`.
    assert!(!sia(&["--task", "nonexistent"]).status.success());
}

#[test]
fn test_invalid_task_exits_nonzero() {
    assert!(!sia(&["run", "--task", "nonexistent"]).status.success());
}
