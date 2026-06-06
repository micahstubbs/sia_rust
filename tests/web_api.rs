//! HTTP API tests for the runs visualizer. Rust port of
//! `tests/test_web.py::test_api_endpoints`, driving the axum app via tower oneshot.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

fn make_runs_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("runs");
    let gen1 = root.join("run_7").join("gen_1");
    let gen2 = root.join("run_7").join("gen_2");
    std::fs::create_dir_all(gen1.join("agent_execution")).unwrap();
    std::fs::create_dir_all(&gen2).unwrap();
    std::fs::write(
        root.join("run_7").join("context.md"),
        "# Run Context: run_7\n\n**Task**: /tasks/gpqa\n**Meta Model**: kimi\n**Task Model**: haiku\n\
**Agent impl**: openhands\n**Started**: 2026-06-05 13:31:32\n**Max Generations**: 3\n\n---\n\n## Generation 1\n",
    )
    .unwrap();
    std::fs::write(gen1.join("target_agent.py"), "print('hello')\n").unwrap();
    std::fs::write(gen1.join("meta_agent_prompt.txt"), "meta prompt body").unwrap();
    std::fs::write(
        gen1.join("evaluation_results.json"),
        json!({
            "total_questions": 4, "correct": 2, "incorrect": 2, "accuracy": 0.5, "accuracy_percent": 50.0,
            "details": [
                {"question_id": 1, "domain": "Physics", "is_correct": true},
                {"question_id": 2, "domain": "Physics", "is_correct": false},
                {"question_id": 3, "domain": "Biology", "is_correct": true},
                {"question_id": 4, "domain": "Biology", "is_correct": false}
            ]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        gen1.join("agent_execution").join("execution_q1.json"),
        json!([
            {"role": "system", "content": [{"type": "text", "text": "You are an expert."}]},
            {"role": "user", "content": "Question 1?"},
            {"role": "assistant", "content": [{"type": "text", "text": "Answer: A"}]}
        ])
        .to_string(),
    )
    .unwrap();
    std::fs::write(gen2.join("improvement.md"), "# Plan\n- do better\n").unwrap();
    (d, root)
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn test_api_endpoints() {
    let (_d, root) = make_runs_root();
    let app = sia::web::create_app(root);

    let (_s, body) = get(&app, "/api/runs").await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v[0]["name"], "run_7");

    let (_s, body) = get(&app, "/api/runs/run_7").await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["agent_impl"], "openhands");

    let (_s, body) = get(&app, "/api/runs/run_7/gens/gen_1/eval").await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 4);

    let (_s, body) = get(&app, "/api/runs/run_7/gens/gen_1/artifact/target_agent").await;
    assert!(body.contains("hello"));

    let (_s, body) = get(&app, "/api/runs/run_7/gens/gen_1/trajectory/1").await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v[0]["role"], "system");

    let (status, _body) = get(&app, "/api/runs/run_404").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _body) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
}
