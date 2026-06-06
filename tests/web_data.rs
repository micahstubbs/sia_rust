//! Tests for the runs-visualizer data layer. Rust port of the data-layer parts of
//! `tests/test_web.py` (the HTTP API test lives in tests/web_api.rs).

use std::path::{Path, PathBuf};

use serde_json::json;
use sia::web::runs as rd;

fn make_runs_root() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("runs");
    let gen1 = root.join("run_7").join("gen_1");
    let gen2 = root.join("run_7").join("gen_2");
    std::fs::create_dir_all(gen1.join("agent_execution")).unwrap();
    std::fs::create_dir_all(&gen2).unwrap();

    std::fs::write(
        root.join("run_7").join("context.md"),
        "# Run Context: run_7\n\n\
**Task**: /tasks/gpqa\n\
**Meta Model**: kimi\n\
**Task Model**: haiku\n\
**Agent impl**: openhands\n\
**Started**: 2026-06-05 13:31:32\n\
**Max Generations**: 3\n\n\
---\n\n## Generation 1\n**Status**: ok\n",
    )
    .unwrap();

    std::fs::write(gen1.join("target_agent.py"), "print('hello')\n").unwrap();
    std::fs::write(gen1.join("meta_agent_prompt.txt"), "meta prompt body").unwrap();
    std::fs::write(
        gen1.join("evaluation_results.json"),
        json!({
            "total_questions": 4,
            "correct": 2,
            "incorrect": 2,
            "accuracy": 0.5,
            "accuracy_percent": 50.0,
            "details": [
                {"question_id": 1, "domain": "Physics", "is_correct": true},
                {"question_id": 2, "domain": "Physics", "is_correct": false},
                {"question_id": 3, "domain": "Biology", "is_correct": true},
                {"question_id": 4, "domain": "Biology", "is_correct": false},
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
            {"role": "assistant", "content": [{"type": "text", "text": "Answer: A"}]},
        ])
        .to_string(),
    )
    .unwrap();

    std::fs::write(gen2.join("improvement.md"), "# Plan\n- do better\n").unwrap();
    (d, root)
}

#[test]
fn test_list_runs_summary() {
    let (_d, root) = make_runs_root();
    let runs = rd::list_runs(&root);
    assert_eq!(runs.len(), 1);
    let r = &runs[0];
    assert_eq!(r.name, "run_7");
    assert_eq!(r.agent_impl.as_deref(), Some("openhands"));
    assert_eq!(r.task_model.as_deref(), Some("haiku"));
    assert_eq!(r.max_generations, Some(3));
    assert_eq!(r.num_generations, 2);
    assert_eq!(r.best_accuracy_percent, Some(50.0));
}

#[test]
fn test_get_run_detail_and_domains() {
    let (_d, root) = make_runs_root();
    let detail = rd::get_run(&root, "run_7").unwrap();
    assert!(detail.context_md.is_some());
    assert!(detail
        .context_md
        .as_ref()
        .unwrap()
        .starts_with("# Run Context"));
    let gen1 = detail
        .generations
        .iter()
        .find(|g| g.name == "gen_1")
        .unwrap();
    assert!(gen1.eval.is_some());
    assert_eq!(gen1.eval.as_ref().unwrap().accuracy_percent, Some(50.0));
    assert!(gen1.artifacts.contains(&"target_agent".to_string()));
    assert!(gen1.artifacts.contains(&"meta_prompt".to_string()));
    assert_eq!(gen1.trajectories, vec![1]);
    let physics = gen1.domains.iter().find(|d| d.domain == "Physics").unwrap();
    assert_eq!(physics.correct, 1);
    assert_eq!(physics.total, 2);
    let biology = gen1.domains.iter().find(|d| d.domain == "Biology").unwrap();
    assert_eq!(biology.accuracy_percent, 50.0);
}

#[test]
fn test_eval_details_and_artifacts() {
    let (_d, root) = make_runs_root();
    let details = rd::get_eval_details(&root, "run_7", "gen_1").unwrap();
    assert_eq!(details.len(), 4);
    assert_eq!(
        rd::get_artifact_text(&root, "run_7", "gen_1", "target_agent").as_deref(),
        Some("print('hello')\n")
    );
    let improvement = rd::get_artifact_text(&root, "run_7", "gen_2", "improvement").unwrap();
    assert!(improvement.starts_with("# Plan"));
}

#[test]
fn test_trajectory_normalization() {
    let (_d, root) = make_runs_root();
    let turns = rd::get_trajectory(&root, "run_7", "gen_1", 1).unwrap();
    let roles: Vec<&str> = turns.iter().map(|t| t["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["system", "user", "assistant"]);
    assert_eq!(turns[0]["text"], "You are an expert.");
    assert_eq!(turns[1]["text"], "Question 1?");
    assert_eq!(turns[2]["text"], "Answer: A");
}

#[test]
fn test_missing_lookups_return_none() {
    let (_d, root) = make_runs_root();
    assert!(rd::get_run(&root, "run_999").is_none());
    assert!(rd::get_trajectory(&root, "run_7", "gen_1", 999).is_none());
    assert!(rd::get_artifact_text(&root, "run_7", "gen_1", "nope").is_none());
}

#[test]
fn test_path_traversal_is_blocked() {
    let (_d, root) = make_runs_root();
    for evil in ["..", "../etc", "run_7/../run_7", "foo/bar", ".", "/abs"] {
        assert!(
            rd::get_run(&root, evil).is_none(),
            "get_run({evil}) should be None"
        );
        assert!(
            rd::resolve_gen(&root, evil, "gen_1").is_none(),
            "resolve_gen({evil}, gen_1)"
        );
        assert!(
            rd::resolve_gen(&root, "run_7", evil).is_none(),
            "resolve_gen(run_7, {evil})"
        );
    }
}

#[test]
fn test_runs_root_missing_is_empty() {
    let d = tempfile::tempdir().unwrap();
    assert!(rd::list_runs(&d.path().join("nope")).is_empty());
}

// Keep Path import used even if helpers change.
#[allow(dead_code)]
fn _typecheck(_: &Path) {}
