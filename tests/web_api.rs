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
    // Closed-loop artifacts (#84): gen_1 carries a scheduler decision + weight
    // update; gen_2 carries neither (exercises 404 / empty state).
    std::fs::write(
        gen1.join("scheduler_decision.json"),
        json!({
            "generation": 1, "decision": "weight", "recommended_next": "weight",
            "rationale": "Harness improvement has plateaued; recommending a weight update.",
            "harness_efficiency": 0.001, "weight_efficiency": null, "harness_plateaued": true
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        gen1.join("weight_update.json"),
        json!({
            "generation": 1, "kind": "weight", "updater": "lora-reference-cpu",
            "num_examples": 2, "loss_before": 0.5, "loss_after": 0.3,
            "updated": true, "details": "lora-reference-cpu: 2 example(s)"
        })
        .to_string(),
    )
    .unwrap();
    // gen_1 carries telemetry; gen_2 does not (exercises graceful degradation).
    std::fs::write(
        gen1.join("telemetry.json"),
        json!({
            "generations": [
                {"generation": 1, "input_tokens": 100, "output_tokens": 20,
                 "num_api_calls": 2, "num_tool_calls": 1, "duration_ms": 500}
            ],
            "cumulative": {"generation": 1, "input_tokens": 100, "output_tokens": 20,
                 "num_api_calls": 2, "num_tool_calls": 1, "duration_ms": 500}
        })
        .to_string(),
    )
    .unwrap();
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

#[tokio::test]
async fn test_telemetry_and_metrics_endpoints() {
    let (_d, root) = make_runs_root();
    let app = sia::web::create_app(root);

    // Per-generation telemetry (present).
    let (status, body) = get(&app, "/api/runs/run_7/gens/gen_1/telemetry").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["cumulative"]["input_tokens"], json!(100));

    // Per-generation telemetry (absent) -> 404.
    let (status, _body) = get(&app, "/api/runs/run_7/gens/gen_2/telemetry").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Run-level telemetry folds the single telemetry-bearing generation.
    let (status, body) = get(&app, "/api/runs/run_7/telemetry").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["generations"].as_array().unwrap().len(), 1);
    assert_eq!(v["cumulative"]["output_tokens"], json!(20));
    assert_eq!(v["cumulative"]["num_tool_calls"], json!(1));

    // Metrics summary: score per gen + tokens where telemetry exists.
    let (status, body) = get(&app, "/api/runs/run_7/metrics").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let gens = v["generations"].as_array().unwrap();
    assert_eq!(gens.len(), 2);
    assert_eq!(gens[0]["score"], json!(50.0));
    assert_eq!(gens[0]["total_tokens"], json!(120));
    // gen_2 has eval-less / telemetry-less => score null, no token fields.
    assert!(gens[1].get("input_tokens").is_none());
    assert_eq!(v["totals"]["total_tokens"], json!(120));

    // Unknown run -> 404 on both new endpoints.
    let (status, _b) = get(&app, "/api/runs/run_404/metrics").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _b) = get(&app, "/api/runs/run_404/telemetry").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_scheduler_and_weights_endpoints() {
    let (_d, root) = make_runs_root();
    let app = sia::web::create_app(root);

    // Scheduler decision present on gen_1 -> 200 with artifact.
    let (status, body) = get(&app, "/api/runs/run_7/gens/gen_1/scheduler").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["decision"], json!("weight"));
    assert_eq!(v["harness_plateaued"], json!(true));

    // Weight update present on gen_1 -> 200 with loss before/after.
    let (status, body) = get(&app, "/api/runs/run_7/gens/gen_1/weights").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["kind"], json!("weight"));
    assert_eq!(v["loss_after"], json!(0.3));

    // Absent on gen_2 -> 404 for both.
    let (status, _b) = get(&app, "/api/runs/run_7/gens/gen_2/scheduler").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _b) = get(&app, "/api/runs/run_7/gens/gen_2/weights").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
