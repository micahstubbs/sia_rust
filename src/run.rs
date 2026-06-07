//! Top-level `run` / `web` dispatch for the `sia` binary. Port of `sia.orchestrator.main`.

use std::path::Path;

use clap::ArgMatches;

use crate::agent_impls::run_agent;
use crate::agent_reference::{copy_reference_into, resolve_agent_reference};
use crate::config::Config;
use crate::error::{SiaError, SiaResult};
use crate::layout::{names, resolve_task_dir, RunLayout, TaskLayout};
use crate::orchestrator::{run_feedback_agent, run_generation_with, FeedbackArgs};
use crate::profiles::{load_meta_agent_profile, load_target_agent_profile};
use crate::prompts::build_meta_prompt;
use crate::run_setup::{load_task_files, setup_run_directory};
use crate::target_exec::{target_fn_for, PythonVenvExecutor};

fn opt_str<'a>(m: &'a ArgMatches, key: &str) -> Option<&'a str> {
    m.get_one::<String>(key).map(|s| s.as_str())
}

/// Resolve the runs root for `sia run`: `--runs-dir` flag wins, then the
/// `SIA_RUNS_DIR` env var, then the `./runs` default (flag > env > default).
///
/// This is intentionally resolved in the CLI/run layer rather than
/// `Config::from_env` so the parity env map stays byte-for-byte aligned with the
/// Python reference.
pub fn resolve_runs_dir(flag: Option<&str>) -> String {
    if let Some(flag) = flag {
        return flag.to_string();
    }
    match std::env::var("SIA_RUNS_DIR") {
        Ok(v) if !v.is_empty() => v,
        _ => names::RUNS_ROOT.to_string(),
    }
}

/// Resolve the `--run_id` argument to a concrete numeric run id.
///
/// `"auto"` (case-insensitive) scans `runs_root` for existing `run_<n>` directories
/// and returns `max(n) + 1`, or `1` when there are none. This makes rehearsals and
/// restarts collision-proof: each `auto` run lands in a fresh directory rather than
/// erroring on an existing one. Any other value must parse as a positive integer and
/// is returned as-is (preserving the historical numeric behavior, including the
/// existing-directory collision error in `setup_run_directory`).
pub fn resolve_run_id(arg: &str, runs_root: &str) -> SiaResult<i64> {
    if arg.eq_ignore_ascii_case("auto") {
        return Ok(next_free_run_id(runs_root));
    }
    match arg.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(SiaError::new(format!(
            "Invalid --run_id '{arg}': expected a positive integer or 'auto'"
        ))),
    }
}

/// Scan `runs_root` for `run_<n>` directories and return the next free id
/// (`max(n) + 1`, or `1` if the root is missing/empty). Non-`run_<n>` entries and
/// entries with non-numeric suffixes are ignored.
fn next_free_run_id(runs_root: &str) -> i64 {
    let mut max_id: i64 = 0;
    if let Ok(entries) = std::fs::read_dir(runs_root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(suffix) = name.strip_prefix("run_") {
                if let Ok(n) = suffix.parse::<i64>() {
                    if n > max_id {
                        max_id = n;
                    }
                }
            }
        }
    }
    max_id + 1
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
    let sandbox = opt_str(args, "sandbox")
        .unwrap_or(&env_config.sandbox_mode)
        .to_string();

    let (task_dir, shared_dir) =
        resolve_task_dir(opt_str(args, "task"), opt_str(args, "task_dir"))?;

    // Resolve the runs root first, then resolve `--run_id` (which may be `auto`)
    // against that same root so the auto-scan and the directory the run writes to
    // agree (honors `--runs-dir` / `SIA_RUNS_DIR`).
    let runs_dir = resolve_runs_dir(opt_str(args, "runs_dir"));
    let run_id = resolve_run_id(opt_str(args, "run_id").unwrap_or("1"), &runs_dir)?;

    // Surface the resolved run directory up front, before any expensive LLM work,
    // so a presenter immediately sees where artifacts will land.
    let resolved_run_dir = RunLayout::for_run_id(run_id, &runs_dir).run_dir;
    println!("Run directory: {resolved_run_dir}");

    // Live dashboard in the background unless disabled. It serves exactly the
    // directory the run writes to (`runs_dir`). The listener is bound up front so
    // we only announce the URL after a successful bind, and the printed URL always
    // reflects the port we actually bound to (which may differ from the default
    // when 8000 is occupied).
    if !args.get_flag("no_web") {
        let web_host = opt_str(args, "web_host").unwrap_or("127.0.0.1");
        let web_port = *args.get_one::<u16>("web_port").unwrap_or(&8000);
        // Did the user explicitly pass --web-port? If so, treat a bind failure as
        // a hard error; otherwise auto-select a fallback port.
        let explicit_port = matches!(
            args.value_source("web_port"),
            Some(clap::parser::ValueSource::CommandLine)
        );
        match crate::web::serve_in_background(web_host, web_port, &runs_dir, explicit_port) {
            Ok(dashboard) => {
                println!("Live dashboard: http://{web_host}:{}", dashboard.port);
            }
            Err(e) if explicit_port => {
                return Err(e);
            }
            Err(e) => {
                // Default port path that exhausted all fallbacks: warn but keep running.
                eprintln!("Warning: live dashboard unavailable: {e}");
            }
        }
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
        &runs_dir,
    )?;

    // Section 3: build the initial meta prompt.
    copy_reference_into(
        &resolved_ref,
        Path::new(&run_setup.meta_agent_working_directory),
    )
    .map_err(|e| {
        SiaError::new(format!(
            "failed to copy agent reference into {}: {e}",
            run_setup.meta_agent_working_directory
        ))
    })?;
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

        // Route target-agent execution through the `TargetExecutor` seam (#138).
        // The default strategy is `PythonVenvExecutor`, which delegates to the
        // existing `run_target_agent` (plain + Docker paths) — behavior unchanged.
        let target_fn = target_fn_for(&PythonVenvExecutor);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_run_id_passes_through() {
        // A custom runs root with no entries does not affect numeric ids.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        assert_eq!(resolve_run_id("1", root).unwrap(), 1);
        assert_eq!(resolve_run_id("7", root).unwrap(), 7);
    }

    #[test]
    fn invalid_run_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        assert!(resolve_run_id("0", root).is_err());
        assert!(resolve_run_id("-1", root).is_err());
        assert!(resolve_run_id("nope", root).is_err());
    }

    #[test]
    fn auto_picks_one_when_root_empty_or_missing() {
        // Missing root.
        assert_eq!(resolve_run_id("auto", "/no/such/runs/root").unwrap(), 1);
        // Existing-but-empty root.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        assert_eq!(resolve_run_id("auto", root).unwrap(), 1);
        assert_eq!(resolve_run_id("AUTO", root).unwrap(), 1);
    }

    #[test]
    fn auto_picks_next_free_id() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("run_1")).unwrap();
        assert_eq!(
            resolve_run_id("auto", root.to_str().unwrap()).unwrap(),
            2,
            "auto should pick run_2 when run_1 exists"
        );

        // Gaps and non-run entries are ignored; auto uses max + 1.
        std::fs::create_dir(root.join("run_5")).unwrap();
        std::fs::create_dir(root.join("not_a_run")).unwrap();
        std::fs::create_dir(root.join("run_abc")).unwrap();
        assert_eq!(resolve_run_id("auto", root.to_str().unwrap()).unwrap(), 6);
    }

    #[test]
    fn auto_resolves_under_custom_runs_dir() {
        // Two independent roots: auto resolves against whichever root it is given.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::create_dir(a.path().join("run_3")).unwrap();
        // Root `a` has run_3 -> auto picks 4; root `b` is empty -> auto picks 1.
        assert_eq!(
            resolve_run_id("auto", a.path().to_str().unwrap()).unwrap(),
            4
        );
        assert_eq!(
            resolve_run_id("auto", b.path().to_str().unwrap()).unwrap(),
            1
        );
    }
}
