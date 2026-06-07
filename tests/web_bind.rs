//! Bind-behavior tests for the auto-started dashboard (issue #115).
//!
//! The orchestrator used to print `Live dashboard: http://host:port` regardless
//! of whether the bind succeeded, so a busy port produced a URL where nothing
//! served. These tests pin the new contract: bind happens up front, an explicit
//! `--web-port` that is in use fails visibly, and the default port falls back to
//! a different (real) port instead of lying.

use std::net::TcpListener;

use sia::web::serve_in_background;

const HOST: &str = "127.0.0.1";

/// Bind an ephemeral port and keep the listener alive so the port stays busy.
fn occupy_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind((HOST, 0)).expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

#[test]
fn explicit_port_in_use_errors_instead_of_lying() {
    let runs = tempfile::tempdir().unwrap();
    let runs_dir = runs.path().to_string_lossy().to_string();

    let (_busy, port) = occupy_port();

    // explicit_port = true: must surface an error, not claim a live dashboard.
    let result = serve_in_background(HOST, port, &runs_dir, /* explicit_port */ true);
    assert!(
        result.is_err(),
        "explicit --web-port on a busy port must error, got Ok"
    );
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains(&port.to_string()),
        "error should mention the busy port {port}: {msg}"
    );
}

#[test]
fn default_port_in_use_falls_back_to_real_port() {
    let runs = tempfile::tempdir().unwrap();
    let runs_dir = runs.path().to_string_lossy().to_string();

    let (_busy, port) = occupy_port();

    // explicit_port = false: must auto-select a *different*, actually-bound port.
    let dashboard = serve_in_background(HOST, port, &runs_dir, /* explicit_port */ false)
        .expect("default path should find a free port");

    assert_ne!(
        dashboard.port, port,
        "fallback must not reuse the occupied port"
    );

    // The reported port must be the real one: binding it again must fail because
    // the dashboard now owns it.
    assert!(
        TcpListener::bind((HOST, dashboard.port)).is_err(),
        "reported port {} should actually be bound by the dashboard",
        dashboard.port
    );
}

#[test]
fn free_default_port_binds_and_reports_that_port() {
    let runs = tempfile::tempdir().unwrap();
    let runs_dir = runs.path().to_string_lossy().to_string();

    // Find a currently-free port, release it, then ask the dashboard for it.
    let port = {
        let (l, p) = occupy_port();
        drop(l);
        p
    };

    let dashboard = serve_in_background(HOST, port, &runs_dir, /* explicit_port */ true)
        .expect("free explicit port should bind");
    assert_eq!(
        dashboard.port, port,
        "an explicit, free port must be the one reported"
    );
}
