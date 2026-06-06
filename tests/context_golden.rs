//! Characterization: lock the full context.md produced by ContextManager.
//! Rust port of `tests/test_context_golden.py`.

mod common;

use serde_json::json;
use sia::context_manager::{ContextManager, GenData};

const GEN1_AGENT: &str = "print('gen 1 agent')\n";
const GEN2_AGENT: &str =
    "import sys\n\n\ndef main():\n    print('gen 2 agent, improved')\n\n\nmain()\n";
const IMPROVEMENT_MD: &str = "# Improvement Plan\n\n\
- Added structured error handling so the agent recovers from tool failures gracefully.\n\
- Switched to a retry loop with exponential backoff for transient API errors.\n\
- Improved logging to capture each tool call and its result for later analysis.\n";

#[test]
fn test_context_md_golden() {
    let d = tempfile::tempdir().unwrap();
    let run_dir = d.path().join("run_1");
    let gen1 = run_dir.join("gen_1");
    let gen2 = run_dir.join("gen_2");
    std::fs::create_dir_all(&gen1).unwrap();
    std::fs::create_dir_all(&gen2).unwrap();

    std::fs::write(gen1.join("target_agent.py"), GEN1_AGENT).unwrap();
    std::fs::write(gen2.join("target_agent.py"), GEN2_AGENT).unwrap();
    std::fs::write(gen2.join("improvement.md"), IMPROVEMENT_MD).unwrap();
    std::fs::write(
        gen1.join("results.json"),
        json!({"accuracy": 50.0, "correct": 99, "total": 198}).to_string(),
    )
    .unwrap();
    std::fs::write(
        gen2.join("results.json"),
        json!({"accuracy": 75.0, "correct": 148, "total": 198}).to_string(),
    )
    .unwrap();

    let config = json!({
        "task_dir": "/tasks/example",
        "meta_model": "haiku",
        "task_model": "claude-haiku-4-5-20251001",
        "agent_impl": "claude",
        "max_gen": 2,
    })
    .as_object()
    .unwrap()
    .clone();

    let mut cm = ContextManager::new(run_dir.to_str().unwrap(), config, None);
    cm.initialize();
    cm.add_generation(
        1,
        &GenData {
            success: true,
            timestamp: "2026-01-01 00:00:00".to_string(),
            duration: 1.5,
            agent_path: gen1.join("target_agent.py").to_string_lossy().into_owned(),
            gen_dir: gen1.to_string_lossy().into_owned(),
            improvement_path: None,
            execution_type: "Single".to_string(),
        },
    );
    cm.add_generation(
        2,
        &GenData {
            success: true,
            timestamp: "2026-01-01 00:05:00".to_string(),
            duration: 2.5,
            agent_path: gen2.join("target_agent.py").to_string_lossy().into_owned(),
            gen_dir: gen2.to_string_lossy().into_owned(),
            improvement_path: Some(gen2.join("improvement.md").to_string_lossy().into_owned()),
            execution_type: "Single".to_string(),
        },
    );
    cm.finalize();

    let content =
        common::normalize_timestamps(&std::fs::read_to_string(run_dir.join("context.md")).unwrap());
    common::assert_golden("context.md", &content);
}
