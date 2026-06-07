//! Target-agent execution seam (issue #138 / ADR-0001).
//!
//! Target agents are LLM-authored **Python** ML programs and are *not* rewritten
//! in Rust. What ADR-0001 calls "native target-agent execution" is a Rust-native
//! **execution abstraction** around them: a [`TargetExecutor`] trait that owns the
//! single responsibility of running one target-agent generation and returning the
//! existing `(success, stdout, stderr, error_msg)` tuple.
//!
//! This seam lets the per-generation flow ([`crate::orchestrator::run_generation_with`])
//! depend on a behavior contract rather than on the concrete Python-venv bridge, so
//! the bridge can be hardened, sandboxed differently, or replaced **incrementally**
//! without touching the orchestration loop or its tests.
//!
//! ## Strategies
//!
//! * [`PythonVenvExecutor`] — the **default**, today's behavior. Delegates verbatim
//!   to [`crate::orchestrator::run_target_agent`] (plain subprocess + Docker sandbox
//!   paths). Byte-for-byte identical to the pre-#138 code path.
//! * [`NativeExecutor`] — a documented scaffold for the Python-bridge-free path
//!   (see [`NativeExecutor`] docs). **Not wired as default**; returns a clear
//!   "not yet implemented" error if invoked. Tracked by `TODO(#138)`.
//!
//! The orchestrator keeps its existing injectable `target_fn` closure seam (tests
//! rely on it); [`target_fn_for`] adapts any [`TargetExecutor`] into that closure so
//! both seams coexist.

use crate::config::Config;
use crate::error::{SiaError, SiaResult};

/// Inputs needed to run one target-agent generation.
///
/// Mirrors the positional arguments of [`crate::orchestrator::run_target_agent`],
/// grouped into a struct so additional strategies can read exactly what they need
/// without growing a wide positional signature.
#[derive(Debug, Clone, Copy)]
pub struct TargetRunRequest<'a> {
    /// Per-generation virtual-environment directory (provides the Python interpreter
    /// for the plain path).
    pub venv_dir: &'a str,
    /// Absolute path to the generation's `target_agent.py`.
    pub target_agent_path: &'a str,
    /// Absolute path to the (read-only) dataset directory.
    pub abs_dataset_dir: &'a str,
    /// Generation working directory (read-write scratch / output).
    pub gen_dir: &'a str,
    /// File the merged stdout/stderr stream is written to.
    pub stdout_log_file: &'a str,
    /// Sandbox mode (`"docker"` selects the Docker sandbox path; anything else is plain).
    pub sandbox: &'a str,
}

/// Outcome of one target-agent generation: `(success, stdout, stderr, error_msg)`.
///
/// This is the exact tuple the orchestrator already consumes, kept as a type alias
/// so the trait return type and the legacy closure seam stay in lock-step.
pub type TargetRunResult = (bool, String, String, String);

/// A strategy for executing one target-agent generation.
///
/// Implementors own *how* the LLM-authored Python program is run (venv subprocess,
/// Docker sandbox, future capability-confined runner, ...). The orchestration loop
/// depends only on this contract.
pub trait TargetExecutor {
    /// Run one target-agent generation and return `(success, stdout, stderr, error_msg)`.
    fn run(&self, req: &TargetRunRequest, config: &Config) -> TargetRunResult;
}

/// Default executor: the existing Python-venv bridge.
///
/// Delegates to [`crate::orchestrator::run_target_agent`], preserving both the plain
/// subprocess and the Docker-sandbox paths exactly as before #138. This is the only
/// executor wired as default, so behavior is byte-for-byte unchanged.
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonVenvExecutor;

impl TargetExecutor for PythonVenvExecutor {
    fn run(&self, req: &TargetRunRequest, config: &Config) -> TargetRunResult {
        crate::orchestrator::run_target_agent(
            req.venv_dir,
            req.target_agent_path,
            req.abs_dataset_dir,
            req.gen_dir,
            req.stdout_log_file,
            req.sandbox,
            config,
        )
    }
}

impl PythonVenvExecutor {
    /// Variant that runs through an injectable process runner, mirroring
    /// [`crate::orchestrator::run_target_agent_with`]. Used by offline unit tests so
    /// the dispatch logic is exercised without spawning a real interpreter.
    pub fn run_with(
        &self,
        runner: &crate::orchestrator::ProcRunner,
        req: &TargetRunRequest,
        config: &Config,
    ) -> TargetRunResult {
        crate::orchestrator::run_target_agent_with(
            runner,
            req.venv_dir,
            req.target_agent_path,
            req.abs_dataset_dir,
            req.gen_dir,
            req.stdout_log_file,
            req.sandbox,
            config,
        )
    }
}

/// Scaffold for the Python-bridge-free execution path (`TODO(#138)`).
///
/// The intended design — *not yet implemented* — is to run the LLM-authored Python
/// program **without** the per-generation uv/pip venv bridge. The leading candidate
/// is a capability-confined direct subprocess that:
///
/// * resolves a single, pre-provisioned interpreter (or a content-addressed
///   environment) instead of building a venv per generation,
/// * confines filesystem and process access through the existing
///   [`crate::sandbox`] allow-list (`check_read`/`check_write`/`check_bash`) rather
///   than relying solely on Docker, so the default build gains a deny-by-default
///   boundary, and
/// * streams output through the same [`crate::orchestrator::stream_to_log`]
///   contract, returning the identical `(success, stdout, stderr, error_msg)` tuple.
///
/// Until that lands this executor is inert: it is never selected as the default and
/// [`TargetExecutor::run`] returns a failure tuple whose `error_msg` explains the
/// status (so an accidental wiring fails loudly rather than silently degrading).
/// [`NativeExecutor::try_run`] returns the same status as a typed [`SiaResult`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeExecutor;

impl NativeExecutor {
    /// Message returned everywhere this scaffold is invoked.
    pub const NOT_IMPLEMENTED: &'static str =
        "NativeExecutor (Python-bridge-free target execution) is not yet implemented (TODO(#138)); \
         use PythonVenvExecutor";

    /// Typed form of the not-yet-implemented status.
    ///
    /// `TODO(#138)`: replace with the capability-confined runner described in the
    /// type-level docs.
    pub fn try_run(&self, _req: &TargetRunRequest, _config: &Config) -> SiaResult<TargetRunResult> {
        Err(SiaError::new(Self::NOT_IMPLEMENTED))
    }
}

impl TargetExecutor for NativeExecutor {
    fn run(&self, req: &TargetRunRequest, config: &Config) -> TargetRunResult {
        match self.try_run(req, config) {
            Ok(result) => result,
            Err(e) => (false, String::new(), String::new(), e.to_string()),
        }
    }
}

/// Adapt a [`TargetExecutor`] into the orchestrator's legacy `target_fn` closure.
///
/// The orchestration loop and its tests pass a `Fn(&str, &str, &str, &str, &str,
/// &str, &Config) -> (bool, String, String, String)` closure; this bridges any
/// executor into that shape so the trait and the existing seam coexist without
/// changing [`crate::orchestrator::run_generation_with`]'s signature.
pub fn target_fn_for<E: TargetExecutor>(
    executor: &E,
) -> impl Fn(&str, &str, &str, &str, &str, &str, &Config) -> TargetRunResult + '_ {
    move |venv_dir,
          target_agent_path,
          abs_dataset_dir,
          gen_dir,
          stdout_log_file,
          sandbox,
          config| {
        let req = TargetRunRequest {
            venv_dir,
            target_agent_path,
            abs_dataset_dir,
            gen_dir,
            stdout_log_file,
            sandbox,
        };
        executor.run(&req, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req<'a>(sandbox: &'a str, log: &'a str) -> TargetRunRequest<'a> {
        TargetRunRequest {
            venv_dir: "/tmp/venv",
            target_agent_path: "/tmp/gen_1/target_agent.py",
            abs_dataset_dir: "/tmp/data",
            gen_dir: "/tmp/gen_1",
            stdout_log_file: log,
            sandbox,
        }
    }

    /// `PythonVenvExecutor` dispatches to the plain (non-docker) command and reports
    /// success on exit code 0 — exercised through the injectable process runner so
    /// no real interpreter is spawned.
    #[test]
    fn python_venv_executor_plain_success() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("stdout.log");
        std::fs::write(&log, "hello from agent\n").unwrap();
        let log = log.to_string_lossy().into_owned();

        let seen = AtomicUsize::new(0);
        let runner = |cmd: &[String], log_file: &str, _timeout: u64| {
            seen.fetch_add(1, Ordering::SeqCst);
            // Plain path: interpreter + -u + target_agent.py + dataset/working flags.
            assert!(cmd[0].contains("python") || cmd[0].ends_with("python3"));
            assert_eq!(cmd[1], "-u");
            assert!(cmd.contains(&"--dataset_dir".to_string()));
            assert!(!cmd.contains(&"docker".to_string()));
            assert_eq!(log_file, log);
            Ok::<i32, std::io::Error>(0)
        };

        let exec = PythonVenvExecutor;
        let (success, stdout, stderr, error_msg) =
            exec.run_with(&runner, &req("none", &log), &Config::default());

        assert_eq!(seen.load(Ordering::SeqCst), 1);
        assert!(success);
        assert_eq!(stdout, "hello from agent\n");
        assert!(stderr.is_empty());
        assert!(error_msg.is_empty());
    }

    /// Non-zero exit -> failure tuple with the canonical error message, while still
    /// surfacing captured stdout (parity with `run_target_agent_with`).
    #[test]
    fn python_venv_executor_reports_failure() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("stdout.log");
        std::fs::write(&log, "partial output\n").unwrap();
        let log = log.to_string_lossy().into_owned();

        let runner = |_cmd: &[String], _log_file: &str, _timeout: u64| Ok::<i32, std::io::Error>(3);
        let exec = PythonVenvExecutor;
        let (success, stdout, _stderr, error_msg) =
            exec.run_with(&runner, &req("none", &log), &Config::default());

        assert!(!success);
        assert_eq!(stdout, "partial output\n");
        assert_eq!(error_msg, "Target agent failed with exit code 3");
    }

    /// `sandbox == "docker"` selects the Docker sandbox command.
    #[test]
    fn python_venv_executor_selects_docker_path() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("stdout.log");
        std::fs::write(&log, "").unwrap();
        let log = log.to_string_lossy().into_owned();

        let runner = |cmd: &[String], _log_file: &str, _timeout: u64| {
            assert_eq!(cmd[0], "docker");
            assert!(cmd.contains(&"--network".to_string()));
            Ok::<i32, std::io::Error>(0)
        };
        let exec = PythonVenvExecutor;
        let (success, _stdout, _stderr, _error_msg) =
            exec.run_with(&runner, &req("docker", &log), &Config::default());
        assert!(success);
    }

    /// `target_fn_for` produces a closure with the orchestrator's seam signature and
    /// forwards through to the executor.
    #[test]
    fn target_fn_for_bridges_executor() {
        struct Stub;
        impl TargetExecutor for Stub {
            fn run(&self, req: &TargetRunRequest, _config: &Config) -> TargetRunResult {
                (true, req.sandbox.to_string(), String::new(), String::new())
            }
        }
        let f = target_fn_for(&Stub);
        let (ok, stdout, _e, _m) = f(
            "v",
            "p",
            "ds",
            "gen",
            "log",
            "weird-sandbox",
            &Config::default(),
        );
        assert!(ok);
        assert_eq!(stdout, "weird-sandbox");
    }

    /// The `NativeExecutor` scaffold is inert: it never succeeds and explains itself.
    #[test]
    fn native_executor_is_not_implemented() {
        let exec = NativeExecutor;
        let cfg = Config::default();
        assert!(exec.try_run(&req("none", "/tmp/x.log"), &cfg).is_err());

        let (success, _stdout, _stderr, error_msg) = exec.run(&req("none", "/tmp/x.log"), &cfg);
        assert!(!success);
        assert!(error_msg.contains("not yet implemented"));
        assert!(error_msg.contains("TODO(#138)"));
    }
}
