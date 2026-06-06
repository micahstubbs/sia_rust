//! Characterization: lock execution_status / execution_section text for the feedback
//! prompt across success/failure x single/multi x results. Port of
//! `tests/test_feedback_context_golden.py`.

mod common;

use serde_json::json;
use sia::orchestrator::build_feedback_context;
use sia::Config;
use sia::TaskFiles;

fn task_files() -> TaskFiles {
    TaskFiles::new("desc", "ref", json!({}), "# Task")
}

fn snapshot(gen_dir: &str, stdout_log_file: &str, status: &str, section: &str) -> String {
    let text =
        format!("===== EXECUTION STATUS =====\n{status}\n===== EXECUTION SECTION =====\n{section}");
    common::normalize_paths(text, &[(gen_dir, "<GEN>"), (stdout_log_file, "<LOG>")])
}

#[test]
fn test_success_single_with_results() {
    let d = tempfile::tempdir().unwrap();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    std::fs::write(
        gen_dir.join("agent_execution.json"),
        json!([{"role": "user", "content": "solve it"}]).to_string(),
    )
    .unwrap();
    std::fs::write(
        gen_dir.join("results.json"),
        json!({"accuracy": 0.9, "correct": 9, "total": 10}).to_string(),
    )
    .unwrap();
    let stdout_log = gen_dir
        .join("target_agent_stdout.log")
        .to_string_lossy()
        .into_owned();

    let (status, section) = build_feedback_context(
        1,
        gen_dir.to_str().unwrap(),
        "/data/public",
        true,
        "",
        "line1\nline2\nline3\n",
        "",
        &stdout_log,
        &task_files(),
        &Config::default(),
    );
    common::assert_golden(
        "feedback_context_success_single.txt",
        &snapshot(gen_dir.to_str().unwrap(), &stdout_log, &status, &section),
    );
}

#[test]
fn test_failure_single_no_results() {
    let d = tempfile::tempdir().unwrap();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    std::fs::write(
        gen_dir.join("agent_execution.json"),
        json!([{"role": "user", "content": "attempt"}]).to_string(),
    )
    .unwrap();
    let stdout_log = gen_dir
        .join("target_agent_stdout.log")
        .to_string_lossy()
        .into_owned();

    let (status, section) = build_feedback_context(
        1,
        gen_dir.to_str().unwrap(),
        "/data/public",
        false,
        "Target agent failed with exit code 1",
        "boot\nrunning\ncrash\n",
        "Traceback: boom",
        &stdout_log,
        &task_files(),
        &Config::default(),
    );
    common::assert_golden(
        "feedback_context_failure_single.txt",
        &snapshot(gen_dir.to_str().unwrap(), &stdout_log, &status, &section),
    );
}

#[test]
fn test_success_multi_with_results() {
    let d = tempfile::tempdir().unwrap();
    let gen_dir = d.path().join("gen_1");
    let exec_dir = gen_dir.join("agent_execution");
    std::fs::create_dir_all(&exec_dir).unwrap();
    for i in 0..2 {
        std::fs::write(
            exec_dir.join(format!("execution_q{i}.json")),
            json!([{"role": "user", "content": format!("q{i}")}]).to_string(),
        )
        .unwrap();
    }
    std::fs::write(
        gen_dir.join("results.json"),
        json!({"accuracy": 0.8}).to_string(),
    )
    .unwrap();
    let stdout_log = gen_dir
        .join("target_agent_stdout.log")
        .to_string_lossy()
        .into_owned();

    let (status, section) = build_feedback_context(
        1,
        gen_dir.to_str().unwrap(),
        "/data/public",
        true,
        "",
        "processing q0\nprocessing q1\ndone\n",
        "",
        &stdout_log,
        &task_files(),
        &Config::default(),
    );
    common::assert_golden(
        "feedback_context_success_multi.txt",
        &snapshot(gen_dir.to_str().unwrap(), &stdout_log, &status, &section),
    );
}
