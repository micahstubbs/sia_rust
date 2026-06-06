//! End-to-end integration tests for the native LLM client (issue #62).
//!
//! The native LLM runners (claude / openhands / pydantic-ai) live behind the
//! `llm` cargo feature in `src/llm/*` and are wired into dispatch via
//! `agent_impls::run_agent`. The core client work is done; this file is the
//! end-to-end proof that the native loop integrates with the artifacts the
//! orchestrator/context pipeline consume, and that dispatch routes to the
//! native runners (not the feature-off boundary error).
//!
//! Everything here is offline (no network): the loops are driven by in-test
//! mock transports implementing the public [`MessagesTransport`] /
//! [`ChatTransport`] seams with scripted responses. The whole file is gated
//! behind the `llm` feature.

#![cfg(feature = "llm")]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::json;

use sia::agent_impls::run_agent;
use sia::config::Config;
use sia::error::{SiaError, SiaResult};
use sia::llm::{
    run_claude_agent, run_openhands_agent, run_pydantic_ai_agent, ApiUsage, ChatMessage,
    ChatRequest, ChatResponse, ChatTransport, ChatUsage, Choice, ContentBlock, FunctionCall,
    MessagesRequest, MessagesResponse, MessagesTransport, ToolCall,
};
use sia::orchestrator::load_agent_execution;

// --------------------------------------------------------------------------
// Mock transports (replicating the minimal in-crate mocks over the public API)
// --------------------------------------------------------------------------

/// Anthropic Messages transport returning a scripted sequence, one per call.
struct MockMessagesTransport {
    responses: RefCell<VecDeque<MessagesResponse>>,
    requests: RefCell<Vec<MessagesRequest>>,
}

impl MockMessagesTransport {
    fn new(responses: Vec<MessagesResponse>) -> Self {
        Self {
            responses: RefCell::new(responses.into_iter().collect()),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl MessagesTransport for MockMessagesTransport {
    fn create_message(&self, req: &MessagesRequest) -> SiaResult<MessagesResponse> {
        self.requests.borrow_mut().push(req.clone());
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| SiaError::new("mock transport ran out of scripted responses"))
    }
}

fn messages_resp(content: Vec<ContentBlock>, stop_reason: &str) -> MessagesResponse {
    MessagesResponse {
        id: "msg_e2e".to_string(),
        role: "assistant".to_string(),
        content,
        stop_reason: Some(stop_reason.to_string()),
        usage: ApiUsage {
            input_tokens: 10,
            output_tokens: 5,
        },
    }
}

/// OpenAI-compatible Chat transport returning a scripted sequence, one per call.
struct MockChatTransport {
    responses: RefCell<VecDeque<ChatResponse>>,
}

impl MockChatTransport {
    fn new(responses: Vec<ChatResponse>) -> Self {
        Self {
            responses: RefCell::new(responses.into_iter().collect()),
        }
    }
}

impl ChatTransport for MockChatTransport {
    fn create(&self, _req: &ChatRequest) -> SiaResult<ChatResponse> {
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| SiaError::new("mock transport ran out of scripted responses"))
    }
}

fn chat_tool_call_resp(id: &str, name: &str, arguments: &str) -> ChatResponse {
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

fn chat_stop_resp(text: &str) -> ChatResponse {
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

// --------------------------------------------------------------------------
// Test 1 — Meta-agent-style full loop via a scripted mock transport.
// --------------------------------------------------------------------------

/// Drive the native claude meta loop with a scripted transport that, on turn 1,
/// emits two `Write` tool_use blocks — one creating `improvement.md` and one
/// creating the target file `target_agent.py` — then ends on turn 2. Assert both
/// files land on disk with the expected contents, and that the captured
/// trajectory round-trips through `orchestrator::load_agent_execution` (the exact
/// artifact the orchestrator/context pipeline consume).
#[test]
fn native_meta_loop_writes_artifacts_and_trajectory_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let wd_str = wd.to_str().unwrap();

    let improvement_body =
        "# Improvement\n\nMake the target agent handle empty input gracefully.\n";
    let target_body = "def run(task):\n    return f\"solved: {task}\"\n";

    // Turn 1: the meta agent writes improvement.md AND the target file.
    // Turn 2: the meta agent ends its turn.
    let transport = MockMessagesTransport::new(vec![
        messages_resp(
            vec![
                ContentBlock::Text {
                    text: "Writing the improvement notes and the target agent.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_improvement".to_string(),
                    name: "Write".to_string(),
                    input: json!({"path": "improvement.md", "content": improvement_body}),
                },
                ContentBlock::ToolUse {
                    id: "toolu_target".to_string(),
                    name: "Write".to_string(),
                    input: json!({"path": "target_agent.py", "content": target_body}),
                },
            ],
            "tool_use",
        ),
        messages_resp(
            vec![ContentBlock::Text {
                text: "Done: wrote improvement.md and target_agent.py.".to_string(),
            }],
            "end_turn",
        ),
    ]);

    let outcome = run_claude_agent(
        &transport,
        "claude-e2e-test",
        8,
        "Improve the target agent and record the change in improvement.md.",
        wd_str,
        &Config::default(),
    )
    .expect("native meta loop should complete");

    assert!(outcome.final_text.contains("improvement.md"));

    // Both artifacts exist on disk with the exact scripted contents.
    assert_eq!(
        std::fs::read_to_string(wd.join("improvement.md")).unwrap(),
        improvement_body,
        "improvement.md must be written by the meta loop"
    );
    assert_eq!(
        std::fs::read_to_string(wd.join("target_agent.py")).unwrap(),
        target_body,
        "target_agent.py must be written by the meta loop"
    );

    // Two API calls were made (initial + after the two tool results).
    assert_eq!(transport.requests.borrow().len(), 2);

    // agent_execution.json was written and round-trips through the orchestrator
    // (the exact artifact the orchestrator/context pipeline consume).
    let (value, is_multi) = load_agent_execution(wd_str, &Config::default());
    assert!(!is_multi, "single agent_execution.json must not be multi");
    assert_eq!(value, outcome.trajectory.to_agent_execution_json());

    let arr = value.as_array().expect("trajectory is a JSON array");
    // user prompt, assistant text, two tool_use blocks captured, two tool_results,
    // final assistant text. Spot-check the two Write tool_use blocks are present.
    assert_eq!(arr[0]["role"], "user");
    let write_targets: Vec<&str> = arr
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|b| b["type"] == "tool_use" && b["name"] == "Write")
        .filter_map(|b| b["input"]["path"].as_str())
        .collect();
    assert!(
        write_targets.contains(&"improvement.md"),
        "trajectory must capture the improvement.md Write; got {write_targets:?}"
    );
    assert!(
        write_targets.contains(&"target_agent.py"),
        "trajectory must capture the target_agent.py Write; got {write_targets:?}"
    );
    // The tool_result blocks fed back to the model are present too.
    assert!(
        arr.iter()
            .filter_map(|m| m["content"].as_array())
            .flatten()
            .any(|b| b["type"] == "tool_result"),
        "trajectory must capture tool_result blocks"
    );
}

// --------------------------------------------------------------------------
// Test 2 — Dispatch reaches the native path for all three impls.
// --------------------------------------------------------------------------

/// Env mutation is process-global; serialize the dispatch test so parallel test
/// threads can't race on the API-key vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with each of `vars` unset, restoring prior values afterward.
fn with_vars_unset<T>(vars: &[&str], f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|v| ((*v).to_string(), std::env::var(v).ok()))
        .collect();
    for v in vars {
        std::env::remove_var(v);
    }
    let out = f();
    for (v, prior) in saved {
        match prior {
            Some(val) => std::env::set_var(&v, val),
            None => std::env::remove_var(&v),
        }
    }
    out
}

/// With the relevant API key UNSET, `agent_impls::run_agent` must route each impl
/// to its *native* runner and surface that runner's missing-key error — proving
/// dispatch reaches the native path under the `llm` feature (NOT the old
/// "build with --features llm" boundary error).
#[test]
fn dispatch_routes_to_native_runners_missing_key_errors() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path().to_str().unwrap();

    // claude (no provider) authenticates via ANTHROPIC_API_KEY.
    let claude_err = with_vars_unset(&["ANTHROPIC_API_KEY"], || {
        run_agent("claude-e2e", "4", "do work", wd, "claude", None).unwrap_err()
    });
    let msg = claude_err.to_string();
    assert!(
        msg.contains("ANTHROPIC_API_KEY is not set"),
        "claude dispatch must hit the native missing-key error; got: {msg}"
    );
    assert!(
        !msg.contains("--features llm"),
        "claude dispatch must NOT hit the feature-off boundary error; got: {msg}"
    );

    // openhands (no provider) falls back to OPENAI_API_KEY with its own message.
    let openhands_err = with_vars_unset(&["OPENAI_API_KEY"], || {
        run_agent("gpt-e2e", "4", "do work", wd, "openhands", None).unwrap_err()
    });
    let msg = openhands_err.to_string();
    assert!(
        msg.contains("OPENAI_API_KEY is not set for the openhands runner"),
        "openhands dispatch must hit the native missing-key error; got: {msg}"
    );
    assert!(
        !msg.contains("--features llm"),
        "openhands dispatch must NOT hit the feature-off boundary error; got: {msg}"
    );

    // pydantic-ai (no provider) falls back to OPENAI_API_KEY with its own message.
    let pydantic_err = with_vars_unset(&["OPENAI_API_KEY"], || {
        run_agent("gpt-e2e", "4", "do work", wd, "pydantic-ai", None).unwrap_err()
    });
    let msg = pydantic_err.to_string();
    assert!(
        msg.contains("OPENAI_API_KEY is not set for the pydantic-ai runner"),
        "pydantic-ai dispatch must hit the native missing-key error; got: {msg}"
    );
    assert!(
        !msg.contains("--features llm"),
        "pydantic-ai dispatch must NOT hit the feature-off boundary error; got: {msg}"
    );
}

// --------------------------------------------------------------------------
// Test 3 — Parallel openhands + pydantic-ai mock-loop smoke tests.
// --------------------------------------------------------------------------

/// openhands native loop: a scripted `file_editor` create lands the artifact on
/// disk and the run ends cleanly.
#[test]
fn openhands_mock_loop_writes_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();

    let transport = MockChatTransport::new(vec![
        chat_tool_call_resp(
            "call_create",
            "file_editor",
            "{\"command\":\"create\",\"path\":\"out.txt\",\"file_text\":\"openhands body\"}",
        ),
        chat_stop_resp("wrote out.txt"),
    ]);

    let summary = run_openhands_agent(
        &transport,
        "openai/test",
        8,
        "create out.txt",
        wd.to_str().unwrap(),
        "session_0",
        &Config::default(),
    )
    .expect("openhands mock loop should complete");

    assert_eq!(summary.final_text, "wrote out.txt");
    assert_eq!(
        std::fs::read_to_string(wd.join("out.txt")).unwrap(),
        "openhands body",
        "openhands loop must land the artifact on disk"
    );
}

/// pydantic-ai native loop: a scripted `write_file` lands the artifact on disk
/// and the captured trajectory round-trips through the orchestrator.
#[test]
fn pydantic_ai_mock_loop_writes_artifact_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let wd_str = wd.to_str().unwrap();

    let transport = MockChatTransport::new(vec![
        chat_tool_call_resp(
            "call_write",
            "write_file",
            "{\"path\":\"out.txt\",\"content\":\"pydantic body\"}",
        ),
        chat_stop_resp("wrote out.txt"),
    ]);

    let outcome = run_pydantic_ai_agent(
        &transport,
        "openai/test",
        8,
        "write out.txt",
        wd_str,
        &Config::default(),
    )
    .expect("pydantic-ai mock loop should complete");

    assert_eq!(outcome.final_text, "wrote out.txt");
    assert_eq!(
        std::fs::read_to_string(wd.join("out.txt")).unwrap(),
        "pydantic body",
        "pydantic-ai loop must land the artifact on disk"
    );

    let (value, is_multi) = load_agent_execution(wd_str, &Config::default());
    assert!(!is_multi);
    assert_eq!(value, outcome.trajectory.to_agent_execution_json());
}
