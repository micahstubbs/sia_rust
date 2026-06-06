//! Native OpenHands-style multi-provider agent runner (issue #40).
//!
//! This is the Rust replacement for the Python OpenHands SDK integration
//! (`sia/agent_impls/openhands.py`). The Python impl built an OpenHands `LLM`
//! (litellm) + `Agent` with a terminal + file-editor tool, ran a conversation,
//! and let the SDK persist a trajectory under
//! `agent_working_directory/openhands_trajectory/<session>/events/event-*.json`.
//!
//! We reproduce that behavior natively:
//!
//! - The role litellm played (routing a model spec to an OpenAI-compatible
//!   `base_url`) is handled by [`super::openai_api::HttpChatTransport`] plus the
//!   `resolve_model` prefixing in [`crate::agent_impls::openhands`].
//! - A `terminal` tool and a `file_editor` tool (commands `view` / `create` /
//!   `str_replace`) are exposed to the model, implemented by reusing the shared
//!   sandboxed executors in [`super::tools`].
//! - An agentic loop runs up to `max_turns`.
//! - Events are persisted in the OpenHands shape via [`OpenHandsEventLog`] so
//!   [`crate::web::runs::list_openhands_sessions`] and
//!   [`crate::web::runs::get_openhands_events`] render them.
//!
//! ## Mirrored event schema
//!
//! The OpenHands SDK writes one JSON object per event under `events/`. We mirror
//! its key fields:
//!
//! - `id`: zero-based monotonic event index.
//! - `timestamp`: ISO-8601-ish UTC string.
//! - `source`: `"user"`, `"agent"`, or `"environment"`.
//! - For an **action** (user message / agent tool call): `action` (the action
//!   name, e.g. `"message"`, `"run"`, `"edit"`), `args` (a map of arguments),
//!   and `message` (a human-readable summary).
//! - For an **observation** (tool output): `observation` (e.g. `"run"`,
//!   `"edit"`), `content` (the result text), and `extras` (e.g. the
//!   originating `tool_call_id`).
//!
//! The whole module is gated behind the non-default `llm` cargo feature.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::Config;
use crate::error::{SiaError, SiaResult};

use super::openai_api::{
    ChatMessage, ChatRequest, ChatResponse, ChatTool, ChatTransport, ToolCall,
};
use super::trajectory_middleware::{RunMetrics, TokenUsage};
use super::{telemetry, tools};

/// Default `max_tokens` per API response. The loop bounds *turns*; this bounds
/// the size of any single generation.
const MAX_TOKENS_PER_RESPONSE: u64 = 8192;

/// A writer for OpenHands-style events under
/// `<working_dir>/openhands_trajectory/<session>/events/`.
///
/// Each call to [`OpenHandsEventLog::write_event`] writes the next
/// `event-NNNNN.json` (zero-padded, monotonically incrementing) so the files
/// sort in emission order and are read back by
/// [`crate::web::runs::get_openhands_events`].
pub struct OpenHandsEventLog {
    events_dir: PathBuf,
    counter: u64,
}

impl OpenHandsEventLog {
    /// Create the events directory and return a fresh log.
    pub fn create(working_dir: &Path, session: &str) -> SiaResult<Self> {
        let events_dir = working_dir
            .join("openhands_trajectory")
            .join(session)
            .join("events");
        std::fs::create_dir_all(&events_dir).map_err(|e| {
            SiaError::new(format!(
                "failed to create openhands events dir {}: {e}",
                events_dir.display()
            ))
        })?;
        Ok(Self {
            events_dir,
            counter: 0,
        })
    }

    /// Current ISO-8601-ish UTC timestamp (seconds since the Unix epoch as an
    /// RFC3339-style string, with no external time crate dependency).
    fn timestamp() -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        // A monotone, sortable, machine-readable stamp. Not wall-clock-formatted
        // (we avoid a chrono dependency), but unambiguous and stable.
        format!("{}.{:09}Z", now.as_secs(), now.subsec_nanos())
    }

    /// Write `event` as the next `event-NNNNN.json`, injecting `id` and
    /// `timestamp` if absent. Returns the assigned event id.
    pub fn write_event(&mut self, mut event: Value) -> SiaResult<u64> {
        let id = self.counter;
        if let Some(obj) = event.as_object_mut() {
            obj.entry("id").or_insert_with(|| json!(id));
            obj.entry("timestamp")
                .or_insert_with(|| json!(Self::timestamp()));
        }
        let filename = format!("event-{id:05}.json");
        let path = self.events_dir.join(&filename);
        let text = serde_json::to_string_pretty(&event)
            .map_err(|e| SiaError::new(format!("failed to serialize event: {e}")))?;
        std::fs::write(&path, text)
            .map_err(|e| SiaError::new(format!("failed to write {}: {e}", path.display())))?;
        self.counter += 1;
        Ok(id)
    }

    /// A `user`-sourced message action (the initial task prompt).
    pub fn user_message(&mut self, text: &str) -> SiaResult<u64> {
        self.write_event(json!({
            "source": "user",
            "action": "message",
            "args": {"content": text},
            "message": text,
        }))
    }

    /// An `agent`-sourced action for an issued tool call (terminal run / file edit).
    pub fn agent_action(
        &mut self,
        action: &str,
        tool_call_id: &str,
        args: Value,
        message: &str,
    ) -> SiaResult<u64> {
        self.write_event(json!({
            "source": "agent",
            "action": action,
            "tool_call_id": tool_call_id,
            "args": args,
            "message": message,
        }))
    }

    /// An `environment`-sourced observation for a tool result.
    pub fn environment_observation(
        &mut self,
        observation: &str,
        tool_call_id: &str,
        content: &str,
    ) -> SiaResult<u64> {
        self.write_event(json!({
            "source": "environment",
            "observation": observation,
            "content": content,
            "extras": {"tool_call_id": tool_call_id},
            "message": content,
        }))
    }

    /// A final `agent`-sourced message (the assistant's closing text / finish).
    pub fn agent_message(&mut self, text: &str) -> SiaResult<u64> {
        self.write_event(json!({
            "source": "agent",
            "action": "message",
            "args": {"content": text},
            "message": text,
        }))
    }
}

/// The two tool definitions exposed to the model: `terminal` (bash) and
/// `file_editor` (view/create/str_replace).
fn tool_defs() -> Vec<ChatTool> {
    vec![
        ChatTool::function(
            "terminal",
            "Run a shell command in the working directory and return its combined \
             stdout and stderr.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The shell command to execute."}
                },
                "required": ["command"]
            }),
        ),
        ChatTool::function(
            "file_editor",
            "View, create, or edit a file (path relative to the working directory). \
             `view` reads a file, `create` writes file_text, `str_replace` replaces a \
             unique old_str with new_str.",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": ["view", "create", "str_replace"],
                        "description": "The file-editor command to run."
                    },
                    "path": {"type": "string", "description": "File path relative to the working directory."},
                    "file_text": {"type": "string", "description": "Full file content (for `create`)."},
                    "old_str": {"type": "string", "description": "Exact unique text to replace (for `str_replace`)."},
                    "new_str": {"type": "string", "description": "Replacement text (for `str_replace`)."}
                },
                "required": ["command", "path"]
            }),
        ),
    ]
}

/// Execute one tool call against the sandboxed executors, returning
/// `(observation_name, result_text)`.
fn execute_tool_call(working_dir: &Path, call: &ToolCall, shell_timeout: u64) -> (String, String) {
    let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
    let s = |key: &str| -> Option<&str> { args.get(key).and_then(|v| v.as_str()) };

    match call.function.name.as_str() {
        "terminal" => {
            let result = match s("command") {
                Some(cmd) => tools::bash(working_dir, cmd, shell_timeout),
                None => format!(
                    "{} terminal requires a 'command' string",
                    tools::ERROR_PREFIX
                ),
            };
            ("run".to_string(), result)
        }
        "file_editor" => {
            let result = match s("command") {
                Some("view") => match s("path") {
                    Some(path) => tools::read_file(working_dir, path),
                    None => format!("{} file_editor view requires a 'path'", tools::ERROR_PREFIX),
                },
                Some("create") => match (s("path"), s("file_text")) {
                    (Some(path), Some(text)) => tools::write_file(working_dir, path, text),
                    _ => format!(
                        "{} file_editor create requires 'path' and 'file_text'",
                        tools::ERROR_PREFIX
                    ),
                },
                Some("str_replace") => match (s("path"), s("old_str"), s("new_str")) {
                    (Some(path), Some(old), Some(new)) => {
                        tools::edit_file(working_dir, path, old, new)
                    }
                    _ => format!(
                        "{} file_editor str_replace requires 'path', 'old_str', and 'new_str'",
                        tools::ERROR_PREFIX
                    ),
                },
                Some(other) => format!(
                    "{} file_editor unknown command '{other}' (expected view/create/str_replace)",
                    tools::ERROR_PREFIX
                ),
                None => format!("{} file_editor requires a 'command'", tools::ERROR_PREFIX),
            };
            ("edit".to_string(), result)
        }
        other => (
            "run".to_string(),
            format!("{} unknown tool '{other}'", tools::ERROR_PREFIX),
        ),
    }
}

/// A short summary of an OpenHands-style agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenHandsRunSummary {
    /// The agent's final textual response (may be empty).
    pub final_text: String,
    /// Number of model turns executed.
    pub turns: u32,
    /// Number of events written to the log.
    pub events_written: u64,
}

/// Run the native OpenHands-style agent loop against an injectable transport.
///
/// Writes a `user` message event for the prompt, then loops up to `max_turns`:
/// each turn calls the transport; if the assistant returns `tool_calls`, each is
/// executed (terminal / file_editor) and an action + observation event pair is
/// written, with the result fed back as a `tool` message; otherwise a final
/// `agent` message event is written and the loop ends.
pub fn run_openhands_agent(
    transport: &dyn ChatTransport,
    model: &str,
    max_turns: u32,
    prompt: &str,
    working_dir: &str,
    session: &str,
    config: &Config,
) -> SiaResult<OpenHandsRunSummary> {
    let wd = Path::new(working_dir);
    let tools = tool_defs();
    let mut log = OpenHandsEventLog::create(wd, session)?;

    log.user_message(prompt)?;

    let mut messages: Vec<ChatMessage> = vec![ChatMessage::user(prompt)];
    let mut final_text = String::new();
    let mut turns = 0u32;

    // This runner persists events via the OpenHands event log rather than a
    // [`TrajectoryMiddleware`], so we accumulate the same telemetry shape inline:
    // provider-reported token usage from each `ChatResponse.usage`, the number of
    // model API calls (turns), and the number of tool calls issued, plus
    // wall-clock timing. This feeds `telemetry::write_run_telemetry` at every
    // exit so a `telemetry.json` lands next to `openhands_trajectory/`.
    let started = std::time::Instant::now();
    let mut metrics = RunMetrics::default();

    for _ in 0..max_turns.max(1) {
        turns += 1;
        metrics.num_turns += 1;
        let req = ChatRequest {
            model: model.to_string(),
            messages: messages.clone(),
            tools: tools.clone(),
            tool_choice: Some(json!("auto")),
            max_tokens: Some(MAX_TOKENS_PER_RESPONSE),
        };

        let resp: ChatResponse = transport.create(&req)?;
        metrics.usage.add(TokenUsage {
            input_tokens: resp.usage.prompt_tokens,
            output_tokens: resp.usage.completion_tokens,
        });
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| SiaError::new("chat-completions response had no choices"))?;
        let assistant = choice.message;

        if assistant.tool_calls.is_empty() {
            // End of turn: record any final text as an agent message.
            final_text = assistant.content.clone().unwrap_or_default();
            log.agent_message(&final_text)?;
            messages.push(assistant);
            metrics.duration_ms = started.elapsed().as_millis();
            telemetry::write_run_telemetry(working_dir, &metrics);
            return Ok(OpenHandsRunSummary {
                final_text,
                turns,
                events_written: log.counter,
            });
        }

        // Append the assistant message (with its tool calls) to the conversation.
        messages.push(assistant.clone());

        // Execute each tool call, write action + observation events, feed results back.
        for call in &assistant.tool_calls {
            metrics.num_tool_calls += 1;
            let args: Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
            let action_name = match call.function.name.as_str() {
                "terminal" => "run",
                "file_editor" => "edit",
                _ => "run",
            };
            let action_msg = format!("{}({})", call.function.name, call.function.arguments);
            log.agent_action(action_name, &call.id, args, &action_msg)?;

            let (observation, result) = execute_tool_call(wd, call, config.shell_timeout);
            log.environment_observation(&observation, &call.id, &result)?;

            messages.push(ChatMessage::tool_result(&call.id, &result));
        }
    }

    // Reached max_turns without the model ending its turn.
    let note = format!("reached max_turns ({max_turns}) without completing");
    log.agent_message(&note)?;
    metrics.duration_ms = started.elapsed().as_millis();
    telemetry::write_run_telemetry(working_dir, &metrics);
    Ok(OpenHandsRunSummary {
        final_text,
        turns,
        events_written: log.counter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::openai_api::{ChatUsage, Choice, FunctionCall};
    use std::cell::RefCell;

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

    /// Build the `<runs_root>/<run>/<gen>` directory layout the web read-back
    /// functions expect, returning `(runs_root_tempdir, run_name, gen_name, gen_dir)`.
    fn runs_layout() -> (tempfile::TempDir, String, String, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let run_name = "run_1";
        let gen_name = "gen_0";
        let gen_dir = root.path().join(run_name).join(gen_name);
        std::fs::create_dir_all(&gen_dir).unwrap();
        (root, run_name.to_string(), gen_name.to_string(), gen_dir)
    }

    #[test]
    fn event_log_round_trips_through_web_read_functions() {
        let (root, run, gen, gen_dir) = runs_layout();
        let session = "session_0";

        let mut log = OpenHandsEventLog::create(&gen_dir, session).unwrap();
        log.user_message("do the task").unwrap();
        log.agent_action(
            "run",
            "call_1",
            json!({"command": "echo hi"}),
            "terminal(echo hi)",
        )
        .unwrap();
        log.environment_observation("run", "call_1", "hi\n")
            .unwrap();
        log.agent_message("all done").unwrap();

        // Session is listed.
        let sessions = crate::web::runs::list_openhands_sessions(root.path(), &run, &gen).unwrap();
        assert_eq!(sessions, vec![session.to_string()]);

        // Events come back in order with the expected fields.
        let events =
            crate::web::runs::get_openhands_events(root.path(), &run, &gen, session).unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["source"], "user");
        assert_eq!(events[0]["action"], "message");
        assert_eq!(events[0]["message"], "do the task");
        assert_eq!(events[0]["id"], 0);

        assert_eq!(events[1]["source"], "agent");
        assert_eq!(events[1]["action"], "run");
        assert_eq!(events[1]["tool_call_id"], "call_1");
        assert_eq!(events[1]["args"]["command"], "echo hi");
        assert_eq!(events[1]["id"], 1);

        assert_eq!(events[2]["source"], "environment");
        assert_eq!(events[2]["observation"], "run");
        assert_eq!(events[2]["content"], "hi\n");
        assert_eq!(events[2]["extras"]["tool_call_id"], "call_1");

        assert_eq!(events[3]["source"], "agent");
        assert_eq!(events[3]["message"], "all done");
        assert_eq!(events[3]["id"], 3);

        // Each event carries a timestamp.
        for e in &events {
            assert!(e.get("timestamp").is_some());
        }
    }

    #[test]
    fn one_turn_terminal_loop_runs_tool_and_writes_events() {
        let (root, run, gen, gen_dir) = runs_layout();
        let session = "session_0";

        let transport = MockChatTransport::new(vec![
            tool_call_resp("call_1", "terminal", "{\"command\":\"echo hi\"}"),
            stop_resp("finished"),
        ]);

        let summary = run_openhands_agent(
            &transport,
            "openai/test",
            8,
            "run echo hi",
            gen_dir.to_str().unwrap(),
            session,
            &Config::default(),
        )
        .unwrap();

        assert_eq!(summary.final_text, "finished");
        assert_eq!(summary.turns, 2);

        // telemetry.json is written next to openhands_trajectory/ (issue #88),
        // sourced from the inline RunMetrics accumulation.
        let telemetry = gen_dir.join(crate::llm::TELEMETRY_JSON);
        assert!(
            telemetry.is_file(),
            "telemetry.json must be written post-run"
        );
        let tv: Value =
            serde_json::from_str(&std::fs::read_to_string(&telemetry).unwrap()).unwrap();
        assert_eq!(tv["cumulative"]["num_api_calls"], json!(2));
        assert_eq!(tv["cumulative"]["num_tool_calls"], json!(1));
        // gen_dir is `.../gen_0`, so the derived generation index is 0.
        assert_eq!(tv["generations"][0]["generation"], json!(0));

        // Two API calls; the second carries the tool result with the command output.
        assert_eq!(transport.requests.borrow().len(), 2);
        let second = &transport.requests.borrow()[1];
        let tool_msg = second.messages.last().unwrap();
        assert_eq!(tool_msg.role, "tool");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(tool_msg.content.as_deref().unwrap().trim(), "hi");

        // Events are readable via the web function: user, action, observation, final message.
        let events =
            crate::web::runs::get_openhands_events(root.path(), &run, &gen, session).unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["source"], "user");
        assert_eq!(events[1]["action"], "run");
        assert_eq!(events[2]["observation"], "run");
        assert_eq!(events[2]["content"].as_str().unwrap().trim(), "hi");
        assert_eq!(events[3]["message"], "finished");
    }

    #[test]
    fn file_editor_create_then_view_round_trips_through_loop() {
        let (root, run, gen, gen_dir) = runs_layout();
        let session = "session_0";

        let transport = MockChatTransport::new(vec![
            tool_call_resp(
                "call_create",
                "file_editor",
                "{\"command\":\"create\",\"path\":\"note.txt\",\"file_text\":\"hello body\"}",
            ),
            tool_call_resp(
                "call_view",
                "file_editor",
                "{\"command\":\"view\",\"path\":\"note.txt\"}",
            ),
            stop_resp("the file says hello body"),
        ]);

        let summary = run_openhands_agent(
            &transport,
            "openai/test",
            8,
            "create then view note.txt",
            gen_dir.to_str().unwrap(),
            session,
            &Config::default(),
        )
        .unwrap();

        assert_eq!(summary.turns, 3);
        // The file was actually created in the working dir.
        assert_eq!(
            std::fs::read_to_string(gen_dir.join("note.txt")).unwrap(),
            "hello body"
        );

        // The third request carries the view observation result (file body).
        let third = &transport.requests.borrow()[2];
        let view_result = third.messages.last().unwrap();
        assert_eq!(view_result.role, "tool");
        assert_eq!(view_result.content.as_deref(), Some("hello body"));

        // Events: user, create-action, create-obs, view-action, view-obs, final.
        let events =
            crate::web::runs::get_openhands_events(root.path(), &run, &gen, session).unwrap();
        assert_eq!(events.len(), 6);
        assert_eq!(events[1]["action"], "edit");
        assert_eq!(events[1]["args"]["command"], "create");
        assert_eq!(events[4]["observation"], "edit");
        assert_eq!(events[4]["content"], "hello body");
    }

    #[test]
    fn max_turns_exhaustion_writes_final_note() {
        let (root, run, gen, gen_dir) = runs_layout();
        let session = "session_0";

        // Model keeps asking for the terminal tool forever; cap at 2 turns.
        let responses: Vec<ChatResponse> = (0..5)
            .map(|i| {
                tool_call_resp(
                    &format!("call_{i}"),
                    "terminal",
                    "{\"command\":\"echo hi\"}",
                )
            })
            .collect();
        let transport = MockChatTransport::new(responses);

        let summary = run_openhands_agent(
            &transport,
            "openai/test",
            2,
            "loop forever",
            gen_dir.to_str().unwrap(),
            session,
            &Config::default(),
        )
        .unwrap();

        assert_eq!(summary.turns, 2);
        // Only 2 API calls despite the model never stopping.
        assert_eq!(transport.requests.borrow().len(), 2);

        let events =
            crate::web::runs::get_openhands_events(root.path(), &run, &gen, session).unwrap();
        let last = events.last().unwrap();
        assert!(last["message"]
            .as_str()
            .unwrap()
            .contains("reached max_turns"));
    }

    /// Live end-to-end test against a real OpenAI-compatible provider. Ignored so
    /// CI never needs a key or network; run with `--ignored`, a provider base URL
    /// in `SIA_TEST_BASE_URL`, model in `SIA_TEST_MODEL`, key in `SIA_TEST_API_KEY`.
    #[test]
    #[ignore = "requires a provider base URL, model, and API key + network access"]
    fn live_openhands_loop_end_to_end() {
        use super::super::openai_api::HttpChatTransport;
        let base_url = std::env::var("SIA_TEST_BASE_URL").expect("SIA_TEST_BASE_URL must be set");
        let model = std::env::var("SIA_TEST_MODEL").expect("SIA_TEST_MODEL must be set");
        let key = std::env::var("SIA_TEST_API_KEY").expect("SIA_TEST_API_KEY must be set");
        let transport = HttpChatTransport::new(base_url, key);
        let dir = tempfile::tempdir().unwrap();
        let summary = run_openhands_agent(
            &transport,
            &model,
            8,
            "Create a file hello.txt containing the word pong, then tell me you are done.",
            dir.path().to_str().unwrap(),
            "session_0",
            &Config::default(),
        )
        .expect("live run should succeed");
        assert!(summary.events_written > 0);
    }
}
