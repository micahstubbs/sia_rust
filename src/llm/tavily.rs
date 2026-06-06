//! Tavily web-search client primitive (issue #106).
//!
//! [Tavily](https://docs.tavily.com/) is a search API built for LLM agents: it
//! takes a natural-language query and returns clean, ranked, LLM-native results
//! (and an optional synthesized `answer`). The free tier grants 1,000 credits
//! per month for agent-ready web search.
//!
//! # Scope: this is the *primitive*, not the wiring
//!
//! This module provides only the low-level client a future **Feedback-Agent
//! web-search tool** / legal-benchmark task will call. It is intentionally
//! **not** wired into the agent tool loop yet: doing that needs a live key to
//! validate end-to-end and is tracked as a follow-up. What lives here is the
//! self-contained, offline-testable building block:
//!
//! - [`SearchRequest`] / [`SearchResponse`] / [`SearchResult`] — serde models
//!   matching the documented `POST https://api.tavily.com/search` wire shape.
//! - [`SearchTransport`] — an injectable seam so the client is testable with
//!   zero network ([`HttpSearchTransport`] for real calls, [`MockSearchTransport`]
//!   for scripted responses in tests).
//! - [`TavilyClient`] — the ergonomic entry point, with [`TavilyClient::from_env`]
//!   reading the API key from the `TAVILY_API_KEY` environment variable.
//!
//! # Authentication
//!
//! The current Tavily API authenticates with a **`Authorization: Bearer <key>`**
//! header (keys are prefixed `tvly-`); the key is *not* sent in the JSON body.
//! See <https://docs.tavily.com/documentation/api-reference/endpoint/search>.
//!
//! The whole module is gated behind the non-default `llm` cargo feature, since
//! [`HttpSearchTransport`] uses the optional `reqwest` dependency.
//!
//! # Example (offline, via a mock transport)
//!
//! ```
//! # use sia::llm::tavily::{TavilyClient, MockSearchTransport, SearchResponse, SearchResult};
//! let scripted = SearchResponse {
//!     query: "rust ownership".to_string(),
//!     answer: Some("Ownership is Rust's memory-management model.".to_string()),
//!     results: vec![SearchResult {
//!         title: "Ownership".to_string(),
//!         url: "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html".to_string(),
//!         content: "Ownership is a set of rules...".to_string(),
//!         score: 0.98,
//!     }],
//! };
//! let client = TavilyClient::new(MockSearchTransport::new(scripted));
//! let lines = client.search_text("rust ownership").unwrap();
//! assert_eq!(lines.len(), 1);
//! ```

use serde::{Deserialize, Serialize};

use crate::error::{SiaError, SiaResult};

/// Default Tavily API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.tavily.com";

/// Environment variable read by [`TavilyClient::from_env`].
pub const API_KEY_ENV: &str = "TAVILY_API_KEY";

/// Default number of results requested per search.
pub const DEFAULT_MAX_RESULTS: usize = 5;

/// A `POST {base_url}/search` request body.
///
/// Only the fields this primitive sets are modeled; optional fields are omitted
/// from the wire when unset so Tavily applies its own defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchRequest {
    /// The natural-language search query (required).
    pub query: String,
    /// Maximum number of results to return (Tavily allows 0–20; default 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    /// Search depth: `"basic"` or `"advanced"` (also `"fast"` / `"ultra-fast"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_depth: Option<String>,
    /// Whether Tavily should synthesize an `answer` from the results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_answer: Option<bool>,
}

impl SearchRequest {
    /// Build a request for `query` with all optional fields unset.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: None,
            search_depth: None,
            include_answer: None,
        }
    }
}

/// A `POST {base_url}/search` response body (only the fields this primitive reads).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResponse {
    /// The query Tavily actually executed.
    #[serde(default)]
    pub query: String,
    /// LLM-synthesized answer, present only when `include_answer` was requested.
    #[serde(default)]
    pub answer: Option<String>,
    /// Search results, ranked by relevance.
    #[serde(default)]
    pub results: Vec<SearchResult>,
}

/// A single Tavily search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Result heading.
    #[serde(default)]
    pub title: String,
    /// Source URL.
    #[serde(default)]
    pub url: String,
    /// Cleaned, relevant content snippet from the source.
    #[serde(default)]
    pub content: String,
    /// Relevance score assigned by Tavily.
    #[serde(default)]
    pub score: f64,
}

/// Injectable transport over the Tavily search endpoint.
///
/// [`TavilyClient`] depends only on this trait, so tests can supply scripted
/// responses ([`MockSearchTransport`]) with no network access.
pub trait SearchTransport {
    /// Send one [`SearchRequest`] and return the parsed [`SearchResponse`].
    fn search(&self, req: &SearchRequest) -> SiaResult<SearchResponse>;
}

/// Real transport: POSTs to `{base_url}/search` via `reqwest::blocking`, using
/// `Authorization: Bearer <api_key>` auth (the key is never put in the body).
#[derive(Debug, Clone)]
pub struct HttpSearchTransport {
    api_key: String,
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpSearchTransport {
    /// Construct a transport against the default Tavily base URL.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    /// Construct a transport against an explicit base URL (e.g. a proxy/gateway).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            client: reqwest::blocking::Client::new(),
        }
    }
}

impl SearchTransport for HttpSearchTransport {
    fn search(&self, req: &SearchRequest) -> SiaResult<SearchResponse> {
        let url = format!("{}/search", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(req)
            .send()
            .map_err(|e| SiaError::new(format!("tavily search request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(SiaError::new(format!(
                "tavily search API returned {status}: {body}"
            )));
        }

        resp.json::<SearchResponse>()
            .map_err(|e| SiaError::new(format!("failed to decode tavily search response: {e}")))
    }
}

/// A transport returning a scripted [`SearchResponse`], for offline tests.
///
/// Made `pub` (not just test-only) so downstream/integration tests of the future
/// Feedback-Agent web-search tool can drive [`TavilyClient`] without a network.
#[derive(Debug, Clone)]
pub struct MockSearchTransport {
    response: SearchResponse,
}

impl MockSearchTransport {
    /// Build a mock that always returns `response`.
    pub fn new(response: SearchResponse) -> Self {
        Self { response }
    }
}

impl SearchTransport for MockSearchTransport {
    fn search(&self, _req: &SearchRequest) -> SiaResult<SearchResponse> {
        Ok(self.response.clone())
    }
}

/// Ergonomic Tavily web-search client over an injectable [`SearchTransport`].
#[derive(Debug, Clone)]
pub struct TavilyClient<T: SearchTransport> {
    transport: T,
    default_max_results: usize,
    search_depth: String,
    include_answer: bool,
}

impl<T: SearchTransport> TavilyClient<T> {
    /// Build a client over `transport` with default search settings.
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            default_max_results: DEFAULT_MAX_RESULTS,
            search_depth: "basic".to_string(),
            include_answer: true,
        }
    }

    /// Override the default `max_results` used by [`TavilyClient::search`].
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        self.default_max_results = max_results;
        self
    }

    /// Override the `search_depth` (`"basic"` or `"advanced"`).
    pub fn with_search_depth(mut self, search_depth: impl Into<String>) -> Self {
        self.search_depth = search_depth.into();
        self
    }

    /// Toggle whether Tavily synthesizes an `answer` from the results.
    pub fn with_include_answer(mut self, include_answer: bool) -> Self {
        self.include_answer = include_answer;
        self
    }

    /// Run a search for `query`, applying the client's default settings.
    pub fn search(&self, query: &str) -> SiaResult<SearchResponse> {
        let req = SearchRequest {
            query: query.to_string(),
            max_results: Some(self.default_max_results),
            search_depth: Some(self.search_depth.clone()),
            include_answer: Some(self.include_answer),
        };
        self.transport.search(&req)
    }

    /// Run a search and format each result as a single line for an LLM tool
    /// result: `"<title> — <url>\n<content>"`. Returns one string per result
    /// (empty vec when there are no results).
    pub fn search_text(&self, query: &str) -> SiaResult<Vec<String>> {
        let resp = self.search(query)?;
        Ok(resp
            .results
            .iter()
            .map(|r| format!("{} — {}\n{}", r.title, r.url, r.content))
            .collect())
    }
}

impl TavilyClient<HttpSearchTransport> {
    /// Build an HTTP-backed client, reading the key from `TAVILY_API_KEY`.
    ///
    /// Returns a [`SiaError`] naming the variable if it is unset (mirroring the
    /// crate's user-facing error style).
    pub fn from_env() -> SiaResult<Self> {
        let api_key = std::env::var(API_KEY_ENV).map_err(|_| {
            SiaError::new(format!(
                "{API_KEY_ENV} is not set (needed for Tavily web search; \
                 get a free key at https://app.tavily.com and `export {API_KEY_ENV}=…`)"
            ))
        })?;
        Ok(Self::new(HttpSearchTransport::new(api_key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn recorded_response() -> serde_json::Value {
        json!({
            "query": "what is rust ownership",
            "answer": "Ownership is Rust's system for managing memory safely.",
            "results": [
                {
                    "title": "Understanding Ownership",
                    "url": "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html",
                    "content": "Ownership is a set of rules that govern how Rust manages memory.",
                    "score": 0.97
                },
                {
                    "title": "Ownership rules",
                    "url": "https://example.com/ownership",
                    "content": "Each value has a single owner.",
                    "score": 0.81
                }
            ],
            "response_time": 1.23
        })
    }

    #[test]
    fn deserializes_recorded_response_with_answer() {
        let resp: SearchResponse = serde_json::from_value(recorded_response()).unwrap();
        assert_eq!(resp.query, "what is rust ownership");
        assert_eq!(
            resp.answer.as_deref(),
            Some("Ownership is Rust's system for managing memory safely.")
        );
        assert_eq!(resp.results.len(), 2);
        assert_eq!(resp.results[0].title, "Understanding Ownership");
        assert_eq!(resp.results[0].score, 0.97);
    }

    #[test]
    fn deserializes_response_without_answer() {
        let fixture = json!({
            "query": "q",
            "results": [
                { "title": "T", "url": "https://e.com", "content": "c", "score": 0.5 }
            ]
        });
        let resp: SearchResponse = serde_json::from_value(fixture).unwrap();
        assert_eq!(resp.answer, None);
        assert_eq!(resp.results.len(), 1);
    }

    #[test]
    fn deserializes_result_with_missing_optional_fields() {
        // A result missing `content` and `score` should fall back to defaults.
        let fixture = json!({
            "results": [ { "title": "T", "url": "https://e.com" } ]
        });
        let resp: SearchResponse = serde_json::from_value(fixture).unwrap();
        assert_eq!(resp.query, "");
        assert_eq!(resp.answer, None);
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].content, "");
        assert_eq!(resp.results[0].score, 0.0);
    }

    #[test]
    fn response_round_trips_through_serde() {
        let resp: SearchResponse = serde_json::from_value(recorded_response()).unwrap();
        let value = serde_json::to_value(&resp).unwrap();
        let back: SearchResponse = serde_json::from_value(value).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn request_omits_unset_optionals() {
        let req = SearchRequest::new("hello");
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["query"], "hello");
        assert!(value.get("max_results").is_none());
        assert!(value.get("search_depth").is_none());
        assert!(value.get("include_answer").is_none());
    }

    #[test]
    fn request_serializes_set_optionals() {
        let req = SearchRequest {
            query: "hello".to_string(),
            max_results: Some(3),
            search_depth: Some("advanced".to_string()),
            include_answer: Some(true),
        };
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["max_results"], 3);
        assert_eq!(value["search_depth"], "advanced");
        assert_eq!(value["include_answer"], true);
        // The API key is auth-header only; it must never appear in the body.
        assert!(value.get("api_key").is_none());
    }

    #[test]
    fn client_search_returns_scripted_results() {
        let scripted: SearchResponse = serde_json::from_value(recorded_response()).unwrap();
        let client = TavilyClient::new(MockSearchTransport::new(scripted));
        let resp = client.search("what is rust ownership").unwrap();
        assert_eq!(resp.results.len(), 2);
        assert_eq!(
            resp.answer.as_deref(),
            Some("Ownership is Rust's system for managing memory safely.")
        );
    }

    #[test]
    fn client_search_text_formats_results() {
        let scripted: SearchResponse = serde_json::from_value(recorded_response()).unwrap();
        let client = TavilyClient::new(MockSearchTransport::new(scripted));
        let lines = client.search_text("q").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "Understanding Ownership — \
             https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html\n\
             Ownership is a set of rules that govern how Rust manages memory."
        );
        assert!(lines[1].starts_with("Ownership rules — https://example.com/ownership"));
    }

    #[test]
    fn client_handles_empty_results() {
        let empty = SearchResponse {
            query: "nothing".to_string(),
            answer: None,
            results: Vec::new(),
        };
        let client = TavilyClient::new(MockSearchTransport::new(empty));
        let resp = client.search("nothing").unwrap();
        assert!(resp.results.is_empty());
        assert!(client.search_text("nothing").unwrap().is_empty());
    }

    #[test]
    fn from_env_errors_when_missing() {
        // Snapshot + clear the var so the test is deterministic regardless of host env.
        let saved = std::env::var(API_KEY_ENV).ok();
        std::env::remove_var(API_KEY_ENV);

        let result = TavilyClient::from_env();

        if let Some(v) = saved {
            std::env::set_var(API_KEY_ENV, v);
        }

        let err = result.expect_err("from_env should error when the key is unset");
        // The error must name the variable so the message is actionable.
        assert!(err.to_string().contains(API_KEY_ENV));
    }

    /// Live test against the real Tavily API. Ignored so CI never needs a key.
    #[test]
    #[ignore = "requires TAVILY_API_KEY and network access"]
    fn live_search() {
        // Return early (rather than panic) if the key is unset, per the issue.
        let Ok(api_key) = std::env::var(API_KEY_ENV) else {
            eprintln!("{API_KEY_ENV} unset; skipping live Tavily search");
            return;
        };
        let client = TavilyClient::new(HttpSearchTransport::new(api_key));
        let resp = client
            .search("what is the capital of France")
            .expect("live Tavily search should succeed");
        assert!(
            !resp.results.is_empty(),
            "live search should return at least one result"
        );
    }
}
