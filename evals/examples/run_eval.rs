//! Run the GPQA-style eval end to end and print an `evaluate.py`-style report.
//!
//! Offline (default, no network, no keys):
//!   cargo run --example run_eval
//!
//! Against a real provider (requires the matching API key in the environment):
//!   EVAL_MODEL=openai:gpt-4o-mini   OPENAI_API_KEY=sk-...    cargo run --example run_eval -- --real
//!   EVAL_MODEL=anthropic:claude-3-5-sonnet-latest ANTHROPIC_API_KEY=sk-... cargo run --example run_eval -- --real

use dspy_rs::{configure, ChatAdapter, Module};
use sia_evals::{load_fixtures, offline_lm, real_lm, score, GpqaModule, GpqaQuestion, MockAdapter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let real = std::env::args().any(|a| a == "--real");

    let questions = load_fixtures()?;
    let examples: Vec<_> = questions.iter().map(GpqaQuestion::to_example).collect();

    if real {
        let model =
            std::env::var("EVAL_MODEL").unwrap_or_else(|_| "openai:gpt-4o-mini".to_string());
        eprintln!("Running against real provider: {model}");
        configure(real_lm(&model).await?, ChatAdapter);
    } else {
        eprintln!("Running OFFLINE with the deterministic mock adapter (no network).");
        // Swap to `MockAdapter::always("A")` to see a lower score.
        configure(offline_lm().await?, MockAdapter::perfect(&questions));
    }

    let module = GpqaModule::new();
    let predictions = module.batch(examples.clone(), 8, false).await?;
    let report = score(&examples, &predictions);

    println!("======================================================================");
    println!("GPQA-style Eval Results (dspy-rs)");
    println!("======================================================================");
    println!("Total Questions:    {}", report.total);
    println!("Correct:            {}", report.correct);
    println!("Incorrect:          {}", report.incorrect);
    println!("Invalid:            {}", report.invalid);
    println!("Accuracy:           {:.2}%", report.accuracy_percent);
    println!("======================================================================");

    Ok(())
}
