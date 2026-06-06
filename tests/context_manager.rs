//! Unit tests for the ContextManager. Rust port of `tests/test_context_manager.py`
//! plus the ContextManager parts of `test_config_injection.py`.

use serde_json::json;
use sia::config::Config;
use sia::context_manager::{ContextManager, GenData};

fn run_dir_with_gen1() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let gen1 = d.path().join("gen_1");
    std::fs::create_dir_all(&gen1).unwrap();
    std::fs::write(gen1.join("target_agent.py"), "print('hello')\n").unwrap();
    let root = d.path().to_path_buf();
    (d, root)
}

fn full_config() -> serde_json::Map<String, serde_json::Value> {
    json!({
        "task_dir": "./tasks/test-task",
        "meta_model": "haiku",
        "task_model": "haiku",
        "agent_impl": "claude",
        "max_gen": 3,
    })
    .as_object()
    .unwrap()
    .clone()
}

fn gen_data(agent_path: &std::path::Path, gen_dir: &std::path::Path, timestamp: &str) -> GenData {
    GenData {
        success: true,
        timestamp: timestamp.to_string(),
        duration: 10.5,
        agent_path: agent_path.to_string_lossy().into_owned(),
        gen_dir: gen_dir.to_string_lossy().into_owned(),
        improvement_path: None,
        execution_type: "Single".to_string(),
    }
}

#[test]
fn test_initialize_creates_context_md() {
    let (_d, root) = run_dir_with_gen1();
    let cm = ContextManager::new(root.to_str().unwrap(), full_config(), None);
    cm.initialize();
    let content = std::fs::read_to_string(root.join("context.md")).unwrap();
    assert!(content.contains("Run Context"));
    assert!(content.contains("haiku"));
}

#[test]
fn test_add_generation() {
    let (_d, root) = run_dir_with_gen1();
    let mut cm = ContextManager::new(root.to_str().unwrap(), full_config(), None);
    cm.initialize();
    let gen_dir = root.join("gen_1");
    cm.add_generation(
        1,
        &gen_data(
            &gen_dir.join("target_agent.py"),
            &gen_dir,
            "2025-01-01 00:00:00",
        ),
    );
    let content = std::fs::read_to_string(root.join("context.md")).unwrap();
    assert!(content.contains("Generation 1"));
    assert!(content.contains("SUCCESS"));
}

#[test]
fn test_add_generation_with_results_json() {
    let (_d, root) = run_dir_with_gen1();
    let mut cm = ContextManager::new(root.to_str().unwrap(), full_config(), None);
    cm.initialize();
    let gen_dir = root.join("gen_1");
    std::fs::write(
        gen_dir.join("results.json"),
        json!({"accuracy": 0.85, "n_correct": 170, "n_total": 200}).to_string(),
    )
    .unwrap();
    cm.add_generation(
        1,
        &gen_data(
            &gen_dir.join("target_agent.py"),
            &gen_dir,
            "2025-01-01 00:00:00",
        ),
    );
    let content = std::fs::read_to_string(root.join("context.md")).unwrap();
    assert!(content.contains("0.85"));
}

#[test]
fn test_finalize_with_metrics() {
    let (_d, root) = run_dir_with_gen1();
    let mut cm = ContextManager::new(root.to_str().unwrap(), full_config(), None);
    cm.initialize();
    let gen_dir = root.join("gen_1");
    std::fs::write(
        gen_dir.join("results.json"),
        json!({"accuracy": 0.80}).to_string(),
    )
    .unwrap();
    cm.add_generation(
        1,
        &gen_data(
            &gen_dir.join("target_agent.py"),
            &gen_dir,
            "2025-01-01 00:00:00",
        ),
    );
    cm.finalize();
    let content = std::fs::read_to_string(root.join("context.md")).unwrap();
    assert!(content.contains("Summary Statistics"));
}

#[test]
fn test_multiple_generations_track_deltas() {
    let (_d, root) = run_dir_with_gen1();
    let mut cm = ContextManager::new(root.to_str().unwrap(), full_config(), None);
    cm.initialize();

    let gen1 = root.join("gen_1");
    std::fs::write(
        gen1.join("results.json"),
        json!({"accuracy": 0.70}).to_string(),
    )
    .unwrap();
    cm.add_generation(
        1,
        &gen_data(&gen1.join("target_agent.py"), &gen1, "2025-01-01 00:00:00"),
    );

    let gen2 = root.join("gen_2");
    std::fs::create_dir_all(&gen2).unwrap();
    std::fs::write(
        gen2.join("target_agent.py"),
        "print('improved')\nimport os\n",
    )
    .unwrap();
    std::fs::write(
        gen2.join("results.json"),
        json!({"accuracy": 0.85}).to_string(),
    )
    .unwrap();
    std::fs::write(
        gen2.join("improvement.md"),
        "## Changes\n- Added better error handling\n- Improved prompt structure\n",
    )
    .unwrap();
    cm.add_generation(
        2,
        &GenData {
            improvement_path: Some(gen2.join("improvement.md").to_string_lossy().into_owned()),
            ..gen_data(&gen2.join("target_agent.py"), &gen2, "2025-01-01 00:01:00")
        },
    );

    let content = std::fs::read_to_string(root.join("context.md")).unwrap();
    assert!(content.contains("Generation 2"));
    assert!(content.contains("Modified by feedback agent"));
}

#[test]
fn test_context_manager_stores_injected_config() {
    let d = tempfile::tempdir().unwrap();
    let cfg = Config {
        agent_code_preview_limit: 7,
        context_summary_max_turns: 2,
        ..Config::default()
    };
    let run_config = json!({"meta_model": "x", "agent_impl": "claude"})
        .as_object()
        .unwrap()
        .clone();
    let cm = ContextManager::new(d.path().to_str().unwrap(), run_config, Some(cfg));
    assert_eq!(cm.cfg().agent_code_preview_limit, 7);
    assert_eq!(cm.cfg().context_summary_max_turns, 2);
}
