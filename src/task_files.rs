//! Container for task reference files loaded from disk.
//!
//! Defined separately from `run_setup` (where Python keeps it) so the prompt
//! builders can depend on it without pulling in run-setup's heavier deps. Both
//! `run_setup` and `orchestrator` re-export it to match the Python import surface.

/// Reference files loaded from a task directory, shared by the prompt builders.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskFiles {
    pub sample_task_descriptions: String,
    pub reference_target_agent_py: String,
    /// Loaded via `json.load` in Python — a JSON object or array.
    pub sample_agent_execution: serde_json::Value,
    pub task_md: String,
}

impl TaskFiles {
    pub fn new(
        sample_task_descriptions: impl Into<String>,
        reference_target_agent_py: impl Into<String>,
        sample_agent_execution: serde_json::Value,
        task_md: impl Into<String>,
    ) -> Self {
        TaskFiles {
            sample_task_descriptions: sample_task_descriptions.into(),
            reference_target_agent_py: reference_target_agent_py.into(),
            sample_agent_execution,
            task_md: task_md.into(),
        }
    }
}
