//! Orchestrator core: execution-log loading, evaluation, target-agent execution
//! (plain + Docker sandbox), feedback-context building, and the per-generation
//! flow. Port of the testable core of `sia/orchestrator.py`.
//!
//! Process execution and the evaluate.py subprocess are behind injectable seams
//! (the `*_with` functions) so the branching logic can be unit-tested without a
//! real interpreter, mirroring the Python tests' `subprocess` mocks.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::agent_reference::{copy_reference_into, ResolvedAgentReference};
use crate::config::Config;
use crate::context_manager::GenData;
use crate::error::SiaResult;
use crate::io_utils::file_size_ok;
use crate::layout::{names, venv_python_path, RunLayout, TaskLayout};
use crate::profiles::MetaAgentProfile;
use crate::prompts::build_feedback_prompt;
use crate::providers::Provider;
use crate::run_setup::{install_requirements, RunSetup};
use crate::task_files::TaskFiles;

// --------------------------------------------------------------------------- //
// Execution-log loading
// --------------------------------------------------------------------------- //

/// Load execution logs with automatic format detection.
///
/// Returns `(execution_data, is_multi_trajectory)` mirroring the Python contract.
pub fn load_agent_execution(gen_directory: &str, config: &Config) -> (Value, bool) {
    let execution_folder = format!("{gen_directory}/{}", names::AGENT_EXECUTION_DIR);
    let execution_file = format!("{gen_directory}/{}", names::AGENT_EXECUTION_JSON);

    if Path::new(&execution_folder).is_dir() {
        let mut files: Vec<String> = match std::fs::read_dir(&execution_folder) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|n| {
                            let n = n.to_string_lossy();
                            n.starts_with(names::EXECUTION_GLOB_PREFIX) && n.ends_with(".json")
                        })
                        .unwrap_or(false)
                })
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort();

        if files.is_empty() {
            return (
                json!({"error": "Empty execution folder", "type": "multi-trajectory"}),
                true,
            );
        }

        let mut trajectories: Vec<Value> = Vec::new();
        for f in &files {
            let basename = Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match file_size_ok(f, config.max_execution_log_size) {
                Ok((within, size)) if !within => {
                    trajectories
                        .push(json!({"error": "File too large", "file": basename, "size": size}));
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    trajectories.push(json!({"error": e.to_string(), "file": basename}));
                    continue;
                }
            }
            match std::fs::read_to_string(f) {
                Ok(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(v) => trajectories.push(v),
                    Err(e) => trajectories.push(json!({"error": e.to_string(), "file": basename})),
                },
                Err(e) => trajectories.push(json!({"error": e.to_string(), "file": basename})),
            }
        }

        let count = trajectories.len();
        return (
            json!({"trajectories": trajectories, "count": count, "type": "multi-trajectory"}),
            true,
        );
    }

    if Path::new(&execution_file).exists() {
        match file_size_ok(&execution_file, config.max_execution_log_size) {
            Ok((within, size)) if !within => {
                return (json!({"error": "File too large", "size": size}), false);
            }
            _ => {}
        }
        match std::fs::read_to_string(&execution_file) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(v) => (v, false),
                Err(e) => {
                    let preview: String = text.chars().take(1000).collect();
                    (
                        json!({
                            "error": "Parse error",
                            "raw_preview": preview,
                            "parse_error": e.to_string(),
                            "file_size": text.chars().count(),
                        }),
                        false,
                    )
                }
            },
            Err(e) => (
                json!({"error": "Could not read file", "read_error": e.to_string()}),
                false,
            ),
        }
    } else {
        (json!({"error": "Execution log not found"}), false)
    }
}

// --------------------------------------------------------------------------- //
// Evaluation
// --------------------------------------------------------------------------- //

/// Outcome of running the evaluate.py subprocess (the injectable seam's return).
#[derive(Debug, Clone)]
pub enum EvalOutcome {
    Completed {
        returncode: i32,
        stdout: String,
        stderr: String,
    },
    TimedOut,
    SpawnError(String),
}

/// Run evaluate.py if present; returns a JSON status dict. Uses a real subprocess.
pub fn run_evaluation(
    gen_directory: &str,
    task_dir: &str,
    venv_dir: &str,
    config: &Config,
) -> Value {
    run_evaluation_with(&real_eval_runner, gen_directory, task_dir, venv_dir, config)
}

/// `run_evaluation` with an injectable command runner `(cmd, timeout) -> EvalOutcome`.
pub fn run_evaluation_with(
    runner: &dyn Fn(&[String], u64) -> EvalOutcome,
    gen_directory: &str,
    task_dir: &str,
    venv_dir: &str,
    config: &Config,
) -> Value {
    let evaluate_script = match TaskLayout::new(task_dir, "").evaluate_script() {
        Some(s) => s,
        None => return json!({"status": "skipped", "reason": "evaluate.py not found"}),
    };

    let eval_log_file = format!("{gen_directory}/{}", names::EVAL_LOG);
    let python_exec = venv_python_path(venv_dir);
    let cmd = vec![
        python_exec,
        evaluate_script,
        "--gen-dir".to_string(),
        gen_directory.to_string(),
    ];

    match runner(&cmd, config.eval_timeout) {
        EvalOutcome::TimedOut => {
            json!({"status": "error", "reason": format!("Evaluation timed out after {}s", config.eval_timeout)})
        }
        EvalOutcome::SpawnError(e) => json!({"status": "error", "reason": e}),
        EvalOutcome::Completed {
            returncode,
            stdout,
            stderr,
        } => {
            let eval_output = format!("{stdout}{stderr}");
            let _ = std::fs::write(&eval_log_file, &eval_output);

            if returncode != 0 {
                return json!({
                    "status": "error",
                    "reason": format!("evaluate.py exited with code {returncode}"),
                    "log_path": eval_log_file,
                    "output": eval_output,
                });
            }

            let results_json_path = format!("{gen_directory}/{}", names::RESULTS_JSON);
            if Path::new(&results_json_path).exists() {
                json!({
                    "status": "success",
                    "log_path": eval_log_file,
                    "results_path": results_json_path,
                    "output": eval_output,
                })
            } else {
                json!({
                    "status": "warning",
                    "reason": "results.json not created by evaluate.py",
                    "log_path": eval_log_file,
                    "output": eval_output,
                })
            }
        }
    }
}

fn real_eval_runner(cmd: &[String], timeout: u64) -> EvalOutcome {
    run_command_with_timeout(cmd, timeout)
}

fn run_command_with_timeout(cmd: &[String], timeout: u64) -> EvalOutcome {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let mut command = Command::new(&cmd[0]);
    command
        .args(&cmd[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return EvalOutcome::SpawnError(e.to_string()),
    };

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = child.wait_with_output();
        let _ = tx.send(out);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(timeout.max(1))) {
        Ok(Ok(output)) => EvalOutcome::Completed {
            returncode: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Ok(Err(e)) => EvalOutcome::SpawnError(e.to_string()),
        Err(_) => EvalOutcome::TimedOut,
    }
}

// --------------------------------------------------------------------------- //
// Target agent execution
// --------------------------------------------------------------------------- //

/// `f"{n}"` for an f64 matching Python's `str(float)` (integers keep a `.0`).
fn py_float(n: f64) -> String {
    let s = format!("{n}");
    if n.is_finite() && !s.contains('.') && !s.contains('e') && !s.contains('E') {
        format!("{s}.0")
    } else {
        s
    }
}

/// Build the Docker sandbox command (dataset read-only, working dir read-write, no network).
pub fn build_sandbox_cmd(dataset_dir: &str, working_dir: &str, config: &Config) -> Vec<String> {
    vec![
        "docker".into(),
        "run".into(),
        "--rm".into(),
        "--network".into(),
        "none".into(),
        "--memory".into(),
        config.docker_memory_limit.clone(),
        format!("--cpus={}", py_float(config.docker_cpu_limit)),
        "-v".into(),
        format!("{dataset_dir}:/data:ro"),
        "-v".into(),
        format!("{working_dir}:/work:rw"),
        config.docker_image.clone(),
        "python".into(),
        "-u".into(),
        "/work/target_agent.py".into(),
        "--dataset_dir".into(),
        "/data".into(),
        "--working_dir".into(),
        "/work".into(),
    ]
}

/// Build the plain (non-sandboxed) target-agent command.
pub fn build_target_cmd(
    python_exec: &str,
    target_agent_path: &str,
    abs_dataset_dir: &str,
    gen_dir: &str,
) -> Vec<String> {
    vec![
        python_exec.into(),
        "-u".into(),
        target_agent_path.into(),
        "--dataset_dir".into(),
        abs_dataset_dir.into(),
        "--working_dir".into(),
        gen_dir.into(),
    ]
}

/// Run `cmd`, streaming merged stdout/stderr to the console and a log file. Returns exit code.
///
/// Python's `_stream_to_log` runs the child with `stderr=STDOUT` (merged) and drains
/// the single pipe. We pipe stdout and stderr separately and drain **both
/// concurrently** on their own threads, writing both into the same log + console.
/// Draining concurrently is essential: a target agent that writes more than a pipe
/// buffer (~64 KiB) to stderr would otherwise block forever while we only read
/// stdout. Merging stderr into the log also preserves failure diagnostics (the
/// feedback prompt's "last 10 lines of output" come from this log).
pub fn stream_to_log(cmd: &[String], stdout_log_file: &str) -> std::io::Result<i32> {
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};

    let log_fh = std::fs::File::create(stdout_log_file)?;
    let mut child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let log = Arc::new(Mutex::new(log_fh));
    let h_out = pump_to_log(child.stdout.take(), Arc::clone(&log));
    let h_err = pump_to_log(child.stderr.take(), Arc::clone(&log));
    // Join the pumps before wait(): the pipes close on child exit, so the threads
    // finish, and only then do we reap the process.
    h_out.join().expect("stdout pump thread panicked")?;
    h_err.join().expect("stderr pump thread panicked")?;

    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}

/// Drain a child stream line-by-line to the console and the shared (merged) log.
fn pump_to_log<R: std::io::Read + Send + 'static>(
    stream: Option<R>,
    log: std::sync::Arc<std::sync::Mutex<std::fs::File>>,
) -> std::thread::JoinHandle<std::io::Result<()>> {
    use std::io::{BufRead, BufReader};
    std::thread::spawn(move || -> std::io::Result<()> {
        if let Some(s) = stream {
            for line in BufReader::new(s).lines() {
                let line = line?;
                println!("{line}");
                let mut fh = log.lock().unwrap();
                writeln!(fh, "{line}")?;
            }
        }
        Ok(())
    })
}

/// Process runner seam: `(cmd, stdout_log_file) -> exit code`.
pub type ProcRunner<'a> = dyn Fn(&[String], &str) -> std::io::Result<i32> + 'a;

/// Run the target agent (sandbox or plain) via an injectable process runner.
#[allow(clippy::too_many_arguments)]
pub fn run_target_agent_with(
    runner: &ProcRunner,
    venv_dir: &str,
    target_agent_path: &str,
    abs_dataset_dir: &str,
    gen_dir: &str,
    stdout_log_file: &str,
    sandbox: &str,
    env_config: &Config,
) -> (bool, String, String, String) {
    let python_exec = venv_python_path(venv_dir);

    let cmd = if sandbox == "docker" {
        build_sandbox_cmd(abs_dataset_dir, gen_dir, env_config)
    } else {
        build_target_cmd(&python_exec, target_agent_path, abs_dataset_dir, gen_dir)
    };

    match runner(&cmd, stdout_log_file) {
        Ok(return_code) => {
            let stdout = std::fs::read_to_string(stdout_log_file).unwrap_or_default();
            if return_code != 0 {
                let error_msg = format!("Target agent failed with exit code {return_code}");
                (false, stdout, String::new(), error_msg)
            } else {
                (true, stdout, String::new(), String::new())
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            false,
            String::new(),
            String::new(),
            format!("Target agent file not found: {target_agent_path}"),
        ),
        Err(e) => {
            let stdout = std::fs::read_to_string(stdout_log_file).unwrap_or_default();
            (
                false,
                stdout,
                String::new(),
                format!("Unexpected error during target agent execution: {e}"),
            )
        }
    }
}

/// Run the target agent using the real process runner.
#[allow(clippy::too_many_arguments)]
pub fn run_target_agent(
    venv_dir: &str,
    target_agent_path: &str,
    abs_dataset_dir: &str,
    gen_dir: &str,
    stdout_log_file: &str,
    sandbox: &str,
    env_config: &Config,
) -> (bool, String, String, String) {
    run_target_agent_with(
        &stream_to_log,
        venv_dir,
        target_agent_path,
        abs_dataset_dir,
        gen_dir,
        stdout_log_file,
        sandbox,
        env_config,
    )
}

// --------------------------------------------------------------------------- //
// Feedback context
// --------------------------------------------------------------------------- //

fn json_pretty(value: &Value) -> String {
    // ensure_ascii=True, matching Python json.dumps for non-ASCII (e.g. LawBench).
    crate::pyjson::dumps_indent2(value)
}

fn truncate_chars(s: &str, limit: usize, suffix: &str) -> String {
    if s.chars().count() > limit {
        let t: String = s.chars().take(limit).collect();
        format!("{t}{suffix}")
    } else {
        s.to_string()
    }
}

/// Build execution status and section text for the feedback prompt.
#[allow(clippy::too_many_arguments)]
pub fn build_feedback_context(
    _current_gen: i64,
    gen_dir: &str,
    _dataset_dir: &str,
    target_agent_success: bool,
    target_agent_error_msg: &str,
    target_agent_stdout: &str,
    target_agent_stderr: &str,
    stdout_log_file: &str,
    _task_files: &TaskFiles,
    config: &Config,
) -> (String, String) {
    let (agent_execution, is_multi) = load_agent_execution(gen_dir, config);

    let execution_section = if is_multi {
        let trajectory_count = agent_execution
            .get("count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let empty = Vec::new();
        let trajectories = agent_execution
            .get("trajectories")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);

        let successful = trajectories.iter().filter(|t| t.is_array()).count();
        let failed = trajectories
            .iter()
            .filter(|t| t.is_object() && t.get("error").map(is_truthy).unwrap_or(false))
            .count();

        let mut sample_trajectories_text = String::new();
        for (idx, traj) in trajectories.iter().take(3).enumerate() {
            let traj_json = truncate_chars(
                &json_pretty(traj),
                config.trajectory_preview_limit,
                "\n  ... (truncated)",
            );
            sample_trajectories_text.push_str(&format!(
                "\n### Trajectory {idx}\n```json\n{traj_json}\n```\n"
            ));
        }

        let exec_dir = format!("{gen_dir}/{}", names::AGENT_EXECUTION_DIR);
        format!(
            r#"
**MULTI-TRAJECTORY EXECUTION**:

The agent executed {trajectory_count} separate trajectories (e.g., different questions/samples).

**Summary**:
- Total trajectories: {trajectory_count}
- Successful: {successful}
- Failed: {failed}
- Execution folder: {exec_dir}

**Sample Trajectories** (first 3 shown, you can read others from the folder):
{sample_trajectories_text}

**To analyze all trajectories**:
- Read files from: {exec_dir}
- Files named: execution_q0.json, execution_q1.json, ..., execution_q{last}.json

**Analysis guidance**:
- Look for common failure patterns across trajectories
- Check if trajectories are properly isolated
- Ensure consistent behavior across all samples
"#,
            trajectory_count = trajectory_count,
            successful = successful,
            failed = failed,
            exec_dir = exec_dir,
            sample_trajectories_text = sample_trajectories_text,
            last = trajectory_count - 1,
        )
    } else {
        let traj_json = truncate_chars(
            &json_pretty(&agent_execution),
            config.trajectory_preview_limit,
            "\n  ... (truncated)",
        );
        format!(
            r#"
Here is the target agent execution trajectory:
```json
{traj_json}
```

NOTE: If you see an "error" field in the above JSON, it means the execution log was malformed or missing. Focus on making the agent more robust.
"#
        )
    };

    // Evaluation results section.
    let results_json_path = format!("{gen_dir}/{}", names::RESULTS_JSON);
    let eval_results_section = if Path::new(&results_json_path).exists() {
        match file_size_ok(&results_json_path, config.max_execution_log_size) {
            Ok((within, size)) if !within => {
                format!(
                    "\n**EVALUATION RESULTS**: results.json too large ({} bytes)\n",
                    crate::pyfmt::commas_u64(size)
                )
            }
            _ => match std::fs::read_to_string(&results_json_path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            {
                Some(eval_data) => format!(
                    "\n\n**EVALUATION RESULTS**:\n```json\n{}\n```\n",
                    json_pretty(&eval_data)
                ),
                None => "\n**EVALUATION RESULTS**: Error loading results.json\n".to_string(),
            },
        }
    } else {
        "\n**EVALUATION RESULTS**: No results.json found (evaluation may not have run or may have failed)\n".to_string()
    };

    // Last 10 lines of output.
    let stdout_lines: Vec<&str> = target_agent_stdout.split('\n').collect();
    let last_10_lines = if stdout_lines.len() > 10 {
        stdout_lines[stdout_lines.len() - 10..].join("\n")
    } else {
        target_agent_stdout.to_string()
    };

    let execution_status = if target_agent_success {
        format!(
            "SUCCESS: Target agent completed execution successfully.\n\
{eval_results_section}\n\n\
**Last 10 lines of output**:\n```\n{last_10_lines}\n```\n\n\
Full logs available at: {stdout_log_file}\n",
            eval_results_section = eval_results_section,
            last_10_lines = last_10_lines,
            stdout_log_file = stdout_log_file,
        )
    } else {
        format!(
            "FAILED: {error_msg}\n\
{eval_results_section}\n\n\
**Last 10 lines of output**:\n```\n{last_10_lines}\n```\n\n\
Full logs available at: {stdout_log_file}\n\n\
STDERR:\n{stderr}\n",
            error_msg = target_agent_error_msg,
            eval_results_section = eval_results_section,
            last_10_lines = last_10_lines,
            stdout_log_file = stdout_log_file,
            stderr = target_agent_stderr,
        )
    };

    (execution_status, execution_section)
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// --------------------------------------------------------------------------- //
// Generation flow
// --------------------------------------------------------------------------- //

/// Dynamic per-generation data passed to the feedback seam.
pub struct FeedbackArgs<'a> {
    pub current_gen: i64,
    pub max_gen: i64,
    pub run_dir: &'a str,
    pub next_gen_dir: &'a str,
    pub execution_status: &'a str,
    pub execution_section: &'a str,
}

/// Signature of the target-agent execution seam.
pub type TargetAgentFn<'a> =
    dyn Fn(&str, &str, &str, &str, &str, &str, &Config) -> (bool, String, String, String) + 'a;

/// Execute one generation with injectable target-agent and feedback seams.
#[allow(clippy::too_many_arguments)]
pub fn run_generation_with(
    target_fn: &TargetAgentFn,
    feedback_fn: &mut dyn FnMut(&FeedbackArgs) -> SiaResult<()>,
    current_gen: i64,
    max_gen: i64,
    run_setup: &mut RunSetup,
    task_files: &TaskFiles,
    abs_dataset_dir: &str,
    dataset_dir: &str,
    sandbox: &str,
    env_config: &Config,
) -> SiaResult<()> {
    let run_dir = run_setup.run_directory.clone();
    let layout = RunLayout::new(run_dir.clone());
    let gen_dir = layout.gen_dir(current_gen);
    let target_agent_path = layout.target_agent(current_gen);
    let stdout_log_file = layout.stdout_log(current_gen);

    // Install this generation's declared dependencies (if any) before running.
    let gen_requirements = format!("{gen_dir}/{}", names::REQUIREMENTS_TXT);
    if Path::new(&gen_requirements).is_file() {
        let _ = install_requirements(&run_setup.venv_dir, &gen_requirements);
    }

    let start = Instant::now();
    let (success, stdout, stderr, error_msg) = target_fn(
        &run_setup.venv_dir,
        &target_agent_path,
        abs_dataset_dir,
        &gen_dir,
        &stdout_log_file,
        sandbox,
        env_config,
    );
    let duration = start.elapsed().as_secs_f64();

    // Run evaluation (if evaluate.py exists).
    let _ = run_evaluation(&gen_dir, dataset_dir, &run_setup.venv_dir, env_config);

    // Closed-loop step (#84): record the adaptive scheduler's harness-vs-weight
    // decision for this generation and, when it recommends weights, run an
    // observable CPU-reference weight update. Purely additive and best-effort —
    // it only writes NEW artifacts + a log line, guards every failure, and never
    // affects this function's control flow or return value.
    {
        let decision = crate::closed_loop::record_scheduler_decision(
            &layout,
            current_gen,
            &crate::scheduler::SchedulerConfig::default(),
        );
        if let Some(d) = &decision {
            if let Some(kind) = d.get("decision").and_then(|v| v.as_str()) {
                let _ = crate::closed_loop::maybe_run_weight_update(
                    &layout,
                    current_gen,
                    kind,
                    &crate::weights::WeightUpdateConfig::default(),
                );
                let rationale = d.get("rationale").and_then(|v| v.as_str()).unwrap_or("");
                println!("[scheduler] gen {current_gen}: decision={kind} — {rationale}");
            }
        }
    }

    // Add generation to context.
    let improvement_md_path = layout.improvement_md(current_gen);
    let execution_type = if Path::new(&layout.agent_execution_dir(current_gen)).is_dir() {
        "Multi-trajectory"
    } else {
        "Single"
    };
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    run_setup.context_mgr.add_generation(
        current_gen,
        &GenData {
            success,
            timestamp,
            duration,
            agent_path: target_agent_path.clone(),
            gen_dir: gen_dir.clone(),
            improvement_path: if Path::new(&improvement_md_path).exists() {
                Some(improvement_md_path.clone())
            } else {
                None
            },
            execution_type: execution_type.to_string(),
        },
    );

    if current_gen < max_gen {
        let (execution_status, execution_section) = build_feedback_context(
            current_gen,
            &gen_dir,
            dataset_dir,
            success,
            &error_msg,
            &stdout,
            &stderr,
            &stdout_log_file,
            task_files,
            env_config,
        );
        let next_gen = current_gen + 1;
        let next_gen_directory = layout.gen_dir(next_gen);
        feedback_fn(&FeedbackArgs {
            current_gen,
            max_gen,
            run_dir: &run_dir,
            next_gen_dir: &next_gen_directory,
            execution_status: &execution_status,
            execution_section: &execution_section,
        })?;
    }

    Ok(())
}

/// Run the feedback agent to create an improved target agent (real wiring).
#[allow(clippy::too_many_arguments)]
pub fn run_feedback_agent(
    args: &FeedbackArgs,
    task_files: &TaskFiles,
    meta_profile: &MetaAgentProfile,
    env_config: &Config,
    dataset_dir: &str,
    task_model: &str,
    target_provider: &Provider,
    resolved_ref: Option<&ResolvedAgentReference>,
) -> SiaResult<()> {
    let layout = RunLayout::new(args.run_dir.to_string());
    let agent_py =
        std::fs::read_to_string(layout.target_agent(args.current_gen)).unwrap_or_default();
    let task = std::fs::read_to_string(format!("{dataset_dir}/task.md")).unwrap_or_default();

    let previous_gens_text = if args.current_gen > 1 {
        (1..args.current_gen)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        "None".to_string()
    };
    let previous_gens_text = if previous_gens_text.is_empty() {
        "None".to_string()
    } else {
        previous_gens_text
    };

    let requirements_dir = match resolved_ref {
        Some(r) if r.requirements.is_some() => Some(args.next_gen_dir),
        _ => None,
    };

    let feedback_prompt = build_feedback_prompt(
        args.current_gen,
        args.max_gen,
        task_files,
        &agent_py,
        &task,
        args.execution_status,
        args.execution_section,
        args.run_dir,
        args.next_gen_dir,
        &previous_gens_text,
        task_model,
        Some(target_provider),
        requirements_dir,
    );

    std::fs::create_dir_all(args.next_gen_dir).ok();
    if let Some(r) = resolved_ref {
        let _ = copy_reference_into(r, Path::new(args.next_gen_dir));
    }

    let feedback_prompt_path = format!("{}/{}", args.next_gen_dir, names::FEEDBACK_PROMPT);
    let _ = std::fs::write(&feedback_prompt_path, &feedback_prompt);

    crate::agent_impls::run_agent(
        &meta_profile.model,
        &env_config.default_max_turns.to_string(),
        &feedback_prompt,
        args.next_gen_dir,
        &meta_profile.agent_impl,
        Some(meta_profile.provider.clone()),
    )
}

// Re-export TaskFiles to match the Python `from sia.orchestrator import TaskFiles`.
pub use crate::task_files::TaskFiles as OrchestratorTaskFiles;

/// Helper used by tests to count keys when needed.
#[doc(hidden)]
pub fn _map_len(m: &Map<String, Value>) -> usize {
    m.len()
}
