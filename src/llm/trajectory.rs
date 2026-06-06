//! Agent trajectory capture in the Anthropic-style `agent_execution.json` shape.
//!
//! [`AgentTrajectory`] accumulates a conversation as a JSON array of message
//! objects, exactly the format the existing consumers
//! ([`crate::orchestrator::load_agent_execution`], `crate::web::runs`) already
//! parse. The `push_*` builder methods are intentionally public and ergonomic
//! because issue #51's middleware will reuse them to record tool-use loops.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::layout::names;

/// A captured agent conversation, serializable to the `agent_execution.json`
/// shape: a JSON array of `{"role", "content"}` message objects.
///
/// `content` is either a plain string or an array of content blocks; the blocks
/// are `text`, `tool_use`, and `tool_result` objects matching what
/// [`crate::orchestrator::load_agent_execution`] and `crate::web::runs` consume.
#[derive(Debug, Clone, Default)]
pub struct AgentTrajectory {
    messages: Vec<Value>,
}

impl AgentTrajectory {
    /// Create an empty trajectory.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Append a user message whose content is plain text.
    pub fn push_user_text(&mut self, text: impl Into<String>) {
        self.messages.push(json!({
            "role": "user",
            "content": text.into(),
        }));
    }

    /// Append an assistant message containing a single `text` content block.
    pub fn push_assistant_text(&mut self, text: impl Into<String>) {
        self.messages.push(json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": text.into()}
            ],
        }));
    }

    /// Append an assistant message containing a single `tool_use` content block.
    pub fn push_assistant_tool_use(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        input: Value,
    ) {
        self.messages.push(json!({
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": id.into(),
                    "name": name.into(),
                    "input": input,
                }
            ],
        }));
    }

    /// Append a user message containing a single `tool_result` content block.
    pub fn push_tool_result(&mut self, tool_use_id: impl Into<String>, content: Value) {
        self.messages.push(json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": tool_use_id.into(),
                    "content": content,
                }
            ],
        }));
    }

    /// Borrow the raw message array.
    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    /// Render the trajectory as the `agent_execution.json` JSON array value.
    pub fn to_agent_execution_json(&self) -> Value {
        Value::Array(self.messages.clone())
    }

    /// Write the trajectory as pretty JSON to `<gen_dir>/agent_execution.json`.
    pub fn write_to(&self, gen_dir: &str) -> std::io::Result<()> {
        let path = Path::new(gen_dir).join(names::AGENT_EXECUTION_JSON);
        let body = serde_json::to_string_pretty(&self.to_agent_execution_json())?;
        fs::write(path, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    #[test]
    fn builder_emits_exact_anthropic_block_shapes() {
        let mut t = AgentTrajectory::new();
        t.push_user_text("hello");
        t.push_assistant_text("hi there");
        t.push_assistant_tool_use("toolu_1", "read_file", json!({"path": "a.txt"}));
        t.push_tool_result("toolu_1", json!("file contents"));

        let expected = json!([
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "hi there"}
            ]},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "a.txt"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "file contents"}
            ]},
        ]);

        assert_eq!(t.to_agent_execution_json(), expected);
    }

    #[test]
    fn default_is_empty() {
        let t = AgentTrajectory::default();
        assert_eq!(t.to_agent_execution_json(), json!([]));
    }

    #[test]
    fn round_trips_through_load_agent_execution() {
        let dir = tempfile::tempdir().unwrap();
        let gen_dir = dir.path().to_str().unwrap();

        let mut t = AgentTrajectory::new();
        t.push_user_text("solve the task");
        t.push_assistant_text("done");

        t.write_to(gen_dir).unwrap();

        let (value, is_multi) =
            crate::orchestrator::load_agent_execution(gen_dir, &Config::default());

        assert!(!is_multi, "single agent_execution.json must not be multi");
        assert_eq!(value, t.to_agent_execution_json());
    }
}
