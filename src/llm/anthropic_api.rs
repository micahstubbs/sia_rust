//! Thin serde model + transport for the Anthropic Messages API (`/v1/messages`).
//!
//! Issue #39 needs a controllable `/v1/messages` tool-use loop with an
//! *injectable* transport so the loop is testable fully offline. This module
//! defines exactly the request/response shapes the loop touches plus a
//! [`MessagesTransport`] seam:
//!
//! - [`HttpMessagesTransport`] is the real `reqwest::blocking` implementation.
//! - Tests use a mock implementing [`MessagesTransport`] to script responses.
//!
//! The whole module is gated behind the non-default `llm` cargo feature.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{SiaError, SiaResult};

/// Default Anthropic API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// A `POST /v1/messages` request body (only the fields the loop sets).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessagesRequest {
    /// Model id (e.g. `claude-haiku-4-5-20251001`).
    pub model: String,
    /// Upper bound on generated tokens.
    pub max_tokens: u64,
    /// The running conversation.
    pub messages: Vec<ApiMessage>,
    /// Tool definitions exposed to the model.
    pub tools: Vec<ToolDef>,
    /// Optional system preamble.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

/// A single conversation message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiMessage {
    /// `"user"` or `"assistant"`.
    pub role: String,
    /// Ordered content blocks.
    pub content: Vec<ContentBlock>,
}

impl ApiMessage {
    /// Build a `user` message containing a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Build an `assistant` message from raw content blocks.
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
        }
    }

    /// Build a `user` message from raw content blocks (e.g. tool results).
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self {
            role: "user".to_string(),
            content,
        }
    }
}

/// A single content block in a message. Serialized with an internal `type` tag,
/// matching the Anthropic Messages API wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// The model requested a tool call.
    ToolUse {
        /// Provider-assigned tool-use id.
        id: String,
        /// Tool name.
        name: String,
        /// JSON input for the tool.
        input: Value,
    },
    /// A tool result fed back to the model.
    ToolResult {
        /// The id of the `tool_use` block being answered.
        tool_use_id: String,
        /// Result text.
        content: String,
        /// Whether the tool reported an error.
        #[serde(default)]
        is_error: bool,
    },
}

/// An Anthropic tool definition (name, description, JSON schema).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    /// Tool name (must match what the executor dispatches on).
    pub name: String,
    /// Human/model-readable description.
    pub description: String,
    /// JSON Schema describing the tool input.
    pub input_schema: Value,
}

/// A `POST /v1/messages` response body (only the fields the loop reads).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessagesResponse {
    /// Response id.
    pub id: String,
    /// Always `"assistant"`.
    pub role: String,
    /// The assistant's content blocks.
    pub content: Vec<ContentBlock>,
    /// Why generation stopped (e.g. `"tool_use"`, `"end_turn"`).
    pub stop_reason: Option<String>,
    /// Token usage for this response.
    #[serde(default)]
    pub usage: ApiUsage,
}

/// Per-response token usage.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiUsage {
    /// Input ("prompt") tokens.
    #[serde(default)]
    pub input_tokens: u64,
    /// Output ("completion") tokens.
    #[serde(default)]
    pub output_tokens: u64,
}

/// Injectable transport over the Messages API. The agent loop depends only on
/// this trait so tests can supply scripted responses with zero network.
pub trait MessagesTransport {
    /// Send one `MessagesRequest` and return the parsed response.
    fn create_message(&self, req: &MessagesRequest) -> SiaResult<MessagesResponse>;
}

/// Real transport: POSTs to `{base_url}/v1/messages` via `reqwest::blocking`.
#[derive(Debug, Clone)]
pub struct HttpMessagesTransport {
    api_key: String,
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpMessagesTransport {
    /// Construct with an explicit API key and the default base URL.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    /// Construct with an explicit API key and a custom base URL.
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            client: reqwest::blocking::Client::new(),
        }
    }
}

impl MessagesTransport for HttpMessagesTransport {
    fn create_message(&self, req: &MessagesRequest) -> SiaResult<MessagesResponse> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(req)
            .send()
            .map_err(|e| SiaError::new(format!("Anthropic request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(SiaError::new(format!(
                "Anthropic API returned {status}: {body}"
            )));
        }

        resp.json::<MessagesResponse>()
            .map_err(|e| SiaError::new(format!("failed to decode Anthropic response: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserializes_recorded_tool_use_response() {
        // A recorded-shape /v1/messages response that requests a tool call.
        let fixture = json!({
            "id": "msg_01ABC",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Let me read that file."},
                {
                    "type": "tool_use",
                    "id": "toolu_01",
                    "name": "Read",
                    "input": {"path": "a.txt"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 120, "output_tokens": 18}
        });

        let resp: MessagesResponse = serde_json::from_value(fixture).unwrap();
        assert_eq!(resp.id, "msg_01ABC");
        assert_eq!(resp.role, "assistant");
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(resp.usage.input_tokens, 120);
        assert_eq!(resp.usage.output_tokens, 18);
        assert_eq!(resp.content.len(), 2);
        match &resp.content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_01");
                assert_eq!(name, "Read");
                assert_eq!(input["path"], "a.txt");
            }
            other => panic!("expected tool_use, got {other:?}"),
        }
    }

    #[test]
    fn usage_defaults_when_absent() {
        let fixture = json!({
            "id": "msg_x",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "hi"}],
            "stop_reason": "end_turn"
        });
        let resp: MessagesResponse = serde_json::from_value(fixture).unwrap();
        assert_eq!(resp.usage, ApiUsage::default());
    }

    #[test]
    fn serializes_tool_result_request_round_trip() {
        let req = MessagesRequest {
            model: "claude-haiku-4-5-20251001".to_string(),
            max_tokens: 1024,
            system: Some("Be terse.".to_string()),
            tools: vec![ToolDef {
                name: "Read".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"type": "object"}),
            }],
            messages: vec![
                ApiMessage::user_text("read a.txt"),
                ApiMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "toolu_01".to_string(),
                    name: "Read".to_string(),
                    input: json!({"path": "a.txt"}),
                }]),
                ApiMessage::user(vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_01".to_string(),
                    content: "hello world".to_string(),
                    is_error: false,
                }]),
            ],
        };

        let value = serde_json::to_value(&req).unwrap();
        // Spot-check the wire shape of the tool_result block.
        let tr = &value["messages"][2]["content"][0];
        assert_eq!(tr["type"], "tool_result");
        assert_eq!(tr["tool_use_id"], "toolu_01");
        assert_eq!(tr["content"], "hello world");
        assert_eq!(tr["is_error"], false);

        // Round-trips back to an equal request.
        let back: MessagesRequest = serde_json::from_value(value).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn system_omitted_when_none() {
        let req = MessagesRequest {
            model: "m".to_string(),
            max_tokens: 8,
            system: None,
            tools: vec![],
            messages: vec![ApiMessage::user_text("hi")],
        };
        let value = serde_json::to_value(&req).unwrap();
        assert!(value.get("system").is_none(), "system must be omitted");
    }
}
