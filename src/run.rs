//! Top-level `run` / `web` dispatch for the `sia` binary. Port of `sia.orchestrator.main`.

use std::path::Path;

use clap::ArgMatches;

use crate::agent_impls::run_agent;
use crate::agent_reference::{copy_reference_into, resolve_agent_reference};
use crate::config::Config;
use crate::error::SiaResult;
use crate::layout::{names, resolve_task_dir, RunLayout, TaskLayout};
use crate::orchestrator::{
    run_feedback_agent, run_generation_with, run_target_agent, FeedbackArgs,
};
use crate::profiles::{load_meta_agent_profile, load_target_agent_profile};
use crate::prompts::build_meta_prompt;
use crate::run_setup::{load_task_files, setup_run_directory};

fn opt_str<'a>(m: &'a ArgMatches, key: &str) -> Option<&'a str> {
    m.get_one::<String>(key).map(|s| s.as_str())
}

/// `sia web`: serve the runs visualizer (blocks).
pub fn run_web(args: &ArgMatches) -> SiaResult<()> {
    let host = opt_str(args, "host").unwrap_or("127.0.0.1");
    let port = *args.get_one::<u16>("port").unwrap_or(&8000);
    let runs_dir = opt_str(args, "runs_dir").unwrap_or(names::RUNS_ROOT);
    let no_browser = args.get_flag("no_browser");
    crate::web::serve(host, port, runs_dir, !no_browser)
}

/// `sia run`: the self-improvement loop.
///
/// Mirrors `sia.orchestrator.main`. Everything up to the meta-agent call is wired and
/// functional: task resolution, profile/provider loading, run-directory + venv setup,
/// the meta prompt, and the per-generation scaffolding (target-agent subprocess
/// execution, evaluation, context tracking, feedback context). The **meta/feedback
/// agents** are dispatched through the agent-impl registry; with `--features llm` the
/// native runners drive the meta/feedback agents (issues #39–#41). Without that
/// feature the default build stops with a clear feature-gate error at the first LLM
/// call. `sia web` is fully functional today.
pub fn run_orchestrator(args: &ArgMatches, env_config: &Config) -> SiaResult<()> {
    let max_gen = *args
        .get_one::<i64>("max_gen")
        .unwrap_or(&env_config.default_max_generations);
    let run_id = *args.get_one::<i64>("run_id").unwrap_or(&1);
    let sandbox = opt_str(args, "sandbox")
        .unwrap_or(&env_config.sandbox_mode)
        .to_string();

    let (task_dir, shared_dir) =
        resolve_task_dir(opt_str(args, "task"), opt_str(args, "task_dir"))?;

    // Live dashboard in the background unless disabled.
    if !args.get_flag("no_web") {
        let web_host = opt_str(args, "web_host").unwrap_or("127.0.0.1");
        let web_port = *args.get_one::<u16>("web_port").unwrap_or(&8000);
        crate::web::serve_in_background(web_host, web_port, names::RUNS_ROOT);
    }

    let meta_profile =
        load_meta_agent_profile(opt_str(args, "meta_agent_profile").unwrap_or("default-meta"))?;
    let target_profile = load_target_agent_profile(
        opt_str(args, "target_agent_profile").unwrap_or("default-target"),
    )?;
    let meta_model = meta_profile.model.clone();
    let task_model = target_profile.model.clone();
    let agent_impl = meta_profile.agent_impl.clone();
    let target_provider = target_profile.provider.clone();

    let task_layout = TaskLayout::new(task_dir.clone(), shared_dir.clone());
    let resolved_ref = resolve_agent_reference(&target_profile.agent_reference, &task_layout)?;

    println!("Configuration:");
    println!("  - Maximum generations: {max_gen}");
    println!("  - Task directory: {task_dir}");
    println!("  - Run ID: {run_id}");
    println!(
        "  - Meta agent profile: {} (agent_impl={agent_impl}, model={meta_model})",
        meta_profile.profile_id
    );
    println!(
        "  - Target agent profile: {} (model={task_model}, reference={})",
        target_profile.profile_id, target_profile.agent_reference.kind
    );

    for (label, prov) in [
        ("meta", &meta_profile.provider),
        ("target", &target_provider),
    ] {
        if std::env::var(&prov.api_key_env).is_err() {
            eprintln!(
                "  ⚠ {} is not set; the {label} agent may fail to authenticate.",
                prov.api_key_env
            );
        }
    }

    // Section 1: load task files.
    let task_files = load_task_files(&task_dir, &shared_dir, Some(&resolved_ref))?;

    // Section 2: setup run directory.
    let mut run_setup = setup_run_directory(
        run_id,
        &task_dir,
        &meta_model,
        &task_model,
        &agent_impl,
        max_gen,
        Some(env_config.clone()),
        Some(&meta_profile),
        Some(&target_profile),
    )?;

    // Section 3: build the initial meta prompt.
    copy_reference_into(
        &resolved_ref,
        Path::new(&run_setup.meta_agent_working_directory),
    )
    .ok();
    let reference_dir = if resolved_ref.ref_dir.is_some() {
        Some(run_setup.meta_agent_working_directory.clone())
    } else {
        None
    };
    let meta_agent_prompt = build_meta_prompt(
        &task_files,
        &task_model,
        &run_setup.meta_agent_working_directory,
        Some(&target_provider),
        reference_dir.as_deref(),
    );

    // Section 4: run the meta agent.
    let meta_prompt_path = format!(
        "{}/{}",
        run_setup.meta_agent_working_directory,
        names::META_PROMPT
    );
    let _ = std::fs::write(&meta_prompt_path, &meta_agent_prompt);
    run_agent(
        &meta_model,
        &env_config.default_max_turns.to_string(),
        &meta_agent_prompt,
        &run_setup.meta_agent_working_directory,
        &agent_impl,
        Some(meta_profile.provider.clone()),
    )?;

    // Section 5: generation loop.
    let dataset_directory = task_layout.dataset_dir();
    let abs_dataset_directory = task_layout.abs_dataset_dir();

    for current_gen in 1..=max_gen {
        println!("Starting Generation {current_gen} of {max_gen}");

        let target_fn =
            |venv: &str, path: &str, abs_ds: &str, gen: &str, log: &str, sb: &str, cfg: &Config| {
                run_target_agent(venv, path, abs_ds, gen, log, sb, cfg)
            };
        let mut feedback_fn = |fargs: &FeedbackArgs| {
            run_feedback_agent(
                fargs,
                &task_files,
                &meta_profile,
                env_config,
                &dataset_directory,
                &task_model,
                &target_provider,
                Some(&resolved_ref),
            )
        };

        run_generation_with(
            &target_fn,
            &mut feedback_fn,
            current_gen,
            max_gen,
            &mut run_setup,
            &task_files,
            &abs_dataset_directory,
            &dataset_directory,
            &sandbox,
            env_config,
        )?;
    }

    run_setup.context_mgr.finalize();
    let _ = RunLayout::new(run_setup.run_directory.clone());
    println!(
        "Orchestrator completed all {max_gen} generations. Results in: {}",
        run_setup.run_directory
    );
    Ok(())
}
