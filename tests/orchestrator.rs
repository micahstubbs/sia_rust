//! Orchestrator tests — Rust port of test_orchestrator_helpers, test_load_execution_formats,
//! test_run_evaluation, test_run_evaluation_outcomes, test_sandbox, test_config_injection
//! (load_agent_execution + run_evaluation parts).

use std::sync::{Arc, Mutex};

use serde_json::json;
use sia::config::Config;
use sia::orchestrator::{
    build_sandbox_cmd, build_target_cmd, load_agent_execution, run_evaluation_with,
    run_target_agent_with, EvalOutcome,
};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// --------------------------- load_agent_execution --------------------------- //

#[test]
fn test_load_single_trajectory() {
    let d = tmp();
    std::fs::write(
        d.path().join("agent_execution.json"),
        json!([{"role": "user", "content": "hello"}]).to_string(),
    )
    .unwrap();
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(!is_multi);
    assert!(data.is_array());
    assert_eq!(data[0]["role"], "user");
}

#[test]
fn test_load_multi_trajectory() {
    let d = tmp();
    let exec = d.path().join("agent_execution");
    std::fs::create_dir(&exec).unwrap();
    for i in 0..3 {
        std::fs::write(
            exec.join(format!("execution_q{i}.json")),
            json!([{"role": "user", "content": format!("question {i}")}]).to_string(),
        )
        .unwrap();
    }
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(is_multi);
    assert_eq!(data["count"], 3);
    assert_eq!(data["trajectories"].as_array().unwrap().len(), 3);
}

#[test]
fn test_load_missing_execution() {
    let d = tmp();
    let (data, _) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(data.get("error").is_some());
}

#[test]
fn test_load_malformed_json() {
    let d = tmp();
    std::fs::write(d.path().join("agent_execution.json"), "{not valid json").unwrap();
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(!is_multi);
    assert!(data.get("error").is_some() || data.get("raw_preview").is_some());
}

#[test]
fn test_load_empty_multi_trajectory_folder() {
    let d = tmp();
    std::fs::create_dir(d.path().join("agent_execution")).unwrap();
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(is_multi);
    assert!(data.get("error").is_some());
}

// exact-shape characterizations
#[test]
fn test_missing_returns_exact_error() {
    let d = tmp();
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(!is_multi);
    assert_eq!(data, json!({"error": "Execution log not found"}));
}

#[test]
fn test_empty_multi_folder_returns_exact_error() {
    let d = tmp();
    std::fs::create_dir(d.path().join("agent_execution")).unwrap();
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(is_multi);
    assert_eq!(
        data,
        json!({"error": "Empty execution folder", "type": "multi-trajectory"})
    );
}

#[test]
fn test_malformed_single_returns_partial_preview() {
    let d = tmp();
    std::fs::write(d.path().join("agent_execution.json"), "{not valid json").unwrap();
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(!is_multi);
    assert_eq!(data["error"], "Parse error");
    assert_eq!(data["raw_preview"], "{not valid json");
    assert_eq!(data["file_size"], "{not valid json".len() as i64);
    assert!(data.get("parse_error").is_some());
}

#[test]
fn test_multi_trajectory_shape() {
    let d = tmp();
    let exec = d.path().join("agent_execution");
    std::fs::create_dir(&exec).unwrap();
    for i in 0..3 {
        std::fs::write(
            exec.join(format!("execution_q{i}.json")),
            json!([{"role": "user", "content": format!("q{i}")}]).to_string(),
        )
        .unwrap();
    }
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(is_multi);
    assert_eq!(data["type"], "multi-trajectory");
    assert_eq!(data["count"], 3);
    let contents: Vec<String> = data["trajectories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t[0]["content"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(contents, vec!["q0", "q1", "q2"]);
}

#[test]
fn test_load_agent_execution_honors_injected_max_size() {
    let d = tmp();
    std::fs::write(
        d.path().join("agent_execution.json"),
        json!([{"role": "user", "content": "hi"}]).to_string(),
    )
    .unwrap();
    let cfg = Config {
        max_execution_log_size: 1,
        ..Config::default()
    };
    let (data, is_multi) = load_agent_execution(d.path().to_str().unwrap(), &cfg);
    assert!(!is_multi);
    assert_eq!(data["error"], "File too large");

    let (data2, _) = load_agent_execution(d.path().to_str().unwrap(), &Config::default());
    assert!(data2.is_array());
    assert_eq!(data2[0]["role"], "user");
}

// ------------------------------ run_evaluation ------------------------------ //

fn make_task_with_eval(task_dir: &std::path::Path) {
    let pub_dir = task_dir.join("data").join("public");
    std::fs::create_dir_all(&pub_dir).unwrap();
    std::fs::write(pub_dir.join("evaluate.py"), "pass").unwrap();
}

#[test]
fn test_skipped_when_no_evaluate_py() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    let task_dir = d.path().join("task");
    std::fs::create_dir(&task_dir).unwrap();
    let runner = |_cmd: &[String], _t: u64| EvalOutcome::Completed {
        returncode: 0,
        stdout: String::new(),
        stderr: String::new(),
    };
    let result = run_evaluation_with(
        &runner,
        gen_dir.to_str().unwrap(),
        task_dir.to_str().unwrap(),
        "/fake/venv",
        &Config::default(),
    );
    assert_eq!(result["status"], "skipped");
}

#[test]
fn test_success_when_results_json_created() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    std::fs::write(
        gen_dir.join("results.json"),
        json!({"accuracy": 0.9}).to_string(),
    )
    .unwrap();
    make_task_with_eval(&d.path().join("task"));
    let runner = |_cmd: &[String], _t: u64| EvalOutcome::Completed {
        returncode: 0,
        stdout: "ok".into(),
        stderr: String::new(),
    };
    let result = run_evaluation_with(
        &runner,
        gen_dir.to_str().unwrap(),
        d.path().join("task").to_str().unwrap(),
        "/fake/venv",
        &Config::default(),
    );
    assert_eq!(result["status"], "success");
}

#[test]
fn test_error_on_nonzero_exit() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    make_task_with_eval(&d.path().join("task"));
    let runner = |_cmd: &[String], _t: u64| EvalOutcome::Completed {
        returncode: 1,
        stdout: String::new(),
        stderr: "traceback".into(),
    };
    let result = run_evaluation_with(
        &runner,
        gen_dir.to_str().unwrap(),
        d.path().join("task").to_str().unwrap(),
        "/fake/venv",
        &Config::default(),
    );
    assert_eq!(result["status"], "error");
    assert!(result["reason"].as_str().unwrap().contains("code 1"));
}

#[test]
fn test_timeout_handled() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    make_task_with_eval(&d.path().join("task"));
    let runner = |_cmd: &[String], _t: u64| EvalOutcome::TimedOut;
    let result = run_evaluation_with(
        &runner,
        gen_dir.to_str().unwrap(),
        d.path().join("task").to_str().unwrap(),
        "/fake/venv",
        &Config::default(),
    );
    assert_eq!(result["status"], "error");
    assert!(result["reason"].as_str().unwrap().contains("timed out"));
}

#[test]
fn test_warning_when_results_json_missing() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    make_task_with_eval(&d.path().join("task"));
    let runner = |_cmd: &[String], _t: u64| EvalOutcome::Completed {
        returncode: 0,
        stdout: "done, no results written".into(),
        stderr: String::new(),
    };
    let result = run_evaluation_with(
        &runner,
        gen_dir.to_str().unwrap(),
        d.path().join("task").to_str().unwrap(),
        "/fake/venv",
        &Config::default(),
    );
    assert_eq!(result["status"], "warning");
    assert_eq!(result["reason"], "results.json not created by evaluate.py");
}

#[test]
fn test_run_evaluation_honors_injected_timeout() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    let pub_dir = d.path().join("task").join("data").join("public");
    std::fs::create_dir_all(&pub_dir).unwrap();
    std::fs::write(pub_dir.join("evaluate.py"), "pass").unwrap();
    std::fs::write(
        gen_dir.join("results.json"),
        json!({"accuracy": 1.0}).to_string(),
    )
    .unwrap();

    let captured = Arc::new(Mutex::new(0u64));
    let cap = captured.clone();
    let runner = move |_cmd: &[String], t: u64| {
        *cap.lock().unwrap() = t;
        EvalOutcome::Completed {
            returncode: 0,
            stdout: "ok".into(),
            stderr: String::new(),
        }
    };
    let cfg = Config {
        eval_timeout: 123,
        ..Config::default()
    };
    run_evaluation_with(
        &runner,
        gen_dir.to_str().unwrap(),
        d.path().join("task").to_str().unwrap(),
        "/fake/venv",
        &cfg,
    );
    assert_eq!(*captured.lock().unwrap(), 123);
}

// -------------------------------- sandbox --------------------------------- //

#[test]
fn test_docker_command_has_network_none() {
    let cmd = build_sandbox_cmd("/data", "/work", &Config::default());
    let idx = cmd.iter().position(|x| x == "--network").unwrap();
    assert_eq!(cmd[idx + 1], "none");
}

#[test]
fn test_docker_dataset_mounted_readonly() {
    let cmd = build_sandbox_cmd("/data", "/work", &Config::default());
    let vol_idx = cmd.iter().position(|x| x == "-v").unwrap();
    assert!(cmd[vol_idx + 1].contains(":/data:ro"));
}

#[test]
fn test_docker_working_dir_mounted_readwrite() {
    let cmd = build_sandbox_cmd("/data", "/work", &Config::default());
    let vol_indices: Vec<usize> = cmd
        .iter()
        .enumerate()
        .filter(|(_, x)| *x == "-v")
        .map(|(i, _)| i)
        .collect();
    assert!(cmd[vol_indices[1] + 1].contains(":/work:rw"));
}

#[test]
fn test_docker_image_and_resource_limits() {
    let cfg = Config::default();
    let cmd = build_sandbox_cmd("/data", "/work", &cfg);
    assert!(cmd.contains(&cfg.docker_image));
    let mem_idx = cmd.iter().position(|x| x == "--memory").unwrap();
    assert_eq!(cmd[mem_idx + 1], cfg.docker_memory_limit);
    let cpu_args: Vec<&String> = cmd.iter().filter(|a| a.starts_with("--cpus=")).collect();
    assert_eq!(cpu_args.len(), 1);
    assert!(cpu_args[0].contains("2.0"));
}

#[test]
fn test_sandbox_none_uses_standard_command() {
    let cmd = build_target_cmd(
        &sia::layout::venv_python_path("/fake/venv"),
        "/tmp/gen/target_agent.py",
        "/data",
        "/tmp/gen",
    );
    assert_eq!(cmd[0], "/fake/venv/bin/python");
    assert!(!cmd[0].contains("docker"));
}

// ------------------------- run_target_agent (seam) ------------------------- //

#[test]
fn test_run_target_agent_success() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    let stdout_log = gen_dir.join("stdout.log").to_string_lossy().into_owned();
    std::fs::write(gen_dir.join("target_agent.py"), "print('ok')").unwrap();

    let runner = |_cmd: &[String], log: &str| -> std::io::Result<i32> {
        std::fs::write(log, "line1\n")?;
        Ok(0)
    };
    let (success, _stdout, _stderr, err) = run_target_agent_with(
        &runner,
        "/fake/venv",
        gen_dir.join("target_agent.py").to_str().unwrap(),
        "/data",
        gen_dir.to_str().unwrap(),
        &stdout_log,
        "none",
        &Config::default(),
    );
    assert!(success);
    assert_eq!(err, "");
}

#[test]
fn test_run_target_agent_failure() {
    let d = tmp();
    let gen_dir = d.path().join("gen_1");
    std::fs::create_dir(&gen_dir).unwrap();
    let stdout_log = gen_dir.join("stdout.log").to_string_lossy().into_owned();

    let runner = |_cmd: &[String], log: &str| -> std::io::Result<i32> {
        std::fs::write(log, "error\n")?;
        Ok(1)
    };
    let (success, _stdout, _stderr, err) = run_target_agent_with(
        &runner,
        "/fake/venv",
        gen_dir.join("target_agent.py").to_str().unwrap(),
        "/data",
        gen_dir.to_str().unwrap(),
        &stdout_log,
        "none",
        &Config::default(),
    );
    assert!(!success);
    assert!(err.contains("exit code 1"));
}
