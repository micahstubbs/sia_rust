//! Online smoke test of the bundled Gemini target profile (issue #95).
//!
//! Google's Gemini exposes an OpenAI-compatible endpoint, which `sia` reaches
//! through the `google` `client_kind` in
//! [`sia::llm::provider_mapping`]. This test loads the bundled `gemini-target`
//! profile, builds the chat transport for its provider, and does one minimal
//! `chat/completions` call, asserting a non-empty response — verifying that the
//! google → OpenAI-compatible routing actually authenticates and replies.
//!
//! It is `#[ignore]` by default and additionally returns early if
//! `GEMINI_API_KEY` is unset, so offline CI is entirely unaffected. Run it
//! explicitly with a key:
//!
//! ```bash
//! GEMINI_API_KEY=... cargo test --features llm --ignored gemini
//! ```
//!
//! The whole file is gated behind the `llm` feature (which provides the
//! optional `reqwest` blocking dependency used by the transport).

#![cfg(feature = "llm")]

use sia::llm::openai_api::{ChatMessage, ChatRequest, ChatTransport};
use sia::llm::provider_mapping::chat_transport_for;
use sia::profiles::load_target_agent_profile;

#[test]
#[ignore = "requires GEMINI_API_KEY and network access to the Gemini OpenAI-compatible endpoint"]
fn gemini_target_profile_completes_a_chat() {
    if std::env::var("GEMINI_API_KEY").is_err() {
        eprintln!("GEMINI_API_KEY not set — skipping live Gemini test");
        return;
    }

    let profile = load_target_agent_profile("gemini-target").expect("bundled gemini-target loads");
    assert_eq!(profile.provider.client_kind, "google");

    let transport = chat_transport_for(&profile.provider).expect("chat transport for gemini");
    let req = ChatRequest {
        model: profile.model.clone(),
        messages: vec![ChatMessage::user("Reply with the single word: pong")],
        tools: Vec::new(),
        tool_choice: None,
        max_tokens: Some(16),
    };

    let resp = transport
        .create(&req)
        .expect("live Gemini chat/completions call succeeds");
    let text = resp
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("");
    assert!(
        !text.trim().is_empty(),
        "expected a non-empty Gemini completion, got: {resp:?}"
    );
}
