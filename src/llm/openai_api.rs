//! Thin serde model + transport for the OpenAI-compatible Chat Completions API
//! (`POST {base_url}/chat/completions`) with function/tool calling.
//!
//! Issue #40 needs a controllable chat-completions tool loop with an *injectable*
//! transport so the loop is testable fully offline; this module defines exactly
//! the request/response shapes the loop touches plus a [`ChatTransport`] seam:
//!
//! - [`HttpChatTransport`] is the real `reqwest::blocking` implementation, routed
//!   to a provider's `base_url` with bearer auth.
//! - Tests use a mock implementing [`ChatTransport`] to script responses.
//!
//! This module is shared: issue #41 may reuse the same serde types + transport.
//! The whole module is gated behind the non-default `llm` cargo feature.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{SiaError, SiaResult};

/// Default OpenAI public base URL, used when a provider has no `base_url`.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// A `POST {base_url}/chat/completions` request body (only the fields the loop sets).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatRequest {
    /// Model id (already provider-prefixed where needed by `resolve_model`).
    pub model: String,
    /// The running conversation.
    pub messages: Vec<ChatMessage>,
    /// Tool (function) definitions exposed to the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ChatTool>,
    /// Optional tool-choice control (`"auto"`, `"none"`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    /// Upper bound on generated tokens per response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// A single chat message (system/user/assistant/tool).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    /// `"system"`, `"user"`, `"assistant"`, or `"tool"`.
    pub role: String,
    /// Message text. Absent for assistant messages that only carry tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls requested by the assistant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For a `tool` message: the id of the `tool_call` it answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// Build a `user` message with text content.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Build a `tool` result message answering `tool_call_id`.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// A tool call requested by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// Provider-assigned tool-call id (echoed back in the `tool` message).
    pub id: String,
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The function name + JSON-encoded arguments.
    pub function: FunctionCall,
}

/// The function name + raw JSON-string arguments of a [`ToolCall`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    /// Function (tool) name.
    pub name: String,
    /// Arguments, as a JSON *string* (per the OpenAI wire format).
    pub arguments: String,
}

/// A function tool definition exposed to the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatTool {
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The function definition.
    pub function: FunctionDef,
}

impl ChatTool {
    /// Build a `function` tool from a name, description, and JSON-schema parameters.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            kind: "function".to_string(),
            function: FunctionDef {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// A function definition (name, description, JSON-schema parameters).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    /// Function name.
    pub name: String,
    /// Human/model-readable description.
    pub description: String,
    /// JSON Schema describing the function parameters.
    pub parameters: Value,
}

/// A `POST {base_url}/chat/completions` response body (only the fields the loop reads).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatResponse {
    /// One or more completion choices (we read the first).
    pub choices: Vec<Choice>,
    /// Token usage for this response.
    #[serde(default)]
    pub usage: ChatUsage,
}

/// A single completion choice.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Choice {
    /// The assistant message.
    pub message: ChatMessage,
    /// Why generation stopped (`"stop"`, `"tool_calls"`, ...).
    pub finish_reason: Option<String>,
}

/// Per-response token usage.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatUsage {
    /// Input ("prompt") tokens.
    #[serde(default)]
    pub prompt_tokens: u64,
    /// Output ("completion") tokens.
    #[serde(default)]
    pub completion_tokens: u64,
}

/// Injectable transport over the Chat Completions API. The agent loop depends
/// only on this trait so tests can supply scripted responses with zero network.
pub trait ChatTransport {
    /// Send one [`ChatRequest`] and return the parsed [`ChatResponse`].
    fn create(&self, req: &ChatRequest) -> SiaResult<ChatResponse>;
}

/// Real transport: POSTs to `{base_url}/chat/completions` via `reqwest::blocking`,
/// with bearer auth.
#[derive(Debug, Clone)]
pub struct HttpChatTransport {
    base_url: String,
    api_key: String,
    client: reqwest::blocking::Client,
}

impl HttpChatTransport {
    /// Construct with a base URL and API key.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            client: reqwest::blocking::Client::new(),
        }
    }
}

impl ChatTransport for HttpChatTransport {
    fn create(&self, req: &ChatRequest) -> SiaResult<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(req)
            .send()
            .map_err(|e| SiaError::new(format!("chat-completions request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(SiaError::new(format!(
                "chat-completions API returned {status}: {body}"
            )));
        }

        resp.json::<ChatResponse>()
            .map_err(|e| SiaError::new(format!("failed to decode chat-completions response: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserializes_response_with_tool_call() {
        let fixture = json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "terminal",
                            "arguments": "{\"command\": \"echo hi\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 42, "completion_tokens": 7}
        });

        let resp: ChatResponse = serde_json::from_value(fixture).unwrap();
        assert_eq!(resp.usage.prompt_tokens, 42);
        assert_eq!(resp.usage.completion_tokens, 7);
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.content, None);
        assert_eq!(choice.message.tool_calls.len(), 1);
        let tc = &choice.message.tool_calls[0];
        assert_eq!(tc.id, "call_abc");
        assert_eq!(tc.kind, "function");
        assert_eq!(tc.function.name, "terminal");
        // Arguments are a raw JSON string.
        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["command"], "echo hi");
    }

    #[test]
    fn usage_defaults_when_absent() {
        let fixture = json!({
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }]
        });
        let resp: ChatResponse = serde_json::from_value(fixture).unwrap();
        assert_eq!(resp.usage, ChatUsage::default());
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("hi"));
    }

    #[test]
    fn serializes_request_with_tool_message_round_trip() {
        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            max_tokens: Some(1024),
            tool_choice: Some(json!("auto")),
            tools: vec![ChatTool::function(
                "terminal",
                "Run a shell command",
                json!({"type": "object", "properties": {"command": {"type": "string"}}}),
            )],
            messages: vec![
                ChatMessage::user("run echo hi"),
                ChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: "call_1".to_string(),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: "terminal".to_string(),
                            arguments: "{\"command\":\"echo hi\"}".to_string(),
                        },
                    }],
                    tool_call_id: None,
                },
                ChatMessage::tool_result("call_1", "hi\n"),
            ],
        };

        let value = serde_json::to_value(&req).unwrap();
        // Spot-check the wire shape of the tool message.
        let tool_msg = &value["messages"][2];
        assert_eq!(tool_msg["role"], "tool");
        assert_eq!(tool_msg["tool_call_id"], "call_1");
        assert_eq!(tool_msg["content"], "hi\n");
        // The assistant tool-call message omits content (None) and tool_call_id.
        let asst = &value["messages"][1];
        assert!(asst.get("content").is_none());
        assert!(asst.get("tool_call_id").is_none());
        assert_eq!(asst["tool_calls"][0]["id"], "call_1");
        assert_eq!(asst["tool_calls"][0]["type"], "function");

        // Round-trips back to an equal request.
        let back: ChatRequest = serde_json::from_value(value).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn empty_tools_and_no_tool_choice_omitted() {
        let req = ChatRequest {
            model: "m".to_string(),
            max_tokens: None,
            tool_choice: None,
            tools: vec![],
            messages: vec![ChatMessage::user("hi")],
        };
        let value = serde_json::to_value(&req).unwrap();
        assert!(value.get("tools").is_none());
        assert!(value.get("tool_choice").is_none());
        assert!(value.get("max_tokens").is_none());
    }
}
