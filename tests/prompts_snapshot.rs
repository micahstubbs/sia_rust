//! Characterization: lock the exact text of the meta and feedback prompts.
//! Rust port of `tests/test_prompts_snapshot.py`.

mod common;

use sia::prompts::{build_feedback_prompt, build_meta_prompt};
use sia::providers::load_provider;
use sia::TaskFiles;

fn task_files() -> TaskFiles {
    TaskFiles::new(
        "SAMPLE DESCRIPTIONS BODY",
        "print('reference target agent')",
        serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
        "# Example Task\nSolve the example problem precisely.",
    )
}

#[test]
fn test_meta_prompt_golden() {
    let prompt = build_meta_prompt(
        &task_files(),
        "claude-haiku-4-5-20251001",
        "/WORK/run_1/gen_1",
        None,
        None,
    );
    common::assert_golden("meta_prompt.txt", &prompt);
}

#[test]
fn test_meta_prompt_anthropic_provider_is_byte_identical() {
    let anthropic = load_provider("anthropic").unwrap();
    let prompt = build_meta_prompt(
        &task_files(),
        "claude-haiku-4-5-20251001",
        "/WORK/run_1/gen_1",
        Some(&anthropic),
        None,
    );
    common::assert_golden("meta_prompt.txt", &prompt);
}

#[test]
fn test_meta_prompt_openai_provider_golden() {
    let nebius = load_provider("nebius").unwrap();
    let prompt = build_meta_prompt(
        &task_files(),
        "moonshotai/Kimi-K2.6",
        "/WORK/run_1/gen_1",
        Some(&nebius),
        None,
    );
    common::assert_golden("meta_prompt_openai.txt", &prompt);
}

#[test]
fn test_feedback_prompt_golden() {
    let prompt = build_feedback_prompt(
        2,
        3,
        &task_files(),
        "print('current target agent gen 2')",
        "# Example Task\nSolve the example problem precisely.",
        "SUCCESS: example status block",
        "EXECUTION SECTION BODY",
        "/RUN/run_1",
        "/RUN/run_1/gen_3",
        "1",
        "claude-haiku-4-5-20251001",
        None,
        None,
    );
    common::assert_golden("feedback_prompt.txt", &prompt);
}
