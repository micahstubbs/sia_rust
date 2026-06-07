//! Integration tests for the generation loop with injected agent seams.
//! Rust port of `tests/test_generation_loop.py` (the run_generation cases).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use sia::agent_impls::register;
use sia::agent_reference::ResolvedAgentReference;
use sia::config::Config;
use sia::context_manager::ContextManager;
use sia::orchestrator::{run_feedback_agent, run_generation_with, FeedbackArgs};
use sia::profiles::MetaAgentProfile;
use sia::providers::load_provider;
use sia::run_setup::RunSetup;
use sia::TaskFiles;

fn make_task_files(root: &std::path::Path) -> std::path::PathBuf {
    let task_dir = root.join("task");
    let pub_dir = task_dir.join("data").join("public");
    std::fs::create_dir_all(&pub_dir).unwrap();
    std::fs::write(pub_dir.join("task.md"), "# Test task\nSolve the problem.").unwrap();
    task_dir
}

fn make_run_setup(root: &std::path::Path, task_dir: &std::path::Path) -> RunSetup {
    let run_dir = root.join("runs").join("run_1");
    let gen1 = run_dir.join("gen_1");
    std::fs::create_dir_all(&gen1).unwrap();
    std::fs::write(gen1.join("target_agent.py"), "print('agent')\n").unwrap();

    let run_config = json!({
        "task_dir": task_dir.to_string_lossy(),
        "meta_model": "haiku",
        "task_model": "haiku",
        "agent_impl": "claude",
        "max_gen": 1,
    })
    .as_object()
    .unwrap()
    .clone();
    let context_mgr = ContextManager::new(run_dir.to_str().unwrap(), run_config, None);
    context_mgr.initialize();

    RunSetup {
        run_directory: run_dir.to_string_lossy().into_owned(),
        meta_agent_working_directory: gen1.to_string_lossy().into_owned(),
        venv_dir: root.join("venv").to_string_lossy().into_owned(),
        context_mgr,
    }
}

fn ok_target(
) -> impl Fn(&str, &str, &str, &str, &str, &str, &Config) -> (bool, String, String, String) {
    |_venv, _path, _ds, _gen, _log, _sandbox, _cfg| {
        (true, "output".to_string(), String::new(), String::new())
    }
}

#[test]
fn test_single_generation_creates_context() {
    let d = tempfile::tempdir().unwrap();
    let task_dir = make_task_files(d.path());
    let mut run_setup = make_run_setup(d.path(), &task_dir);
    let ds = task_dir
        .join("data")
        .join("public")
        .to_string_lossy()
        .into_owned();

    let fb_calls = Arc::new(AtomicUsize::new(0));
    let fb = fb_calls.clone();
    let mut feedback = move |_args: &FeedbackArgs| {
        fb.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };

    run_generation_with(
        &ok_target(),
        &mut feedback,
        1,
        1,
        &mut run_setup,
        &TaskFiles::new("desc", "ref", json!({}), "# Task"),
        &ds,
        &ds,
        "none",
        &Config::default(),
    )
    .unwrap();

    let ctx = std::fs::read_to_string(format!("{}/context.md", run_setup.run_directory)).unwrap();
    assert!(ctx.contains("Generation 1"));
    assert!(ctx.contains("SUCCESS"));
    assert_eq!(fb_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn test_run_generation_directory_structure() {
    let d = tempfile::tempdir().unwrap();
    let task_dir = make_task_files(d.path());
    let mut run_setup = make_run_setup(d.path(), &task_dir);

    let mut feedback = |_args: &FeedbackArgs| Ok(());
    run_generation_with(
        &ok_target(),
        &mut feedback,
        1,
        1,
        &mut run_setup,
        &TaskFiles::new("d", "r", json!({}), "# T"),
        "/data",
        "/data",
        "none",
        &Config::default(),
    )
    .unwrap();

    let gen_dir = std::path::Path::new(&run_setup.run_directory).join("gen_1");
    assert!(gen_dir.is_dir());
    assert!(gen_dir.join("target_agent.py").is_file());
}

#[test]
fn test_run_generation_propagates_requirements_install_failure() {
    let d = tempfile::tempdir().unwrap();
    let task_dir = make_task_files(d.path());
    let mut run_setup = make_run_setup(d.path(), &task_dir);
    let gen_dir = std::path::Path::new(&run_setup.run_directory).join("gen_1");
    std::fs::write(
        gen_dir.join("requirements.txt"),
        "definitely-not-a-real-package==0\n",
    )
    .unwrap();

    let target_calls = Arc::new(AtomicUsize::new(0));
    let calls = target_calls.clone();
    let target = move |_venv: &str,
                       _path: &str,
                       _ds: &str,
                       _gen: &str,
                       _log: &str,
                       _sandbox: &str,
                       _cfg: &Config| {
        calls.fetch_add(1, Ordering::SeqCst);
        (true, "output".to_string(), String::new(), String::new())
    };
    let mut feedback = |_args: &FeedbackArgs| Ok(());

    let result = run_generation_with(
        &target,
        &mut feedback,
        1,
        1,
        &mut run_setup,
        &TaskFiles::new("d", "r", json!({}), "# T"),
        "/data",
        "/data",
        "none",
        &Config::default(),
    );

    assert!(
        result.is_err(),
        "requirements install failure should propagate"
    );
    assert_eq!(
        target_calls.load(Ordering::SeqCst),
        0,
        "target agent should not run after dependency setup fails"
    );
}

#[test]
fn test_two_generations_with_feedback() {
    let d = tempfile::tempdir().unwrap();
    let task_dir = make_task_files(d.path());
    let mut run_setup = make_run_setup(d.path(), &task_dir);

    let fb_calls = Arc::new(AtomicUsize::new(0));
    let task_files = TaskFiles::new("d", "r", json!({}), "# T");

    // Generation 1 (should trigger feedback agent which creates gen_2 files).
    {
        let fb = fb_calls.clone();
        let mut feedback = move |args: &FeedbackArgs| {
            fb.fetch_add(1, Ordering::SeqCst);
            std::fs::create_dir_all(args.next_gen_dir).unwrap();
            std::fs::write(
                format!("{}/target_agent.py", args.next_gen_dir),
                "print('improved')\n",
            )
            .unwrap();
            std::fs::write(
                format!("{}/improvement.md", args.next_gen_dir),
                "- Better prompts\n- More robust error handling\n",
            )
            .unwrap();
            Ok(())
        };
        run_generation_with(
            &ok_target(),
            &mut feedback,
            1,
            2,
            &mut run_setup,
            &task_files,
            "/data",
            "/data",
            "none",
            &Config::default(),
        )
        .unwrap();
    }
    assert_eq!(fb_calls.load(Ordering::SeqCst), 1);

    // Generation 2 (last generation -> no feedback).
    {
        let fb = fb_calls.clone();
        let mut feedback = move |_args: &FeedbackArgs| {
            fb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        run_generation_with(
            &ok_target(),
            &mut feedback,
            2,
            2,
            &mut run_setup,
            &task_files,
            "/data",
            "/data",
            "none",
            &Config::default(),
        )
        .unwrap();
    }
    assert_eq!(fb_calls.load(Ordering::SeqCst), 1);

    let run_dir = std::path::Path::new(&run_setup.run_directory);
    assert!(run_dir.join("gen_1").join("target_agent.py").is_file());
    assert!(run_dir.join("gen_2").join("target_agent.py").is_file());

    let ctx = std::fs::read_to_string(run_dir.join("context.md")).unwrap();
    assert!(ctx.contains("Generation 1"));
    assert!(ctx.contains("Generation 2"));

    run_setup.context_mgr.finalize();
    let ctx_final = std::fs::read_to_string(run_dir.join("context.md")).unwrap();
    assert!(ctx_final.contains("Summary Statistics"));
    assert!(ctx_final.contains("**Total Generations**: 2"));
}

// --------------------------------------------------------------------------- //
// Issue #90: the scheduler decision drives the generation loop.
//
// These tests force a scheduler decision by pre-seeding per-generation
// `results.json` score histories (so `record_scheduler_decision` returns a
// decision) and assert that the loop ACTS on it: a `weight` decision skips the
// feedback seam and records a weight update; a `harness` decision still invokes
// feedback; and the acted decision is written back into scheduler_decision.json.
// --------------------------------------------------------------------------- //

/// Build a RunSetup whose run dir has `gen_0..=current_gen` directories, each
/// carrying a `results.json` with the matching accuracy from `scores`. The
/// current generation also gets `target_agent.py` and a single-trajectory
/// `agent_execution.json` so a weight update has examples to train on.
fn make_scheduler_run_setup(
    root: &std::path::Path,
    task_dir: &std::path::Path,
    scores: &[f64],
) -> (RunSetup, i64) {
    let run_dir = root.join("runs").join("run_sched");
    std::fs::create_dir_all(&run_dir).unwrap();

    for (i, &acc) in scores.iter().enumerate() {
        let gen_dir = run_dir.join(format!("gen_{i}"));
        std::fs::create_dir_all(&gen_dir).unwrap();
        std::fs::write(
            gen_dir.join("results.json"),
            json!({"accuracy": acc, "accuracy_percent": acc * 100.0,
                   "correct": (acc * 10.0) as i64, "total": 10})
            .to_string(),
        )
        .unwrap();
    }

    let current_gen = (scores.len() - 1) as i64;
    let gen_dir = run_dir.join(format!("gen_{current_gen}"));
    std::fs::write(gen_dir.join("target_agent.py"), "print('agent')\n").unwrap();
    std::fs::write(
        gen_dir.join("agent_execution.json"),
        json!([
            {"role": "user", "content": "what is 2+2?"},
            {"role": "assistant", "content": [{"type": "text", "text": "The answer is 4."}]},
            {"role": "user", "content": "and 3+3?"},
            {"role": "assistant", "content": [{"type": "text", "text": "Six."}]}
        ])
        .to_string(),
    )
    .unwrap();

    let run_config = json!({
        "task_dir": task_dir.to_string_lossy(),
        "meta_model": "haiku",
        "task_model": "haiku",
        "agent_impl": "claude",
        "max_gen": current_gen + 1,
    })
    .as_object()
    .unwrap()
    .clone();
    let context_mgr = ContextManager::new(run_dir.to_str().unwrap(), run_config, None);
    context_mgr.initialize();

    let run_setup = RunSetup {
        run_directory: run_dir.to_string_lossy().into_owned(),
        meta_agent_working_directory: gen_dir.to_string_lossy().into_owned(),
        venv_dir: root.join("venv").to_string_lossy().into_owned(),
        context_mgr,
    };
    (run_setup, current_gen)
}

#[test]
fn weight_decision_skips_feedback_and_records_weight_update() {
    let d = tempfile::tempdir().unwrap();
    let task_dir = make_task_files(d.path());
    // A plateaued harness history (early jump, then flat) makes the scheduler
    // decide `weight`: deltas below eps and >= min_harness_gens generations.
    let scores = [0.10, 0.60, 0.605, 0.606, 0.6065];
    let (mut run_setup, current_gen) = make_scheduler_run_setup(d.path(), &task_dir, &scores);

    let fb_calls = Arc::new(AtomicUsize::new(0));
    let fb = fb_calls.clone();
    let mut feedback = move |_args: &FeedbackArgs| {
        fb.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };

    run_generation_with(
        &ok_target(),
        &mut feedback,
        current_gen,
        current_gen + 1, // not the last gen, so feedback WOULD run on the harness path
        &mut run_setup,
        &TaskFiles::new("d", "r", json!({}), "# T"),
        "/data",
        "/data",
        "none",
        &Config::default(),
    )
    .unwrap();

    // A `weight` decision short-circuits the feedback (harness) step.
    assert_eq!(
        fb_calls.load(Ordering::SeqCst),
        0,
        "weight decision must skip the feedback seam"
    );

    let gen_dir = std::path::Path::new(&run_setup.run_directory).join(format!("gen_{current_gen}"));

    // The decision artifact records BOTH the recommendation and what we acted on.
    let decision: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(gen_dir.join("scheduler_decision.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(decision["decision"], json!("weight"));
    assert_eq!(decision["acted"], json!("weight"));
    assert_eq!(decision["weight_ran"], json!(true));
    assert_eq!(decision["harness_ran"], json!(false));
    assert!(decision["weight_update"]["updated"].as_bool().unwrap());

    // And a weight_update.json artifact was written with >= 1 example.
    let weight: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(gen_dir.join("weight_update.json")).unwrap())
            .unwrap();
    assert_eq!(weight["kind"], json!("weight"));
    assert!(weight["num_examples"].as_i64().unwrap() >= 1);
}

#[test]
fn harness_decision_still_calls_feedback_and_records_acted() {
    let d = tempfile::tempdir().unwrap();
    let task_dir = make_task_files(d.path());
    // A steadily improving history (deltas well above eps) decides `harness`.
    let scores = [0.10, 0.30, 0.55];
    let (mut run_setup, current_gen) = make_scheduler_run_setup(d.path(), &task_dir, &scores);

    let fb_calls = Arc::new(AtomicUsize::new(0));
    let fb = fb_calls.clone();
    let mut feedback = move |_args: &FeedbackArgs| {
        fb.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };

    run_generation_with(
        &ok_target(),
        &mut feedback,
        current_gen,
        current_gen + 1, // not the last gen -> feedback should run on the harness path
        &mut run_setup,
        &TaskFiles::new("d", "r", json!({}), "# T"),
        "/data",
        "/data",
        "none",
        &Config::default(),
    )
    .unwrap();

    // A `harness` decision keeps today's behavior: the feedback agent runs once.
    assert_eq!(
        fb_calls.load(Ordering::SeqCst),
        1,
        "harness decision must call the feedback seam exactly once"
    );

    let gen_dir = std::path::Path::new(&run_setup.run_directory).join(format!("gen_{current_gen}"));
    let decision: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(gen_dir.join("scheduler_decision.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(decision["decision"], json!("harness"));
    assert_eq!(decision["acted"], json!("harness"));
    assert_eq!(decision["harness_ran"], json!(true));
    assert_eq!(decision["weight_ran"], json!(false));
    // No weight update ran on the harness path.
    assert!(decision["weight_update"].is_null());
    assert!(!gen_dir.join("weight_update.json").exists());
}

#[test]
fn test_run_feedback_agent_propagates_reference_copy_failure() {
    let d = tempfile::tempdir().unwrap();
    let run_dir = d.path().join("runs").join("run_1");
    let gen1 = run_dir.join("gen_1");
    std::fs::create_dir_all(&gen1).unwrap();
    std::fs::write(gen1.join("target_agent.py"), "print('agent')\n").unwrap();
    let dataset_dir = d.path().join("task").join("data").join("public");
    std::fs::create_dir_all(&dataset_dir).unwrap();
    std::fs::write(dataset_dir.join("task.md"), "# Task").unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    register(
        "reference-copy-failure-test",
        Arc::new(move |_args| {
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    let provider = load_provider("anthropic").unwrap();
    let meta_profile = MetaAgentProfile {
        profile_id: "test-meta".to_string(),
        name: "Test Meta".to_string(),
        agent_impl: "reference-copy-failure-test".to_string(),
        model: "test-model".to_string(),
        provider: provider.clone(),
    };
    let missing_ref = ResolvedAgentReference {
        inline_seed: None,
        ref_dir: Some(d.path().join("missing-reference-dir")),
        entrypoint: "target_agent.py".to_string(),
        requirements: None,
    };
    let next_gen_dir = run_dir.join("gen_2");
    let args = FeedbackArgs {
        current_gen: 1,
        max_gen: 2,
        run_dir: run_dir.to_str().unwrap(),
        next_gen_dir: next_gen_dir.to_str().unwrap(),
        execution_status: "FAILED",
        execution_section: "section",
    };

    let result = run_feedback_agent(
        &args,
        &TaskFiles::new("d", "r", json!({}), "# T"),
        &meta_profile,
        &Config::default(),
        dataset_dir.to_str().unwrap(),
        "task-model",
        &provider,
        Some(&missing_ref),
    );

    assert!(result.is_err(), "reference-copy failure should propagate");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "feedback agent should not run after reference copy fails"
    );
}
