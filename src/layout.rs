//! Filesystem layout: path/filename constants and run/task path builders.
//!
//! Port of `sia/layout.py`. Path-building methods return `String` (not `PathBuf`)
//! to match the original `os.path`-based call sites, keeping behavior identical.

use std::path::{Component, Path, PathBuf};

use crate::error::{SiaError, SiaResult};

/// Tasks that ship with the framework under `sia/tasks/<name>/`.
pub const BUNDLED_TASKS: &[&str] = &["gpqa", "lawbench", "longcot-chess", "spaceship-titanic"];

/// Every filename / relative-path literal used by a run or a task.
pub mod names {
    // Run / generation artifacts
    pub const TARGET_AGENT: &str = "target_agent.py";
    pub const AGENT_EXECUTION_JSON: &str = "agent_execution.json";
    pub const AGENT_EXECUTION_DIR: &str = "agent_execution";
    pub const EXECUTION_GLOB_PREFIX: &str = "execution_q";
    pub const STDOUT_LOG: &str = "target_agent_stdout.log";
    pub const EVAL_LOG: &str = "evaluation.log";
    pub const RESULTS_JSON: &str = "results.json";
    pub const CONTEXT_MD: &str = "context.md";
    pub const IMPROVEMENT_MD: &str = "improvement.md";
    pub const META_PROMPT: &str = "meta_agent_prompt.txt";
    pub const FEEDBACK_PROMPT: &str = "feedback_agent_prompt.txt";
    pub const REQUIREMENTS_TXT: &str = "requirements.txt";
    pub const VENV_DIR: &str = "venv";
    pub const RUNS_ROOT: &str = "./runs";

    // Task inputs
    pub const DATA_PUBLIC: &str = "data/public";
    pub const TASK_MD: &str = "data/public/task.md";
    pub const EVALUATE_PY: &str = "evaluate.py";
    pub const SHARED_SAMPLE_EXECUTION: &str = "sample_agent_execution.json";
    pub const REFERENCE_DIR: &str = "reference";
    pub const REFERENCE_AGENT_FILE: &str = "reference_target_agent.py";
    pub const SAMPLE_TASK_DESCRIPTIONS: &str = "reference/SAMPLE_TASK_DESCRIPTIONS.md";
    pub const REFERENCE_AGENT: &str = "reference/reference_target_agent.py";
    pub const SHARED_DIR: &str = "_shared";
}

/// Join path components with `/`, matching `os.path.join` for our (always-relative
/// trailing) usage.
fn join(parts: &[&str]) -> String {
    parts.join("/")
}

/// Lexical absolute path, matching `os.path.abspath`: makes the path absolute
/// against the current working directory and normalizes `.`/`..` without touching
/// the filesystem or resolving symlinks.
pub fn abspath(path: &str) -> String {
    let p = Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    normpath(&abs)
}

fn normpath(p: &Path) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut root = String::new();
    for comp in p.components() {
        match comp {
            Component::RootDir => root = "/".to_string(),
            Component::Prefix(pre) => root = pre.as_os_str().to_string_lossy().into_owned(),
            Component::CurDir => {}
            Component::ParentDir => {
                if out.last().map(|s| s != "..").unwrap_or(false) {
                    out.pop();
                } else if root.is_empty() {
                    out.push("..".to_string());
                }
            }
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
        }
    }
    let joined = out.join("/");
    if root == "/" {
        format!("/{joined}")
    } else if !root.is_empty() {
        format!("{root}/{joined}")
    } else {
        joined
    }
}

/// Path to the python executable inside a venv.
pub fn venv_python_path(venv_dir: &str) -> String {
    join(&[venv_dir, "bin", "python"])
}

/// Path to the pip executable inside a venv.
pub fn venv_pip_path(venv_dir: &str) -> String {
    join(&[venv_dir, "bin", "pip"])
}

/// Locate evaluate.py: prefer data/public/evaluate.py, then task_dir/evaluate.py, else None.
pub fn find_evaluate_script(task_dir: &str) -> Option<String> {
    let candidate = join(&[task_dir, names::DATA_PUBLIC, names::EVALUATE_PY]);
    if Path::new(&candidate).exists() {
        return Some(candidate);
    }
    let candidate = join(&[task_dir, names::EVALUATE_PY]);
    if Path::new(&candidate).exists() {
        return Some(candidate);
    }
    None
}

/// Root directory holding bundled tasks: `$SIA_TASKS_DIR` else `./sia/tasks`.
fn bundled_tasks_root() -> PathBuf {
    match std::env::var("SIA_TASKS_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from("sia/tasks"),
    }
}

/// Resolve `--task` / `--task_dir` to a `(task_dir, shared_dir)` pair of real paths.
pub fn resolve_task_dir(task: Option<&str>, task_dir: Option<&str>) -> SiaResult<(String, String)> {
    let bundled_root = bundled_tasks_root();
    let bundled_shared = bundled_root.join(names::SHARED_DIR);

    if let Some(task) = task {
        let resolved = bundled_root.join(task);
        if !resolved.is_dir() {
            let available = BUNDLED_TASKS.join(", ");
            return Err(SiaError::new(format!(
                "Bundled task '{task}' not found. Available: {available}"
            )));
        }
        return Ok((
            resolved.display().to_string(),
            bundled_shared.display().to_string(),
        ));
    }

    if let Some(task_dir) = task_dir {
        let resolved =
            std::fs::canonicalize(task_dir).unwrap_or_else(|_| PathBuf::from(abspath(task_dir)));
        if !resolved.is_dir() {
            return Err(SiaError::new(format!(
                "Task directory does not exist: {task_dir}"
            )));
        }
        let external_shared = resolved
            .parent()
            .map(|p| p.join(names::SHARED_DIR))
            .filter(|p| p.is_dir());
        let shared = external_shared.unwrap_or(bundled_shared);
        return Ok((resolved.display().to_string(), shared.display().to_string()));
    }

    Err(SiaError::new(
        "Either --task or --task_dir must be provided",
    ))
}

/// Paths under a run directory (e.g. `./runs/run_1`).
#[derive(Debug, Clone, PartialEq)]
pub struct RunLayout {
    pub run_dir: String,
}

impl RunLayout {
    pub fn new(run_dir: impl Into<String>) -> Self {
        RunLayout {
            run_dir: run_dir.into(),
        }
    }

    pub fn for_run_id(run_id: i64, runs_root: &str) -> Self {
        RunLayout {
            run_dir: format!("{runs_root}/run_{run_id}"),
        }
    }

    /// Absolute path to a generation directory.
    pub fn gen_dir(&self, n: i64) -> String {
        abspath(&format!("{}/gen_{}", self.run_dir, n))
    }

    /// Relative path to a generation directory.
    pub fn gen_dir_rel(&self, n: i64) -> String {
        join(&[&self.run_dir, &format!("gen_{n}")])
    }

    pub fn venv_dir(&self) -> String {
        join(&[&self.run_dir, names::VENV_DIR])
    }

    pub fn venv_python(&self) -> String {
        venv_python_path(&self.venv_dir())
    }

    pub fn context_md(&self) -> String {
        join(&[&self.run_dir, names::CONTEXT_MD])
    }

    pub fn target_agent(&self, n: i64) -> String {
        join(&[&self.gen_dir(n), names::TARGET_AGENT])
    }

    pub fn stdout_log(&self, n: i64) -> String {
        join(&[&self.gen_dir(n), names::STDOUT_LOG])
    }

    pub fn improvement_md(&self, n: i64) -> String {
        join(&[&self.gen_dir(n), names::IMPROVEMENT_MD])
    }

    pub fn agent_execution_dir(&self, n: i64) -> String {
        join(&[&self.gen_dir(n), names::AGENT_EXECUTION_DIR])
    }

    pub fn meta_prompt(&self, n: i64) -> String {
        join(&[&self.gen_dir(n), names::META_PROMPT])
    }
}

/// Paths for a resolved task directory and its shared directory.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskLayout {
    pub task_dir: String,
    pub shared_dir: String,
}

impl TaskLayout {
    pub fn new(task_dir: impl Into<String>, shared_dir: impl Into<String>) -> Self {
        TaskLayout {
            task_dir: task_dir.into(),
            shared_dir: shared_dir.into(),
        }
    }

    pub fn dataset_dir(&self) -> String {
        join(&[&self.task_dir, names::DATA_PUBLIC])
    }

    pub fn abs_dataset_dir(&self) -> String {
        abspath(&self.dataset_dir())
    }

    pub fn task_md(&self) -> String {
        join(&[&self.task_dir, names::TASK_MD])
    }

    pub fn sample_descriptions(&self) -> String {
        join(&[&self.task_dir, names::SAMPLE_TASK_DESCRIPTIONS])
    }

    pub fn reference_dir(&self) -> String {
        join(&[&self.task_dir, names::REFERENCE_DIR])
    }

    pub fn reference_agent(&self) -> String {
        join(&[&self.task_dir, names::REFERENCE_AGENT])
    }

    pub fn sample_execution(&self) -> String {
        join(&[&self.shared_dir, names::SHARED_SAMPLE_EXECUTION])
    }

    pub fn evaluate_script(&self) -> Option<String> {
        find_evaluate_script(&self.task_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_venv_paths() {
        assert_eq!(
            venv_python_path("runs/run_1/venv"),
            "runs/run_1/venv/bin/python"
        );
        assert_eq!(venv_pip_path("v"), "v/bin/pip");
    }

    #[test]
    fn test_run_layout_rel() {
        let l = RunLayout::for_run_id(1, names::RUNS_ROOT);
        assert_eq!(l.run_dir, "./runs/run_1");
        assert_eq!(l.gen_dir_rel(2), "./runs/run_1/gen_2");
        assert_eq!(l.venv_dir(), "./runs/run_1/venv");
        assert_eq!(l.context_md(), "./runs/run_1/context.md");
    }

    #[test]
    fn test_abspath_normalizes() {
        assert_eq!(abspath("/a/b/../c"), "/a/c");
        assert_eq!(abspath("/a/./b"), "/a/b");
    }

    #[test]
    fn test_task_layout() {
        let t = TaskLayout::new("/tasks/gpqa", "/shared");
        assert_eq!(t.dataset_dir(), "/tasks/gpqa/data/public");
        assert_eq!(t.task_md(), "/tasks/gpqa/data/public/task.md");
        assert_eq!(
            t.reference_agent(),
            "/tasks/gpqa/reference/reference_target_agent.py"
        );
        assert_eq!(t.sample_execution(), "/shared/sample_agent_execution.json");
    }
}
