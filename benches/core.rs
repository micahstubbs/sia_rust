//! Criterion benchmarks for the deterministic SIA core (issue #31).
//!
//! `harness = false` (see Cargo.toml) means this file owns `main`; Criterion
//! generates it via `criterion_main!`. Each bench measures the same operation as
//! the matching entry in `benchmarks/bench_python.py`, using identical fixtures,
//! so `benchmarks/run_comparison.py` can align the two by bench id.
//!
//! Bench ids (must match the Python script):
//!   build_meta_prompt, build_feedback_prompt, context_manager_run,
//!   build_feedback_context_single, build_feedback_context_multi,
//!   load_agent_execution_single, load_agent_execution_multi,
//!   web_list_runs, web_get_run

use std::path::{Path, PathBuf};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::json;

use sia::config::Config;
use sia::context_manager::{ContextManager, GenData};
use sia::orchestrator::{build_feedback_context, load_agent_execution};
use sia::web::runs as rd;
use sia::TaskFiles;

// --------------------------------------------------------------------------- //
// Shared fixtures
// --------------------------------------------------------------------------- //

fn task_files() -> TaskFiles {
    TaskFiles::new(
        "SAMPLE DESCRIPTIONS BODY",
        "print('reference target agent')",
        json!({"messages": [{"role": "user", "content": "hi"}]}),
        "# Example Task\nSolve the example problem precisely.",
    )
}

/// TaskFiles matching tests/feedback_context_golden.rs for the feedback-context bench.
fn feedback_task_files() -> TaskFiles {
    TaskFiles::new("desc", "ref", json!({}), "# Task")
}

// --------------------------------------------------------------------------- //
// Prompt benches (pure, no filesystem)
// --------------------------------------------------------------------------- //

fn bench_build_meta_prompt(c: &mut Criterion) {
    let tf = task_files();
    c.bench_function("build_meta_prompt", |b| {
        b.iter(|| {
            sia::prompts::build_meta_prompt(
                black_box(&tf),
                black_box("claude-haiku-4-5-20251001"),
                black_box("/WORK/run_1/gen_1"),
                None,
                None,
            )
        })
    });
}

fn bench_build_feedback_prompt(c: &mut Criterion) {
    let tf = task_files();
    c.bench_function("build_feedback_prompt", |b| {
        b.iter(|| {
            sia::prompts::build_feedback_prompt(
                black_box(2),
                black_box(3),
                black_box(&tf),
                black_box("print('current target agent gen 2')"),
                black_box("# Example Task\nSolve the example problem precisely."),
                black_box("SUCCESS: example status block"),
                black_box("EXECUTION SECTION BODY"),
                black_box("/RUN/run_1"),
                black_box("/RUN/run_1/gen_3"),
                black_box("1"),
                black_box("claude-haiku-4-5-20251001"),
                None,
                None,
            )
        })
    });
}

// --------------------------------------------------------------------------- //
// context_manager_run: initialize + add_generation x2 + finalize
// --------------------------------------------------------------------------- //

const GEN1_AGENT: &str = "print('gen 1 agent')\n";
const GEN2_AGENT: &str =
    "import sys\n\n\ndef main():\n    print('gen 2 agent, improved')\n\n\nmain()\n";
const IMPROVEMENT_MD: &str = "# Improvement Plan\n\n\
- Added structured error handling so the agent recovers from tool failures gracefully.\n\
- Switched to a retry loop with exponential backoff for transient API errors.\n\
- Improved logging to capture each tool call and its result for later analysis.\n";

/// Build a fresh run dir with two populated generations. Returns the run dir and
/// the two gen dirs so the bench body can drive a ContextManager over them.
fn setup_context_run(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let run_dir = base.join("run_1");
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
    (run_dir, gen1, gen2)
}

fn bench_context_manager_run(c: &mut Criterion) {
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

    c.bench_function("context_manager_run", |b| {
        b.iter_batched(
            // setup: fresh tempdir + fixtures each iteration (excluded from timing)
            || {
                let d = tempfile::tempdir().unwrap();
                let (run_dir, gen1, gen2) = setup_context_run(d.path());
                (d, run_dir, gen1, gen2)
            },
            |(_d, run_dir, gen1, gen2)| {
                let mut cm = ContextManager::new(run_dir.to_str().unwrap(), config.clone(), None);
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
                        improvement_path: Some(
                            gen2.join("improvement.md").to_string_lossy().into_owned(),
                        ),
                        execution_type: "Single".to_string(),
                    },
                );
                cm.finalize();
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

// --------------------------------------------------------------------------- //
// build_feedback_context (single + multi)
// --------------------------------------------------------------------------- //

fn setup_feedback_single(base: &Path) -> (PathBuf, String) {
    let gen_dir = base.join("gen_1");
    std::fs::create_dir_all(&gen_dir).unwrap();
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
    (gen_dir, stdout_log)
}

fn setup_feedback_multi(base: &Path) -> (PathBuf, String) {
    let gen_dir = base.join("gen_1");
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
    (gen_dir, stdout_log)
}

fn bench_build_feedback_context_single(c: &mut Criterion) {
    let d = tempfile::tempdir().unwrap();
    let (gen_dir, stdout_log) = setup_feedback_single(d.path());
    let tf = feedback_task_files();
    let cfg = Config::default();
    let gen_dir = gen_dir.to_str().unwrap().to_string();
    c.bench_function("build_feedback_context_single", |b| {
        b.iter(|| {
            build_feedback_context(
                black_box(1),
                black_box(&gen_dir),
                black_box("/data/public"),
                black_box(true),
                black_box(""),
                black_box("line1\nline2\nline3\n"),
                black_box(""),
                black_box(&stdout_log),
                black_box(&tf),
                black_box(&cfg),
            )
        })
    });
}

fn bench_build_feedback_context_multi(c: &mut Criterion) {
    let d = tempfile::tempdir().unwrap();
    let (gen_dir, stdout_log) = setup_feedback_multi(d.path());
    let tf = feedback_task_files();
    let cfg = Config::default();
    let gen_dir = gen_dir.to_str().unwrap().to_string();
    c.bench_function("build_feedback_context_multi", |b| {
        b.iter(|| {
            build_feedback_context(
                black_box(1),
                black_box(&gen_dir),
                black_box("/data/public"),
                black_box(true),
                black_box(""),
                black_box("processing q0\nprocessing q1\ndone\n"),
                black_box(""),
                black_box(&stdout_log),
                black_box(&tf),
                black_box(&cfg),
            )
        })
    });
}

// --------------------------------------------------------------------------- //
// load_agent_execution (single + multi)
// --------------------------------------------------------------------------- //

fn bench_load_agent_execution_single(c: &mut Criterion) {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("agent_execution.json"),
        json!([{"role": "user", "content": "hello"}]).to_string(),
    )
    .unwrap();
    let cfg = Config::default();
    let gen_dir = d.path().to_str().unwrap().to_string();
    c.bench_function("load_agent_execution_single", |b| {
        b.iter(|| load_agent_execution(black_box(&gen_dir), black_box(&cfg)))
    });
}

fn bench_load_agent_execution_multi(c: &mut Criterion) {
    let d = tempfile::tempdir().unwrap();
    let exec = d.path().join("agent_execution");
    std::fs::create_dir(&exec).unwrap();
    for i in 0..3 {
        std::fs::write(
            exec.join(format!("execution_q{i}.json")),
            json!([{"role": "user", "content": format!("question {i}")}]).to_string(),
        )
        .unwrap();
    }
    let cfg = Config::default();
    let gen_dir = d.path().to_str().unwrap().to_string();
    c.bench_function("load_agent_execution_multi", |b| {
        b.iter(|| load_agent_execution(black_box(&gen_dir), black_box(&cfg)))
    });
}

// --------------------------------------------------------------------------- //
// web_list_runs / web_get_run over a synthesized runs/ tree
// --------------------------------------------------------------------------- //

/// Build a runs/ tree with `n_runs` runs x `n_gens` generations, each gen carrying
/// target_agent.py, evaluation_results.json (with details), context.md and a
/// multi-trajectory agent_execution folder.
fn make_runs_tree(base: &Path, n_runs: usize, n_gens: usize) -> PathBuf {
    let root = base.join("runs");
    for r in 1..=n_runs {
        let run_dir = root.join(format!("run_{r}"));
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("context.md"),
            format!(
                "# Run Context: run_{r}\n\n\
**Task**: /tasks/gpqa\n\
**Meta Model**: kimi\n\
**Task Model**: haiku\n\
**Agent impl**: openhands\n\
**Started**: 2026-06-05 13:31:32\n\
**Max Generations**: {n_gens}\n\n\
---\n\n## Generation 1\n**Status**: ok\n"
            ),
        )
        .unwrap();
        for g in 1..=n_gens {
            let gen = run_dir.join(format!("gen_{g}"));
            let exec = gen.join("agent_execution");
            std::fs::create_dir_all(&exec).unwrap();
            std::fs::write(gen.join("target_agent.py"), "print('hello')\n").unwrap();
            std::fs::write(gen.join("meta_agent_prompt.txt"), "meta prompt body").unwrap();
            std::fs::write(gen.join("context.md"), "gen context\n").unwrap();
            std::fs::write(
                gen.join("evaluation_results.json"),
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
            for q in 1..=3 {
                std::fs::write(
                    exec.join(format!("execution_q{q}.json")),
                    json!([
                        {"role": "system", "content": [{"type": "text", "text": "You are an expert."}]},
                        {"role": "user", "content": format!("Question {q}?")},
                        {"role": "assistant", "content": [{"type": "text", "text": "Answer: A"}]},
                    ])
                    .to_string(),
                )
                .unwrap();
            }
        }
    }
    root
}

fn bench_web_list_runs(c: &mut Criterion) {
    let d = tempfile::tempdir().unwrap();
    let root = make_runs_tree(d.path(), 10, 3);
    c.bench_function("web_list_runs", |b| {
        b.iter(|| rd::list_runs(black_box(&root)))
    });
}

fn bench_web_get_run(c: &mut Criterion) {
    let d = tempfile::tempdir().unwrap();
    let root = make_runs_tree(d.path(), 10, 3);
    c.bench_function("web_get_run", |b| {
        b.iter(|| rd::get_run(black_box(&root), black_box("run_5")))
    });
}

criterion_group!(
    benches,
    bench_build_meta_prompt,
    bench_build_feedback_prompt,
    bench_context_manager_run,
    bench_build_feedback_context_single,
    bench_build_feedback_context_multi,
    bench_load_agent_execution_single,
    bench_load_agent_execution_multi,
    bench_web_list_runs,
    bench_web_get_run,
);
criterion_main!(benches);
