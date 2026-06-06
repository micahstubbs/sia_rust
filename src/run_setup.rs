//! Run/task setup: load task reference files and create the run directory.
//! Port of `sia/run_setup.py`.

use std::path::Path;
use std::process::Command;

use serde_json::json;

use crate::agent_reference::ResolvedAgentReference;
use crate::config::Config;
use crate::context_manager::ContextManager;
use crate::error::{SiaError, SiaResult};
use crate::layout::{venv_pip_path, venv_python_path, RunLayout, TaskLayout};
use crate::profiles::{MetaAgentProfile, TargetAgentProfile};
use crate::task_files::TaskFiles;

/// Container for run directory paths and managers.
pub struct RunSetup {
    pub run_directory: String,
    pub meta_agent_working_directory: String,
    pub venv_dir: String,
    pub context_mgr: ContextManager,
}

/// Load all reference files from the task directory.
pub fn load_task_files(
    task_dir: &str,
    shared_dir: &str,
    resolved_ref: Option<&ResolvedAgentReference>,
) -> SiaResult<TaskFiles> {
    let paths = TaskLayout::new(task_dir, shared_dir);

    let sample_task_descriptions = std::fs::read_to_string(paths.sample_descriptions())
        .map_err(|e| SiaError::new(format!("Could not read sample descriptions: {e}")))?;

    let reference_target_agent_py = match resolved_ref {
        None => std::fs::read_to_string(paths.reference_agent())
            .map_err(|e| SiaError::new(format!("Could not read reference agent: {e}")))?,
        Some(r) => r.inline_seed.clone().unwrap_or_default(),
    };

    let sample_text = std::fs::read_to_string(paths.sample_execution())
        .map_err(|e| SiaError::new(format!("Could not read sample execution: {e}")))?;
    let sample_agent_execution: serde_json::Value = serde_json::from_str(&sample_text)
        .map_err(|e| SiaError::new(format!("Invalid sample execution JSON: {e}")))?;

    let task_md = std::fs::read_to_string(paths.task_md())
        .map_err(|e| SiaError::new(format!("Could not read task.md: {e}")))?;

    Ok(TaskFiles {
        sample_task_descriptions,
        reference_target_agent_py,
        sample_agent_execution,
        task_md,
    })
}

fn uv_available() -> bool {
    which("uv")
}

fn which(program: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join(program).is_file() {
                return true;
            }
        }
    }
    false
}

/// Create a virtual environment and install packages.
fn create_venv(venv_dir: &str, packages: &[&str]) -> SiaResult<()> {
    let status = if uv_available() {
        let venv_status = Command::new("uv")
            .args(["venv", venv_dir])
            .status()
            .map_err(|e| SiaError::new(format!("uv venv failed: {e}")))?;
        if !venv_status.success() {
            return Err(SiaError::new(
                "uv venv returned non-zero; aborting venv setup",
            ));
        }
        let mut cmd = Command::new("uv");
        cmd.args(["pip", "install", "--python", &venv_python_path(venv_dir)]);
        cmd.args(packages);
        cmd.status()
    } else {
        let venv_status = Command::new("python3")
            .args(["-m", "venv", venv_dir])
            .status()
            .map_err(|e| SiaError::new(format!("venv creation failed: {e}")))?;
        if !venv_status.success() {
            return Err(SiaError::new(
                "python -m venv returned non-zero; aborting venv setup",
            ));
        }
        let mut cmd = Command::new(venv_pip_path(venv_dir));
        cmd.arg("install");
        cmd.args(packages);
        cmd.status()
    };
    status
        .map_err(|e| SiaError::new(format!("venv package install failed: {e}")))
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(SiaError::new("venv setup returned non-zero"))
            }
        })
}

/// Install a requirements.txt into an existing venv (augmenting the baseline packages).
pub fn install_requirements(venv_dir: &str, requirements_path: &str) -> SiaResult<()> {
    let status = if uv_available() {
        Command::new("uv")
            .args([
                "pip",
                "install",
                "--python",
                &venv_python_path(venv_dir),
                "-r",
                requirements_path,
            ])
            .status()
    } else {
        Command::new(venv_pip_path(venv_dir))
            .args(["install", "-r", requirements_path])
            .status()
    };
    status
        .map_err(|e| SiaError::new(format!("requirements install failed: {e}")))
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(SiaError::new("requirements install returned non-zero"))
            }
        })
}

/// Persist the resolved meta/target profiles as `profiles.json` in the run dir.
fn write_run_profiles(
    run_directory: &str,
    meta_profile: Option<&MetaAgentProfile>,
    target_profile: Option<&TargetAgentProfile>,
) {
    let mut profiles = serde_json::Map::new();
    if let Some(m) = meta_profile {
        profiles.insert(
            "meta".to_string(),
            serde_json::to_value(m).unwrap_or(json!(null)),
        );
    }
    if let Some(t) = target_profile {
        profiles.insert(
            "target".to_string(),
            serde_json::to_value(t).unwrap_or(json!(null)),
        );
    }
    if profiles.is_empty() {
        return;
    }
    let path = format!("{run_directory}/profiles.json");
    if let Ok(text) = serde_json::to_string_pretty(&serde_json::Value::Object(profiles)) {
        let _ = std::fs::write(path, text);
    }
}

/// Create run directories, venv, and context manager.
#[allow(clippy::too_many_arguments)]
pub fn setup_run_directory(
    run_id: i64,
    task_dir: &str,
    meta_model: &str,
    task_model: &str,
    agent_impl: &str,
    max_gen: i64,
    config: Option<Config>,
    meta_profile: Option<&MetaAgentProfile>,
    target_profile: Option<&TargetAgentProfile>,
) -> SiaResult<RunSetup> {
    let cfg = config.unwrap_or_default();
    let layout = RunLayout::for_run_id(run_id, crate::layout::names::RUNS_ROOT);
    let run_directory = layout.run_dir.clone();
    let meta_agent_working_directory = layout.gen_dir(1);

    if Path::new(&run_directory).exists() {
        return Err(SiaError::new(format!(
            "Run directory already exists: {run_directory}. Please use a different run_id or remove the existing directory"
        )));
    }

    std::fs::create_dir_all(&run_directory)
        .map_err(|e| SiaError::new(format!("Could not create run directory: {e}")))?;
    std::fs::create_dir_all(&meta_agent_working_directory)
        .map_err(|e| SiaError::new(format!("Could not create meta agent dir: {e}")))?;

    let venv_dir = layout.venv_dir();
    create_venv(&venv_dir, Config::VENV_PACKAGES)?;

    write_run_profiles(&run_directory, meta_profile, target_profile);

    let run_config = json!({
        "task_dir": task_dir,
        "meta_model": meta_model,
        "task_model": task_model,
        "agent_impl": agent_impl,
        "max_gen": max_gen,
    })
    .as_object()
    .unwrap()
    .clone();
    let context_mgr = ContextManager::new(&run_directory, run_config, Some(cfg));
    context_mgr.initialize();

    Ok(RunSetup {
        run_directory,
        meta_agent_working_directory,
        venv_dir,
        context_mgr,
    })
}
