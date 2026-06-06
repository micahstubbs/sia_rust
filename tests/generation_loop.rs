//! Integration tests for the generation loop with injected agent seams.
//! Rust port of `tests/test_generation_loop.py` (the run_generation cases).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use sia::config::Config;
use sia::context_manager::ContextManager;
use sia::orchestrator::{run_generation_with, FeedbackArgs};
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
