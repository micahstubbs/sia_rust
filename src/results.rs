//! Result containers replacing positional tuple returns. Port of `sia/results.py`.

/// Outcome of running a target agent generation.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetAgentResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub error_msg: String,
}

impl TargetAgentResult {
    pub fn new(success: bool, stdout: String, stderr: String, error_msg: String) -> Self {
        TargetAgentResult {
            success,
            stdout,
            stderr,
            error_msg,
        }
    }

    pub fn as_tuple(self) -> (bool, String, String, String) {
        (self.success, self.stdout, self.stderr, self.error_msg)
    }
}

/// The two text blocks the feedback prompt is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackContext {
    pub execution_status: String,
    pub execution_section: String,
}

impl FeedbackContext {
    pub fn new(execution_status: String, execution_section: String) -> Self {
        FeedbackContext {
            execution_status,
            execution_section,
        }
    }

    pub fn as_tuple(self) -> (String, String) {
        (self.execution_status, self.execution_section)
    }
}
