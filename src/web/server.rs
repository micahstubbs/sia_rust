//! axum app + launchers for the runs visualizer. Port of `sia/web/server.py`.
//!
//! `create_app(runs_dir)` builds the router; `serve(...)` runs it in the foreground
//! (the `sia web` command); `serve_in_background(...)` starts it on a daemon thread
//! so the orchestrator can expose a live dashboard during `sia run`.

use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::assets;
use crate::web::runs as runs_data;

type AppState = Arc<PathBuf>;

/// Build the axum application serving the runs under `runs_dir`.
pub fn create_app(runs_dir: impl Into<PathBuf>) -> Router {
    let runs_dir = runs_dir.into();
    let runs_root = std::fs::canonicalize(&runs_dir).unwrap_or(runs_dir);
    let state: AppState = Arc::new(runs_root);

    Router::new()
        .route("/api/runs", get(api_runs))
        .route("/api/runs/:run_name", get(api_run))
        .route(
            "/api/runs/:run_name/gens/:gen_name/eval",
            get(api_eval_details),
        )
        .route(
            "/api/runs/:run_name/gens/:gen_name/artifact/:label",
            get(api_artifact),
        )
        .route(
            "/api/runs/:run_name/gens/:gen_name/trajectory/:qid",
            get(api_trajectory),
        )
        .route(
            "/api/runs/:run_name/gens/:gen_name/openhands",
            get(api_openhands_sessions),
        )
        .route(
            "/api/runs/:run_name/gens/:gen_name/openhands/:session",
            get(api_openhands_events),
        )
        .route("/", get(index))
        .with_state(state)
}

async fn api_runs(State(root): State<AppState>) -> Json<Vec<runs_data::RunSummary>> {
    Json(runs_data::list_runs(&root))
}

async fn api_run(State(root): State<AppState>, Path(run_name): Path<String>) -> Response {
    match runs_data::get_run(&root, &run_name) {
        Some(detail) => Json(detail).into_response(),
        None => not_found(&format!("Run not found: {run_name}")),
    }
}

async fn api_eval_details(
    State(root): State<AppState>,
    Path((run_name, gen_name)): Path<(String, String)>,
) -> Response {
    match runs_data::get_eval_details(&root, &run_name, &gen_name) {
        Some(details) => Json(details).into_response(),
        None => not_found("No evaluation details found"),
    }
}

async fn api_artifact(
    State(root): State<AppState>,
    Path((run_name, gen_name, label)): Path<(String, String, String)>,
) -> Response {
    match runs_data::get_artifact_text(&root, &run_name, &gen_name, &label) {
        Some(text) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response(),
        None => not_found(&format!("Artifact not found: {label}")),
    }
}

async fn api_trajectory(
    State(root): State<AppState>,
    Path((run_name, gen_name, qid)): Path<(String, String, i64)>,
) -> Response {
    match runs_data::get_trajectory(&root, &run_name, &gen_name, qid) {
        Some(turns) => Json(turns).into_response(),
        None => not_found(&format!("Trajectory not found: q{qid}")),
    }
}

async fn api_openhands_sessions(
    State(root): State<AppState>,
    Path((run_name, gen_name)): Path<(String, String)>,
) -> Response {
    match runs_data::list_openhands_sessions(&root, &run_name, &gen_name) {
        Some(sessions) => Json(sessions).into_response(),
        None => not_found("Generation not found"),
    }
}

async fn api_openhands_events(
    State(root): State<AppState>,
    Path((run_name, gen_name, session)): Path<(String, String, String)>,
) -> Response {
    match runs_data::get_openhands_events(&root, &run_name, &gen_name, &session) {
        Some(events) => Json(events).into_response(),
        None => not_found("Session not found"),
    }
}

async fn index() -> Response {
    match assets::web_static_bytes("index.html") {
        Some(bytes) => {
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], bytes).into_response()
        }
        None => not_found("index.html not found"),
    }
}

fn not_found(detail: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"detail": detail})),
    )
        .into_response()
}

/// Run the server in the foreground (blocks). Used by `sia web`.
pub fn serve(host: &str, port: u16, runs_dir: &str, _open_browser: bool) -> crate::SiaResult<()> {
    let app = create_app(runs_dir);
    let addr = resolve_addr(host, port)?;
    let resolved = std::fs::canonicalize(runs_dir).unwrap_or_else(|_| PathBuf::from(runs_dir));
    println!(
        "SIA visualizer serving {} at http://{}",
        resolved.display(),
        addr
    );

    let rt = tokio::runtime::Runtime::new().map_err(|e| crate::SiaError::new(e.to_string()))?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| crate::SiaError::new(format!("Could not bind {addr}: {e}")))?;
        axum::serve(listener, app)
            .await
            .map_err(|e| crate::SiaError::new(e.to_string()))
    })
}

/// Start the server on a daemon thread; never blocks. Returns the thread handle.
pub fn serve_in_background(
    host: &str,
    port: u16,
    runs_dir: &str,
) -> Option<std::thread::JoinHandle<()>> {
    let host_owned = host.to_string();
    let runs_dir = runs_dir.to_string();
    let handle = std::thread::Builder::new()
        .name("sia-web".to_string())
        .spawn(move || {
            let _ = serve(&host_owned, port, &runs_dir, false);
        })
        .ok()?;
    println!("Live dashboard: http://{host}:{port}");
    Some(handle)
}

fn resolve_addr(host: &str, port: u16) -> crate::SiaResult<std::net::SocketAddr> {
    format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| crate::SiaError::new(format!("Invalid host/port {host}:{port}: {e}")))?
        .next()
        .ok_or_else(|| crate::SiaError::new(format!("Could not resolve {host}:{port}")))
}
