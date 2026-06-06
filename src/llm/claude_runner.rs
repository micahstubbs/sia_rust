//! Native Claude runner: a `/v1/messages` tool-use loop (issue #39).
//!
//! ## Native Claude runner — reuse decision
//!
//! Before writing this, we evaluated reusing `rig-core`'s Anthropic client
//! (already a dependency, used by [`crate::llm::RigAgentRunner`]) and the
//! general-purpose Anthropic Rust crates. We instead implement a thin, direct
//! Messages API client here for three reasons:
//!
//! 1. Issue #39 requires a controllable `/v1/messages` tool-use loop with an
//!    *injectable* transport ([`MessagesTransport`]) so the whole loop is
//!    testable fully offline with scripted responses — `rig`'s agent abstraction
//!    hides the raw request/response and the HTTP seam.
//! 2. We need the exact `tool_use` / `tool_result` block plumbing and `stop_reason`
//!    control flow, which a thin client expresses directly.
//! 3. We reuse the existing `llm` trajectory types ([`AgentTrajectory`],
//!    [`TrajectoryMiddleware`]) for capture, so only the transport + loop are new.
//!
//! The loop sends the prompt + [`tools::tool_defs`] to the API, executes any
//! returned `tool_use` blocks via the sandboxed [`tools`] executors, feeds back
//! `tool_result` blocks, and repeats until `stop_reason != "tool_use"` or
//! `max_turns` is reached. The whole module is gated behind the `llm` feature.

use std::path::Path;

use serde_json::json;

use crate::config::Config;
use crate::error::{SiaError, SiaResult};
use crate::sandbox::{Capabilities, CapabilityError};

use super::anthropic_api::{
    ApiMessage, ContentBlock, MessagesRequest, MessagesResponse, MessagesTransport,
};
use super::trajectory_middleware::{TokenUsage, TrajectoryEvent, TrajectoryMiddleware};
use super::{telemetry, tools, AgentRunOutcome};

/// Default `max_tokens` per API response. The agent loop bounds *turns*; this
/// bounds the size of any single generation.
const MAX_TOKENS_PER_RESPONSE: u64 = 8192;

/// Render a capability denial as an `Error:`-prefixed tool-result string so it
/// flows back to the model exactly like any other tool error (issue #89).
fn deny(err: CapabilityError) -> String {
    format!("{} {err}", tools::ERROR_PREFIX)
}

/// Dispatch a single `tool_use` block to the matching sandboxed executor,
/// returning the result text.
///
/// Every model-invoked tool call is first checked against the agent's
/// [`Capabilities`] allow-list (issue #89): the relevant `check_*` runs *before*
/// the [`tools`] executor, and a denial is returned as an `Error:`-prefixed
/// string without touching the filesystem or spawning a process. The lexical
/// sandbox inside [`tools`] remains as a defense-in-depth second layer.
fn execute_tool(
    caps: &Capabilities,
    working_dir: &Path,
    name: &str,
    input: &serde_json::Value,
    shell_timeout: u64,
) -> String {
    let s = |key: &str| -> Option<&str> { input.get(key).and_then(|v| v.as_str()) };
    match name {
        "Bash" => match s("command") {
            Some(cmd) => match caps.check_bash(cmd) {
                Ok(()) => tools::bash(working_dir, cmd, shell_timeout),
                Err(e) => deny(e),
            },
            None => format!("{} Bash requires a 'command' string", tools::ERROR_PREFIX),
        },
        "Read" => match s("path") {
            Some(path) => match caps.check_read(path) {
                Ok(()) => tools::read_file(working_dir, path),
                Err(e) => deny(e),
            },
            None => format!("{} Read requires a 'path' string", tools::ERROR_PREFIX),
        },
        "Write" => match (s("path"), s("content")) {
            (Some(path), Some(content)) => match caps
                .check_write(path)
                .and_then(|()| caps.check_size(content.len() as u64))
            {
                Ok(()) => tools::write_file(working_dir, path, content),
                Err(e) => deny(e),
            },
            _ => format!(
                "{} Write requires 'path' and 'content' strings",
                tools::ERROR_PREFIX
            ),
        },
        "Edit" => match (s("path"), s("old_string"), s("new_string")) {
            (Some(path), Some(old), Some(new)) => match caps
                .check_write(path)
                .and_then(|()| caps.check_size(new.len() as u64))
            {
                Ok(()) => tools::edit_file(working_dir, path, old, new),
                Err(e) => deny(e),
            },
            _ => format!(
                "{} Edit requires 'path', 'old_string', and 'new_string' strings",
                tools::ERROR_PREFIX
            ),
        },
        // Glob is intentionally not capability-gated: a glob pattern is not a
        // path (it contains wildcards and may legitimately span subdirectories),
        // so `check_within_root` would be awkward and false-positive-prone. Reads
        // it discovers still flow through `Read`, which *is* gated. The lexical
        // sandbox in `tools::glob` keeps results rooted at `working_dir`.
        "Glob" => match s("pattern") {
            Some(pattern) => tools::glob(working_dir, pattern),
            None => format!("{} Glob requires a 'pattern' string", tools::ERROR_PREFIX),
        },
        other => format!("{} unknown tool '{other}'", tools::ERROR_PREFIX),
    }
}

/// Run the native Claude agent loop against an injectable transport.
///
/// Sends `prompt` + tool definitions to the Messages API, executes tool calls
/// sandboxed to `working_dir`, and loops until the model ends its turn or
/// `max_turns` is reached. Captures the full conversation via a
/// [`TrajectoryMiddleware`] and writes `agent_execution.json` into `working_dir`.
pub fn run_claude_agent(
    transport: &dyn MessagesTransport,
    model: &str,
    max_turns: u32,
    prompt: &str,
    working_dir: &str,
    config: &Config,
) -> SiaResult<AgentRunOutcome> {
    let wd = Path::new(working_dir);
    let tool_defs = tools::tool_defs();

    let mut mw = TrajectoryMiddleware::new();
    mw.start();
    mw.record(TrajectoryEvent::UserPrompt {
        text: prompt.to_string(),
    });

    // The running API conversation (kept separate from the trajectory; the
    // trajectory is the render target, this is what we send to the model).
    let mut messages: Vec<ApiMessage> = vec![ApiMessage::user_text(prompt)];
    let mut final_text = String::new();

    for _turn in 0..max_turns.max(1) {
        let req = MessagesRequest {
            model: model.to_string(),
            max_tokens: MAX_TOKENS_PER_RESPONSE,
            messages: messages.clone(),
            tools: tool_defs.clone(),
            system: None,
        };

        let resp: MessagesResponse = match transport.create_message(&req) {
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
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
        });

        // Record assistant text turns and collect any tool_use blocks.
        let mut tool_uses: Vec<(String, String, serde_json::Value)> = Vec::new();
        for block in &resp.content {
            match block {
                ContentBlock::Text { text } => {
                    final_text = text.clone();
                    mw.record(TrajectoryEvent::AssistantText { text: text.clone() });
                }
                ContentBlock::ToolUse { id, name, input } => {
                    mw.record(TrajectoryEvent::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                    tool_uses.push((id.clone(), name.clone(), input.clone()));
                }
                // The model should not emit tool_result blocks; ignore if it does.
                ContentBlock::ToolResult { .. } => {}
            }
        }

        // Append the assistant message (raw content) to the API conversation.
        messages.push(ApiMessage::assistant(resp.content.clone()));

        let is_tool_use = resp.stop_reason.as_deref() == Some("tool_use") || !tool_uses.is_empty();
        if !is_tool_use {
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

        // Execute each tool call and build the user tool_result message. Every
        // call is gated by the agent's capability allow-list (issue #89).
        let caps = Capabilities::agent_default(wd);
        let mut result_blocks: Vec<ContentBlock> = Vec::new();
        for (id, name, input) in &tool_uses {
            let result = execute_tool(&caps, wd, name, input, config.shell_timeout);
            let is_error = tools::is_error_result(&result);
            mw.record(TrajectoryEvent::ToolResult {
                tool_use_id: id.clone(),
                content: json!(result),
                is_error,
            });
            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: result,
                is_error,
            });
        }
        messages.push(ApiMessage::user(result_blocks));
    }

    // Reached max_turns without the model ending its turn.
    mw.record(TrajectoryEvent::Error {
        message: format!("reached max_turns ({max_turns}) without completing"),
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
    use crate::llm::anthropic_api::{ApiUsage, ToolDef};
    use std::cell::RefCell;

    /// A transport that returns a scripted sequence of responses, one per call.
    struct MockTransport {
        responses: RefCell<std::collections::VecDeque<MessagesResponse>>,
        requests: RefCell<Vec<MessagesRequest>>,
    }

    impl MockTransport {
        fn new(responses: Vec<MessagesResponse>) -> Self {
            Self {
                responses: RefCell::new(responses.into_iter().collect()),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl MessagesTransport for MockTransport {
        fn create_message(&self, req: &MessagesRequest) -> SiaResult<MessagesResponse> {
            self.requests.borrow_mut().push(req.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| SiaError::new("mock transport ran out of scripted responses"))
        }
    }

    fn resp(content: Vec<ContentBlock>, stop_reason: &str) -> MessagesResponse {
        MessagesResponse {
            id: "msg_test".to_string(),
            role: "assistant".to_string(),
            content,
            stop_reason: Some(stop_reason.to_string()),
            usage: ApiUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        }
    }

    #[test]
    fn single_turn_no_tools_writes_trajectory() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().to_str().unwrap();
        let transport = MockTransport::new(vec![resp(
            vec![ContentBlock::Text {
                text: "all done".to_string(),
            }],
            "end_turn",
        )]);

        let outcome = run_claude_agent(
            &transport,
            "claude-test",
            8,
            "do the thing",
            wd,
            &Config::default(),
        )
        .unwrap();

        assert_eq!(outcome.final_text, "all done");
        // agent_execution.json must exist and round-trip through the orchestrator.
        let (value, is_multi) = crate::orchestrator::load_agent_execution(wd, &Config::default());
        assert!(!is_multi);
        assert_eq!(value, outcome.trajectory.to_agent_execution_json());
        assert_eq!(
            value,
            json!([
                {"role": "user", "content": "do the thing"},
                {"role": "assistant", "content": [{"type": "text", "text": "all done"}]},
            ])
        );
    }

    #[test]
    fn run_writes_telemetry_json_with_token_and_turn_counts() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        std::fs::write(wd.join("data.txt"), "scripted file body").unwrap();
        let wd_str = wd.to_str().unwrap();

        // Turn 1: model asks to Read (usage 10/5). Turn 2: model summarizes (10/5).
        let transport = MockTransport::new(vec![
            resp(
                vec![
                    ContentBlock::Text {
                        text: "Reading the file.".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "Read".to_string(),
                        input: json!({"path": "data.txt"}),
                    },
                ],
                "tool_use",
            ),
            resp(
                vec![ContentBlock::Text {
                    text: "done".to_string(),
                }],
                "end_turn",
            ),
        ]);

        run_claude_agent(
            &transport,
            "claude-test",
            8,
            "Read data.txt and summarize",
            wd_str,
            &Config::default(),
        )
        .unwrap();

        // telemetry.json exists and carries the scripted token / turn / tool counts.
        let telemetry_path = wd.join(crate::llm::TELEMETRY_JSON);
        assert!(telemetry_path.is_file(), "telemetry.json must be written");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&telemetry_path).unwrap()).unwrap();

        // Two API calls at 10/5 each = 20 input / 10 output. Two assistant text
        // turns, one tool call.
        assert_eq!(v["cumulative"]["input_tokens"], json!(20));
        assert_eq!(v["cumulative"]["output_tokens"], json!(10));
        assert_eq!(v["cumulative"]["num_api_calls"], json!(2));
        assert_eq!(v["cumulative"]["num_tool_calls"], json!(1));
        // Single generation entry; gen index 0 (temp dir name has no trailing digit).
        assert_eq!(v["generations"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn full_loop_drives_one_tool_call_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        std::fs::write(wd.join("data.txt"), "scripted file body").unwrap();
        let wd_str = wd.to_str().unwrap();

        // Turn 1: model asks to Read data.txt. Turn 2: model summarizes.
        let transport = MockTransport::new(vec![
            resp(
                vec![
                    ContentBlock::Text {
                        text: "Reading the file.".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "Read".to_string(),
                        input: json!({"path": "data.txt"}),
                    },
                ],
                "tool_use",
            ),
            resp(
                vec![ContentBlock::Text {
                    text: "The file says: scripted file body".to_string(),
                }],
                "end_turn",
            ),
        ]);

        let outcome = run_claude_agent(
            &transport,
            "claude-test",
            8,
            "Read data.txt and summarize",
            wd_str,
            &Config::default(),
        )
        .unwrap();

        assert!(outcome.final_text.contains("scripted file body"));

        // Two API calls were made (initial + after tool result).
        assert_eq!(transport.requests.borrow().len(), 2);
        // The second request must carry the tool_result with the real file body.
        let second = &transport.requests.borrow()[1];
        let last = second.messages.last().unwrap();
        assert_eq!(last.role, "user");
        match &last.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "toolu_1");
                assert_eq!(content, "scripted file body");
                assert!(!is_error);
            }
            other => panic!("expected tool_result, got {other:?}"),
        }

        // agent_execution.json written and round-trips through the orchestrator.
        let (value, is_multi) =
            crate::orchestrator::load_agent_execution(wd_str, &Config::default());
        assert!(!is_multi);
        assert_eq!(value, outcome.trajectory.to_agent_execution_json());
        let arr = value.as_array().unwrap();
        // user prompt, assistant text, assistant tool_use, tool_result, assistant final text.
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[2]["content"][0]["type"], "tool_use");
        assert_eq!(arr[3]["content"][0]["type"], "tool_result");
    }

    #[test]
    fn tool_error_is_marked_and_fed_back() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().to_str().unwrap();
        let transport = MockTransport::new(vec![
            resp(
                vec![ContentBlock::ToolUse {
                    id: "toolu_err".to_string(),
                    name: "Read".to_string(),
                    input: json!({"path": "missing.txt"}),
                }],
                "tool_use",
            ),
            resp(
                vec![ContentBlock::Text {
                    text: "couldn't read it".to_string(),
                }],
                "end_turn",
            ),
        ]);

        run_claude_agent(
            &transport,
            "claude-test",
            8,
            "read missing",
            wd,
            &Config::default(),
        )
        .unwrap();

        let second = &transport.requests.borrow()[1];
        match &second.messages.last().unwrap().content[0] {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(is_error, "missing-file read should be flagged is_error");
                assert!(content.contains("file not found"));
            }
            other => panic!("expected tool_result, got {other:?}"),
        }
    }

    #[test]
    fn transport_error_records_error_event_and_propagates() {
        struct FailingTransport;
        impl MessagesTransport for FailingTransport {
            fn create_message(&self, _req: &MessagesRequest) -> SiaResult<MessagesResponse> {
                Err(SiaError::new("boom"))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().to_str().unwrap();
        let result = run_claude_agent(
            &FailingTransport,
            "claude-test",
            8,
            "hi",
            wd,
            &Config::default(),
        );
        assert!(result.is_err());
        // A partial trajectory (prompt + error note) is still written.
        let (value, _is_multi) = crate::orchestrator::load_agent_execution(wd, &Config::default());
        let arr = value.as_array().unwrap();
        assert_eq!(arr[0]["content"], "hi");
        assert!(arr[1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("boom"));
    }

    #[test]
    fn max_turns_exhaustion_still_writes_trajectory() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().to_str().unwrap();
        // Model keeps asking for tools forever; cap at 2 turns.
        let mut responses = Vec::new();
        for i in 0..5 {
            responses.push(resp(
                vec![ContentBlock::ToolUse {
                    id: format!("toolu_{i}"),
                    name: "Bash".to_string(),
                    input: json!({"command": "echo hi"}),
                }],
                "tool_use",
            ));
        }
        let transport = MockTransport::new(responses);
        let outcome =
            run_claude_agent(&transport, "claude-test", 2, "loop", wd, &Config::default()).unwrap();
        // Only 2 API calls despite the model never stopping.
        assert_eq!(transport.requests.borrow().len(), 2);
        // The trajectory ends with the max_turns error note.
        let arr = outcome.trajectory.to_agent_execution_json();
        let last = arr.as_array().unwrap().last().unwrap();
        assert!(last["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("reached max_turns"));
    }

    /// Live end-to-end test against the real Anthropic API. Ignored so CI never
    /// needs a key or network; run with `--ignored` and `ANTHROPIC_API_KEY` set.
    #[test]
    #[ignore = "requires ANTHROPIC_API_KEY and network access"]
    fn live_native_loop_end_to_end() {
        use super::super::HttpMessagesTransport;
        let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
        let transport = HttpMessagesTransport::new(key);
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().to_str().unwrap();
        let outcome = run_claude_agent(
            &transport,
            "claude-haiku-4-5",
            8,
            "Write a file called hello.txt containing the word pong, then tell me you are done.",
            wd,
            &Config::default(),
        )
        .expect("live run should succeed");
        assert!(!outcome.final_text.is_empty());
        assert!(dir.path().join("agent_execution.json").exists());
    }

    #[test]
    fn tool_defs_are_sent_in_request() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().to_str().unwrap();
        let transport = MockTransport::new(vec![resp(
            vec![ContentBlock::Text {
                text: "ok".to_string(),
            }],
            "end_turn",
        )]);
        run_claude_agent(&transport, "claude-test", 8, "hi", wd, &Config::default()).unwrap();
        let first = &transport.requests.borrow()[0];
        let names: Vec<&str> = first
            .tools
            .iter()
            .map(|t: &ToolDef| t.name.as_str())
            .collect();
        assert_eq!(names, ["Bash", "Read", "Write", "Edit", "Glob"]);
    }

    // --- Capability allow-list enforcement (issue #89) ---

    #[test]
    fn execute_tool_denies_bash_when_capability_disallows_it() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let mut caps = Capabilities::agent_default(wd);
        caps.allow_bash = false;

        let result = execute_tool(
            &caps,
            wd,
            "Bash",
            &json!({"command": "echo SHOULD_NOT_RUN > marker.txt"}),
            10,
        );
        assert!(tools::is_error_result(&result), "{result}");
        assert!(result.contains("bash capability denied"));
        // The command did NOT run: no side-effect file was created.
        assert!(!wd.join("marker.txt").exists());
    }

    #[test]
    fn execute_tool_denies_oversize_write() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let mut caps = Capabilities::agent_default(wd);
        caps.max_file_bytes = 4;

        let result = execute_tool(
            &caps,
            wd,
            "Write",
            &json!({"path": "big.txt", "content": "0123456789"}),
            10,
        );
        assert!(tools::is_error_result(&result), "{result}");
        assert!(result.contains("exceeds"));
        // The write did NOT happen.
        assert!(!wd.join("big.txt").exists());
    }

    #[test]
    fn execute_tool_allows_ops_under_agent_default() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let caps = Capabilities::agent_default(wd);

        let w = execute_tool(
            &caps,
            wd,
            "Write",
            &json!({"path": "ok.txt", "content": "hi"}),
            10,
        );
        assert!(!tools::is_error_result(&w), "{w}");
        assert!(wd.join("ok.txt").exists());

        let b = execute_tool(&caps, wd, "Bash", &json!({"command": "echo hi"}), 10);
        assert!(!tools::is_error_result(&b), "{b}");
        assert!(b.contains("hi"));
    }
}
