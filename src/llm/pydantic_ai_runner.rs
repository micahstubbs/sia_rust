//! Native PydanticAI-style agent runner (issue #41).
//!
//! # Reuse decision (research-first)
//!
//! There is no current, license-compatible, feature-complete Rust port of
//! PydanticAI: the project is Python-only, and the agent/tool abstractions in
//! `dspy-rs` and `rig-core` are general agent frameworks, not drop-in PydanticAI
//! equivalents (different tool registration, usage-limit, and message-shape
//! semantics). Rather than take on a heavy new agent dependency just to mirror a
//! three-tool loop, this module reuses what the crate already has: the in-tree
//! [`super::openai_api`] `ChatTransport` (the LLM seam from #40), the shared
//! sandboxed executors in [`super::tools`] (#39), and the
//! [`super::trajectory_middleware::TrajectoryMiddleware`] / [`super::AgentTrajectory`]
//! capture (#50/#51). We implement only the three PydanticAI tools
//! (`write_file` / `read_file` / `bash`) and the request limit ourselves, exactly
//! matching Python's `UsageLimits(request_limit=max_turns)`. No new dependency.
//!
//! # Mapping to `sia/agent_impls/pydantic_ai.py`
//!
//! The Python impl builds a PydanticAI `Agent` with `_make_tools(working_dir)`:
//! `write_file(path, content)` -> `"Written {n} characters to '{abs}'."`,
//! `read_file(path)` -> file contents or `"Error: File '{abs}' not found."`,
//! `bash(command)` -> stripped stdout (+ `"\n[stderr]\n{stderr}"`) or
//! `"(no output)"`, timing out as `"Error: Command timed out."`. We reproduce
//! those result strings on top of the sandboxed [`super::tools`] executors. The
//! Python impl persists no trajectory artifact of its own; we capture the run and
//! write the shared `agent_execution.json` (the format the web visualizer and
//! [`crate::orchestrator::load_agent_execution`] already consume), matching the
//! sibling native runners (#39/#40).
//!
//! The whole module is gated behind the non-default `llm` cargo feature.

use std::path::Path;

use serde_json::{json, Value};

use crate::config::Config;
use crate::error::{SiaError, SiaResult};

use super::openai_api::{
    ChatMessage, ChatRequest, ChatResponse, ChatTool, ChatTransport, ToolCall,
};
use super::trajectory_middleware::{TokenUsage, TrajectoryEvent, TrajectoryMiddleware};
use super::{telemetry, tools, AgentRunOutcome};

/// Default `max_tokens` per API response. The loop bounds *requests*; this bounds
/// the size of any single generation.
const MAX_TOKENS_PER_RESPONSE: u64 = 8192;

/// The three PydanticAI tool definitions exposed to the model: `write_file`,
/// `read_file`, and `bash` (all relative to the working directory).
pub fn tool_defs() -> Vec<ChatTool> {
    vec![
        ChatTool::function(
            "write_file",
            "Write (overwrite) a file with the given content. The path is relative to the \
             working directory; parent directories are created as needed.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the working directory."},
                    "content": {"type": "string", "description": "The full file content to write."}
                },
                "required": ["path", "content"]
            }),
        ),
        ChatTool::function(
            "read_file",
            "Read and return the contents of a file (path relative to the working directory).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the working directory."}
                },
                "required": ["path"]
            }),
        ),
        ChatTool::function(
            "bash",
            "Run a bash command in the working directory and return its stdout (plus stderr).",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The shell command to execute."}
                },
                "required": ["command"]
            }),
        ),
    ]
}

/// Write a file under `working_dir`, returning PydanticAI's result string.
///
/// Mirrors Python `write_file`: `"Written {len} characters to '{target}'."` on
/// success (where `target` is the resolved absolute path), or an `Error: ...`
/// string on a sandbox-escape / IO failure (the sandboxed executor's message).
pub fn write_file(working_dir: &Path, path: &str, content: &str) -> String {
    let result = tools::write_file(working_dir, path, content);
    if tools::is_error_result(&result) {
        return result;
    }
    // Match Python's message: character count + resolved absolute target path.
    let target = working_dir.join(path);
    format!(
        "Written {} characters to '{}'.",
        content.chars().count(),
        target.display()
    )
}

/// Read a file under `working_dir`, returning PydanticAI's result string.
///
/// Mirrors Python `read_file`: the file contents on success, or
/// `"Error: File '{target}' not found."` when absent (other IO/sandbox errors
/// fall through as the sandboxed executor's `Error: ...` string).
pub fn read_file(working_dir: &Path, path: &str) -> String {
    let result = tools::read_file(working_dir, path);
    // Re-map the "file not found" message to Python's exact phrasing.
    if result == format!("{} file not found: {path}", tools::ERROR_PREFIX) {
        let target = working_dir.join(path);
        return format!("Error: File '{}' not found.", target.display());
    }
    result
}

/// Run a bash command in `working_dir`, returning PydanticAI's result string.
///
/// Mirrors Python `bash`: stripped combined output, `"(no output)"` when empty,
/// and `"Error: Command timed out."` on timeout. (Unlike Python, the underlying
/// executor also reports a non-zero exit code, which is useful signal for the
/// model and is preserved.)
pub fn bash(working_dir: &Path, command: &str, timeout_secs: u64) -> String {
    let result = tools::bash(working_dir, command, timeout_secs);
    if result.starts_with(&format!("{} command timed out", tools::ERROR_PREFIX)) {
        return "Error: Command timed out.".to_string();
    }
    let trimmed = result.trim();
    if trimmed.is_empty() {
        "(no output)".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Execute one tool call against the PydanticAI tools, returning the result text.
fn execute_tool_call(working_dir: &Path, call: &ToolCall, shell_timeout: u64) -> String {
    let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
    let s = |key: &str| -> Option<&str> { args.get(key).and_then(|v| v.as_str()) };

    match call.function.name.as_str() {
        "write_file" => match (s("path"), s("content")) {
            (Some(path), Some(content)) => write_file(working_dir, path, content),
            _ => format!(
                "{} write_file requires 'path' and 'content'",
                tools::ERROR_PREFIX
            ),
        },
        "read_file" => match s("path") {
            Some(path) => read_file(working_dir, path),
            None => format!("{} read_file requires a 'path'", tools::ERROR_PREFIX),
        },
        "bash" => match s("command") {
            Some(cmd) => bash(working_dir, cmd, shell_timeout),
            None => format!("{} bash requires a 'command'", tools::ERROR_PREFIX),
        },
        other => format!("{} unknown tool '{other}'", tools::ERROR_PREFIX),
    }
}

/// Run the native PydanticAI-style agent loop against an injectable transport.
///
/// Records the prompt, then loops up to `max_turns` **requests** (the
/// `UsageLimits(request_limit=...)` equivalent): each request calls the transport;
/// if the assistant returns `tool_calls`, each is executed (`write_file` /
/// `read_file` / `bash`) and the result fed back as a `tool` message while being
/// recorded into the trajectory; otherwise the loop ends. On finish the captured
/// trajectory is written to `<working_dir>/agent_execution.json`.
///
/// When the request limit is hit with tool calls still pending, the loop stops
/// and records a final note (mirroring PydanticAI raising a `UsageLimitExceeded`)
/// rather than panicking.
pub fn run_pydantic_ai_agent(
    transport: &dyn ChatTransport,
    model: &str,
    max_turns: u32,
    prompt: &str,
    working_dir: &str,
    config: &Config,
) -> SiaResult<AgentRunOutcome> {
    let wd = Path::new(working_dir);
    let tools = tool_defs();

    let mut mw = TrajectoryMiddleware::new();
    mw.start();
    mw.record(TrajectoryEvent::UserPrompt {
        text: prompt.to_string(),
    });

    // The running API conversation (kept separate from the trajectory render).
    let mut messages: Vec<ChatMessage> = vec![ChatMessage::user(prompt)];
    let mut final_text = String::new();

    let request_limit = max_turns.max(1);
    for _request in 0..request_limit {
        let req = ChatRequest {
            model: model.to_string(),
            messages: messages.clone(),
            tools: tools.clone(),
            tool_choice: Some(json!("auto")),
            max_tokens: Some(MAX_TOKENS_PER_RESPONSE),
        };

        let resp: ChatResponse = match transport.create(&req) {
            Ok(r) => r,
            Err(e) => {
                mw.record(TrajectoryEvent::Error {
                    message: e.to_string(),
                });
                let (trajectory, metrics) = mw.finish();
                let _ = trajectory.write_to(working_dir);
                telemetry::write_run_telemetry(working_dir, &metrics);
                return Err(e);
            }
        };

        mw.record_usage(TokenUsage {
            input_tokens: resp.usage.prompt_tokens,
            output_tokens: resp.usage.completion_tokens,
        });

        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| SiaError::new("chat-completions response had no choices"))?;
        let assistant = choice.message;

        // Record any assistant text turn.
        if let Some(text) = &assistant.content {
            if !text.is_empty() {
                final_text = text.clone();
                mw.record(TrajectoryEvent::AssistantText { text: text.clone() });
            }
        }

        if assistant.tool_calls.is_empty() {
            // End of turn.
            let (trajectory, metrics) = mw.finish();
            trajectory
                .write_to(working_dir)
                .map_err(|e| SiaError::new(format!("failed to write agent_execution.json: {e}")))?;
            telemetry::write_run_telemetry(working_dir, &metrics);
            return Ok(AgentRunOutcome {
                final_text,
                trajectory,
            });
        }

        // Append the assistant message (with its tool calls) to the conversation.
        messages.push(assistant.clone());

        // Execute each tool call, record it, and feed the result back.
        for call in &assistant.tool_calls {
            let input: Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
            mw.record(TrajectoryEvent::ToolCall {
                id: call.id.clone(),
                name: call.function.name.clone(),
                input,
            });

            let result = execute_tool_call(wd, call, config.shell_timeout);
            let is_error = tools::is_error_result(&result);
            mw.record(TrajectoryEvent::ToolResult {
                tool_use_id: call.id.clone(),
                content: json!(result),
                is_error,
            });

            messages.push(ChatMessage::tool_result(&call.id, &result));
        }
    }

    // Reached the request limit with tool calls still pending: mirror PydanticAI's
    // UsageLimitExceeded by recording a final note rather than panicking.
    mw.record(TrajectoryEvent::Error {
        message: format!(
            "reached request limit ({request_limit}) without completing \
             (UsageLimits.request_limit equivalent)"
        ),
    });
    let (trajectory, metrics) = mw.finish();
    trajectory
        .write_to(working_dir)
        .map_err(|e| SiaError::new(format!("failed to write agent_execution.json: {e}")))?;
    telemetry::write_run_telemetry(working_dir, &metrics);
    Ok(AgentRunOutcome {
        final_text,
        trajectory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::openai_api::{ChatUsage, Choice, FunctionCall};
    use std::cell::RefCell;

    // ----- Tool closures (mirror Python `_make_tools` semantics) -----

    #[test]
    fn write_file_then_read_file_round_trips_with_python_messages() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();

        let msg = write_file(wd, "notes/a.txt", "hello world");
        assert!(!tools::is_error_result(&msg), "{msg}");
        // Python: "Written {len} characters to '{target}'."
        assert!(msg.starts_with("Written 11 characters to '"));
        assert!(msg.ends_with("a.txt'."));
        // Parent dir created.
        assert!(wd.join("notes").is_dir());

        let read = read_file(wd, "notes/a.txt");
        assert_eq!(read, "hello world");
    }

    #[test]
    fn write_file_counts_characters_not_bytes() {
        let dir = tempfile::tempdir().unwrap();
        // "é" is 2 bytes but 1 character; Python's len() counts characters.
        let msg = write_file(dir.path(), "u.txt", "é");
        assert!(msg.starts_with("Written 1 characters to '"), "{msg}");
    }

    #[test]
    fn read_file_not_found_uses_python_phrasing() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_file(dir.path(), "nope.txt");
        // Python: "Error: File '{target}' not found."
        assert!(result.starts_with("Error: File '"), "{result}");
        assert!(result.ends_with("nope.txt' not found."), "{result}");
        assert!(tools::is_error_result(&result));
    }

    #[test]
    fn bash_returns_output_and_reports_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ok = bash(dir.path(), "echo hello", 10);
        assert_eq!(ok, "hello");

        let bad = bash(dir.path(), "echo oops; exit 3", 10);
        assert!(bad.contains("oops"));
        assert!(bad.contains("exit code: 3"));
    }

    #[test]
    fn bash_empty_output_reports_no_output() {
        let dir = tempfile::tempdir().unwrap();
        let result = bash(dir.path(), "true", 10);
        assert_eq!(result, "(no output)");
    }

    #[test]
    fn bash_timeout_uses_python_message() {
        let dir = tempfile::tempdir().unwrap();
        let result = bash(dir.path(), "sleep 5", 1);
        assert_eq!(result, "Error: Command timed out.");
    }

    // ----- Mocked transport plumbing -----

    /// A transport that returns a scripted sequence of responses, one per call.
    struct MockChatTransport {
        responses: RefCell<std::collections::VecDeque<ChatResponse>>,
        requests: RefCell<Vec<ChatRequest>>,
    }

    impl MockChatTransport {
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: RefCell::new(responses.into_iter().collect()),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl ChatTransport for MockChatTransport {
        fn create(&self, req: &ChatRequest) -> SiaResult<ChatResponse> {
            self.requests.borrow_mut().push(req.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| SiaError::new("mock transport ran out of scripted responses"))
        }
    }

    fn tool_call_resp(id: &str, name: &str, arguments: &str) -> ChatResponse {
        ChatResponse {
            choices: vec![Choice {
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: id.to_string(),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: name.to_string(),
                            arguments: arguments.to_string(),
                        },
                    }],
                    tool_call_id: None,
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: ChatUsage::default(),
        }
    }

    fn stop_resp(text: &str) -> ChatResponse {
        ChatResponse {
            choices: vec![Choice {
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(text.to_string()),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: ChatUsage::default(),
        }
    }

    // ----- Mocked multi-tool loop (acceptance core) -----

    #[test]
    fn mocked_write_then_read_loop_writes_and_round_trips_agent_execution() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();

        let transport = MockChatTransport::new(vec![
            tool_call_resp(
                "call_write",
                "write_file",
                "{\"path\":\"note.txt\",\"content\":\"hello body\"}",
            ),
            tool_call_resp("call_read", "read_file", "{\"path\":\"note.txt\"}"),
            stop_resp("the file says hello body"),
        ]);

        let outcome = run_pydantic_ai_agent(
            &transport,
            "openai/test",
            8,
            "write then read note.txt",
            wd.to_str().unwrap(),
            &Config::default(),
        )
        .unwrap();

        assert_eq!(outcome.final_text, "the file says hello body");

        // The file was actually written to the working dir.
        assert_eq!(
            std::fs::read_to_string(wd.join("note.txt")).unwrap(),
            "hello body"
        );

        // Three requests; the third carries the read result fed back as a tool msg.
        assert_eq!(transport.requests.borrow().len(), 3);
        let third = &transport.requests.borrow()[2];
        let read_result = third.messages.last().unwrap();
        assert_eq!(read_result.role, "tool");
        assert_eq!(read_result.tool_call_id.as_deref(), Some("call_read"));
        assert_eq!(read_result.content.as_deref(), Some("hello body"));

        // telemetry.json is now written next to agent_execution.json (issue #88).
        let telemetry = wd.join(crate::llm::TELEMETRY_JSON);
        assert!(
            telemetry.is_file(),
            "telemetry.json must be written post-run"
        );
        let tv: Value =
            serde_json::from_str(&std::fs::read_to_string(&telemetry).unwrap()).unwrap();
        // One write_file + one read_file tool call were issued.
        assert_eq!(tv["cumulative"]["num_tool_calls"], json!(2));

        // agent_execution.json was written and round-trips through the orchestrator.
        let (value, is_multi) =
            crate::orchestrator::load_agent_execution(wd.to_str().unwrap(), &Config::default());
        assert!(!is_multi, "single agent_execution.json must not be multi");
        assert_eq!(value, outcome.trajectory.to_agent_execution_json());

        // Spot-check the captured shape: a tool_use + matching tool_result are present.
        let arr = value.as_array().unwrap();
        assert!(arr.iter().any(|m| {
            m["content"][0]["type"] == "tool_use" && m["content"][0]["name"] == "write_file"
        }));
        assert!(arr.iter().any(|m| {
            m["content"][0]["type"] == "tool_result" && m["content"][0]["content"] == "hello body"
        }));
    }

    // ----- Request limit (UsageLimits.request_limit equivalent) -----

    #[test]
    fn request_limit_caps_calls_and_records_final_note() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();

        // Model keeps asking for bash forever; cap at 3 requests.
        let responses: Vec<ChatResponse> = (0..6)
            .map(|i| tool_call_resp(&format!("call_{i}"), "bash", "{\"command\":\"echo hi\"}"))
            .collect();
        let transport = MockChatTransport::new(responses);

        let outcome = run_pydantic_ai_agent(
            &transport,
            "openai/test",
            3,
            "loop forever",
            wd.to_str().unwrap(),
            &Config::default(),
        )
        .unwrap();

        // Exactly max_turns requests despite the model never stopping.
        assert_eq!(transport.requests.borrow().len(), 3);

        // A final note recording the request-limit was captured.
        let value = outcome.trajectory.to_agent_execution_json();
        let arr = value.as_array().unwrap();
        let last = arr.last().unwrap();
        assert_eq!(last["content"][0]["type"], "text");
        let note = last["content"][0]["text"].as_str().unwrap();
        assert!(note.contains("reached request limit"), "{note}");
        assert!(note.contains("request_limit"), "{note}");
    }

    #[test]
    fn unknown_and_malformed_tool_calls_report_errors_without_panic() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();

        let transport = MockChatTransport::new(vec![
            // write_file missing the 'content' argument.
            tool_call_resp("call_bad", "write_file", "{\"path\":\"x.txt\"}"),
            stop_resp("done"),
        ]);

        let outcome = run_pydantic_ai_agent(
            &transport,
            "openai/test",
            4,
            "bad call",
            wd.to_str().unwrap(),
            &Config::default(),
        )
        .unwrap();

        let value = outcome.trajectory.to_agent_execution_json();
        let arr = value.as_array().unwrap();
        // The tool_result records the error string.
        assert!(arr.iter().any(|m| {
            m["content"][0]["type"] == "tool_result"
                && m["content"][0]["content"]
                    .as_str()
                    .map(|c| c.contains("requires 'path' and 'content'"))
                    .unwrap_or(false)
        }));
    }

    /// Live end-to-end test against a real OpenAI-compatible provider. Ignored so
    /// CI never needs a key or network; run with `--ignored`, a provider base URL
    /// in `SIA_TEST_BASE_URL`, model in `SIA_TEST_MODEL`, key in `SIA_TEST_API_KEY`.
    #[test]
    #[ignore = "requires a provider base URL, model, and API key + network access"]
    fn live_pydantic_ai_loop_end_to_end() {
        use super::super::openai_api::HttpChatTransport;
        let base_url = std::env::var("SIA_TEST_BASE_URL").expect("SIA_TEST_BASE_URL must be set");
        let model = std::env::var("SIA_TEST_MODEL").expect("SIA_TEST_MODEL must be set");
        let key = std::env::var("SIA_TEST_API_KEY").expect("SIA_TEST_API_KEY must be set");
        let transport = HttpChatTransport::new(base_url, key);
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_pydantic_ai_agent(
            &transport,
            &model,
            8,
            "Create a file hello.txt containing the word pong, then tell me you are done.",
            dir.path().to_str().unwrap(),
            &Config::default(),
        )
        .expect("live run should succeed");
        assert!(!outcome.trajectory.messages().is_empty());
    }
}
