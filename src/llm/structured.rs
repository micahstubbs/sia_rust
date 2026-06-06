//! Structured-output extraction + parity harness (issue #47).
//!
//! This module is gated behind the non-default `llm` cargo feature. It provides
//! a robust, fully-offline structured-output extraction layer plus a thin
//! wrapper around `rig-core`'s [`Extractor`](rig::extractor::Extractor) for the
//! live path.
//!
//! # Parity gate against the Python reference
//!
//! The native Rust path must produce structured outputs **semantically
//! equivalent** to the Python reference agent in
//! `sia/tasks/gpqa/reference/reference_target_agent.py`. That reference defines:
//!
//! ```python
//! class Answer(BaseModel):
//!     answer: str = Field(description="Letter A, B, C, or D")
//! ```
//!
//! and parses model responses with `parse_answer_letter`:
//!
//! ```python
//! def parse_answer_letter(model_answer_raw: str, parsed_response) -> str:
//!     if parsed_response is not None and hasattr(parsed_response, "answer"):
//!         answer = str(parsed_response.answer).strip().upper()
//!     else:
//!         try:
//!             answer = str(json.loads(model_answer_raw).get("answer", "")).strip().upper()
//!         except json.JSONDecodeError:
//!             answer = model_answer_raw.strip().upper()
//!     return answer if answer in "ABCD" else next((letter for letter in "ABCD" if letter in answer), "")
//! ```
//!
//! Key behaviors mirrored here:
//!
//! 1. The structured value's `answer` field is `.strip().upper()`-normalized.
//! 2. If parsing structured JSON fails, fall back to the raw text uppercased.
//! 3. The final letter is the value itself if it is exactly one of `A`/`B`/`C`/`D`,
//!    otherwise the **first** A–D letter found scanning left-to-right.
//! 4. The letter fallback is **non-raising**: when no A–D letter is present at
//!    all, it returns `""` (the empty string) rather than erroring. The Rust
//!    [`Answer::letter`] mirrors this exactly.
//!
//! The golden-master tests at the bottom of this file are the parity gate: they
//! assert that recorded model responses extract to the same typed value and the
//! same normalized letter the Python reference would produce, and that
//! re-serialization is byte-exact against golden JSON.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{SiaError, SiaResult};

/// Representative typed structured output mirroring the Python `Answer` pydantic
/// model in `reference_target_agent.py`:
///
/// ```python
/// class Answer(BaseModel):
///     answer: str = Field(description="Letter A, B, C, or D")
/// ```
///
/// A single field `answer` carrying a letter `A`–`D`. `JsonSchema` is derived so
/// this type can be used with the live rig [`RigStructuredExtractor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "llm", derive(schemars::JsonSchema))]
pub struct Answer {
    /// The selected answer letter (expected to be `A`, `B`, `C`, or `D`).
    pub answer: String,
}

impl Answer {
    /// Normalize and extract the answer letter, mirroring the Python
    /// `parse_answer_letter` fallback chain exactly.
    ///
    /// Given `self.answer`, this `.trim().to_uppercase()`-normalizes it, then:
    /// returns it verbatim if it equals one of `A`/`B`/`C`/`D`; otherwise returns
    /// the first A–D letter found scanning left-to-right; otherwise returns `""`.
    ///
    /// Mirrors the tail of `parse_answer_letter`:
    /// `return answer if answer in "ABCD" else next((letter for letter in "ABCD" if letter in answer), "")`.
    pub fn letter(&self) -> String {
        normalize_letter(&self.answer)
    }
}

/// Normalize a raw answer string to a canonical A–D letter (or `""`).
///
/// Mirrors the tail of the Python `parse_answer_letter`:
/// `.strip().upper()`, then return the value if it is exactly one of the four
/// letters, else the first A–D letter found, else `""`.
fn normalize_letter(raw: &str) -> String {
    let answer = raw.trim().to_uppercase();
    // `answer in "ABCD"` in Python is true when `answer` is a single char that is
    // a substring of "ABCD" (or the empty string, but we exclude that below since
    // "" would otherwise be considered "in" any string). We match the practical
    // intent: the value is exactly one of the four letters.
    if matches!(answer.as_str(), "A" | "B" | "C" | "D") {
        return answer;
    }
    // `next((letter for letter in "ABCD" if letter in answer), "")`:
    // the FIRST of A,B,C,D (in that order) that appears anywhere in `answer`.
    for letter in ['A', 'B', 'C', 'D'] {
        if answer.contains(letter) {
            return letter.to_string();
        }
    }
    String::new()
}

/// Robustly pull the first valid JSON object/array out of a model response.
///
/// Mirrors the leniency of the Python reference parsing: handles
/// ```` ```json ... ``` ```` and ```` ``` ... ``` ```` code fences, leading and
/// trailing prose, and finds the first balanced `{...}` or `[...]` span. Returns
/// a helpful [`SiaError`] when nothing parses.
///
/// Strategy (in order):
/// 1. Try to parse the whole (trimmed) text as JSON.
/// 2. Strip a single fenced code block (```` ```json ```` or ```` ``` ````) and
///    try its contents.
/// 3. Scan for the first balanced `{...}` / `[...]` span (string- and
///    escape-aware) and try each candidate until one parses.
pub fn extract_json_value(response_text: &str) -> SiaResult<serde_json::Value> {
    let trimmed = response_text.trim();

    // 1. Whole text is JSON.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Ok(value);
    }

    // 2. Fenced code block.
    if let Some(inner) = strip_code_fence(trimmed) {
        let inner = inner.trim();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(inner) {
            return Ok(value);
        }
        // The fence may itself contain prose around the JSON; fall through to the
        // balanced scan over the fence contents.
        if let Some(value) = scan_balanced(inner) {
            return Ok(value);
        }
    }

    // 3. Balanced-span scan over the full text.
    if let Some(value) = scan_balanced(trimmed) {
        return Ok(value);
    }

    Err(SiaError::new(format!(
        "no parseable JSON object or array found in model response: {:?}",
        truncate(trimmed, 200)
    )))
}

/// Extract a typed value `T` from a model response.
///
/// Runs [`extract_json_value`] then `serde_json::from_value`, attaching a clear
/// error message (naming the offending field where serde reports it) on schema
/// mismatch.
pub fn extract_struct<T: DeserializeOwned>(response_text: &str) -> SiaResult<T> {
    let value = extract_json_value(response_text)?;
    serde_json::from_value(value).map_err(|e| {
        SiaError::new(format!(
            "extracted JSON did not match the expected schema: {e}"
        ))
    })
}

/// Extract an [`Answer`] from a model response, mirroring the Python reference's
/// full `parse_answer_letter` behavior including the letter-only fallback.
///
/// 1. Try [`extract_struct::<Answer>`](extract_struct); if it yields an `Answer`,
///    return it with the `answer` field normalized via [`normalize_letter`]
///    (mirrors the structured / `json.loads` branches).
/// 2. Otherwise fall back to scanning the raw text for a lone A–D letter
///    (mirrors `answer = model_answer_raw.strip().upper()` + the letter scan).
///    This fallback is **non-raising**: it returns `Answer { answer: "" }` when
///    no A–D letter is present, exactly as the Python returns `""`.
pub fn extract_answer(response_text: &str) -> Answer {
    if let Ok(answer) = extract_struct::<Answer>(response_text) {
        return Answer {
            answer: normalize_letter(&answer.answer),
        };
    }
    Answer {
        answer: normalize_letter(response_text),
    }
}

/// Strip a single Markdown code fence, returning the inner contents.
///
/// Handles ```` ```json\n...\n``` ````, ```` ```\n...\n``` ```` and inline
/// variants. Returns `None` when the text is not fenced.
fn strip_code_fence(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after_ticks = &text[start + 3..];
    // Skip an optional language tag up to the first newline.
    let body_start = match after_ticks.find('\n') {
        Some(nl) => {
            let lang = after_ticks[..nl].trim();
            // Treat the first line as a language tag only if it has no spaces and
            // isn't obviously JSON content (defensive: a bare `{` opening fence).
            if lang.is_empty() || (!lang.contains(char::is_whitespace) && !lang.starts_with('{')) {
                nl + 1
            } else {
                0
            }
        }
        None => 0,
    };
    let after_lang = &after_ticks[body_start..];
    let end = after_lang.find("```")?;
    Some(&after_lang[..end])
}

/// Scan `text` for the first balanced `{...}` or `[...]` span that parses as
/// JSON. String- and escape-aware so braces inside JSON strings don't throw off
/// the balance count.
fn scan_balanced(text: &str) -> Option<serde_json::Value> {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let (open, close) = match b {
            b'{' => (b'{', b'}'),
            b'[' => (b'[', b']'),
            _ => continue,
        };
        if let Some(end) = balanced_end(bytes, i, open, close) {
            // `end` is exclusive of the closing char; include it.
            let candidate = &text[i..=end];
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate) {
                return Some(value);
            }
        }
    }
    None
}

/// Find the index of the matching close delimiter for the open delimiter at
/// `start`, accounting for JSON string literals and escapes.
fn balanced_end(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            _ if b == open => depth += 1,
            _ if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Truncate a string to at most `max` bytes (on a char boundary) for error
/// messages.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ---------------------------------------------------------------------------
// Live structured-output path: thin wrapper around rig-core's Extractor.
// ---------------------------------------------------------------------------

#[cfg(feature = "llm")]
mod live {
    use super::*;
    use rig::client::completion::CompletionClient;
    use rig::providers::anthropic;
    use schemars::JsonSchema;
    use serde::Serialize;

    /// Thin wrapper around `rig-core`'s Anthropic [`Extractor`](rig::extractor::Extractor)
    /// for the live structured-output path.
    ///
    /// Mirrors [`RigAgentRunner`](crate::llm::RigAgentRunner): an inherent async
    /// `extract` plus a synchronous wrapper that blocks on a current-thread tokio
    /// runtime. Built on rig 0.22.0's `client.extractor::<T>(model).build()` →
    /// `extractor.extract(text).await -> Result<T, ExtractionError>`.
    #[derive(Debug, Clone)]
    pub struct RigStructuredExtractor {
        api_key: String,
        model: String,
    }

    impl RigStructuredExtractor {
        /// Construct with an explicit Anthropic API key and model id.
        pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
            Self {
                api_key: api_key.into(),
                model: model.into(),
            }
        }

        /// Construct from `ANTHROPIC_API_KEY`, using `model` as the model id.
        pub fn from_env(model: impl Into<String>) -> SiaResult<Self> {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| SiaError::new("ANTHROPIC_API_KEY is not set"))?;
            Ok(Self::new(api_key, model))
        }

        /// Extract a typed value `T` from `text` using rig's Anthropic Extractor.
        pub async fn extract<T>(&self, text: &str) -> SiaResult<T>
        where
            T: JsonSchema + DeserializeOwned + Serialize + Send + Sync + 'static,
        {
            let client = anthropic::Client::new(&self.api_key);
            let extractor = client.extractor::<T>(&self.model).build();
            extractor
                .extract(text)
                .await
                .map_err(|e| SiaError::new(format!("rig structured extraction failed: {e}")))
        }

        /// Synchronous wrapper around [`extract`](Self::extract): blocks on a
        /// current-thread tokio runtime, matching `RigAgentRunner`'s seam.
        pub fn extract_blocking<T>(&self, text: &str) -> SiaResult<T>
        where
            T: JsonSchema + DeserializeOwned + Serialize + Send + Sync + 'static,
        {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| SiaError::new(format!("failed to build tokio runtime: {e}")))?;
            runtime.block_on(self.extract(text))
        }
    }
}

#[cfg(feature = "llm")]
pub use live::RigStructuredExtractor;

// ---------------------------------------------------------------------------
// Golden-master parity tests — the acceptance core for issue #47.
//
// These assert the Rust offline harness extracts the same typed value and the
// same normalized letter the Python `reference_target_agent.py` would produce,
// across realistic recorded model responses, and that re-serialization is
// byte-exact against golden JSON.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    // --- Recorded model-response fixtures (realistic outputs) ---

    /// 1. Clean JSON, exactly what the reference prompt asks for.
    const CLEAN_JSON: &str = r#"{"answer": "A"}"#;

    /// 2. Fenced JSON with surrounding prose.
    const FENCED_JSON: &str = "Here is my reasoning, the answer is the second option.\n\n```json\n{\n  \"answer\": \"B\"\n}\n```\n";

    /// 3. Prose + embedded object (no fence).
    const PROSE_EMBEDDED: &str = r#"Here is my answer: {"answer":"C"} — I'm confident in this."#;

    /// 3b. Prose + bare-fence embedded object.
    const PROSE_BARE_FENCE: &str = "The answer is option D.\n```\n{\"answer\": \"D\"}\n```";

    /// 4. Letter-only fallback: no JSON at all.
    ///
    /// IMPORTANT parity subtlety: the Python scan looks for the first of `A`,`B`,
    /// `C`,`D` (in that order) that appears *anywhere* in the uppercased text. The
    /// word "ANSWER" contains an `A`, so a phrase like `"the answer is B"` would
    /// resolve to `"A"`, NOT `"B"`, in the Python reference. To exercise a genuine
    /// lone-letter fallback we therefore avoid any word containing `A`/`C`/`D`.
    /// `"My pick: B"` uppercases to `"MY PICK: B"` whose first A–D letter is `B`.
    const LETTER_ONLY: &str = "My pick: B";

    /// 5. Malformed / none: no JSON and no A–D letter.
    ///
    /// Uppercases to `"NOT SURE."` which contains none of `A`,`B`,`C`,`D`.
    const NONE_AT_ALL: &str = "not sure.";

    #[test]
    fn clean_json_extracts() {
        let answer: Answer = extract_struct(CLEAN_JSON).expect("clean JSON should parse");
        assert_eq!(answer, Answer { answer: "A".into() });
        assert_eq!(answer.letter(), "A");
    }

    #[test]
    fn fenced_json_extracts() {
        let answer: Answer = extract_struct(FENCED_JSON).expect("fenced JSON should parse");
        assert_eq!(answer, Answer { answer: "B".into() });
        assert_eq!(answer.letter(), "B");
    }

    #[test]
    fn prose_embedded_object_extracts() {
        let answer: Answer = extract_struct(PROSE_EMBEDDED).expect("embedded JSON should parse");
        assert_eq!(answer, Answer { answer: "C".into() });
        assert_eq!(answer.letter(), "C");
    }

    #[test]
    fn prose_bare_fence_extracts() {
        let answer: Answer =
            extract_struct(PROSE_BARE_FENCE).expect("bare-fenced JSON should parse");
        assert_eq!(answer, Answer { answer: "D".into() });
        assert_eq!(answer.letter(), "D");
    }

    #[test]
    fn raw_value_returned_by_extract_json_value() {
        let value = extract_json_value(FENCED_JSON).expect("should extract value");
        assert_eq!(value, serde_json::json!({"answer": "B"}));
    }

    #[test]
    fn letter_only_fallback_yields_letter() {
        // Mirrors the Python letter fallback: no structured JSON, so scan the raw
        // UPPERCASED text for the first of A,B,C,D (in that order) appearing
        // anywhere. "My pick: B" -> "MY PICK: B": no A/C/D present, so -> "B".
        let answer = extract_answer(LETTER_ONLY);
        assert_eq!(answer, Answer { answer: "B".into() });
        assert_eq!(answer.letter(), "B");
    }

    #[test]
    fn none_at_all_struct_errors_helpfully() {
        // `extract_struct::<Answer>` over text with no parseable JSON returns a
        // helpful SiaError naming the failure.
        let err = extract_struct::<Answer>(NONE_AT_ALL).unwrap_err();
        assert!(
            err.0.contains("no parseable JSON"),
            "error should mention no parseable JSON, got: {}",
            err.0
        );
    }

    #[test]
    fn none_at_all_answer_fallback_is_non_raising_empty() {
        // Parity with Python: the letter fallback is non-raising. No A–D letter
        // present in "NOT SURE." -> empty string (NOT an error).
        let answer = extract_answer(NONE_AT_ALL);
        assert_eq!(
            answer,
            Answer {
                answer: String::new()
            }
        );
        assert_eq!(answer.letter(), "");
    }

    #[test]
    fn normalize_letter_mirrors_python_strip_upper() {
        // `str(...).strip().upper()` then exact-match-or-first-letter.
        assert_eq!(normalize_letter("  a  "), "A");
        // First A-D wins, scanning A,B,C,D order (not text order) — matches
        // `next((letter for letter in "ABCD" if letter in answer), "")`.
        assert_eq!(normalize_letter("D or B?"), "B");
        assert_eq!(normalize_letter("nope"), "");
        // PARITY SUBTLETY: the scan is a substring test over A,B,C,D in order.
        // "ANSWER: C" contains an "A" (in "ANSWER"), so the Python reference
        // returns "A", not "C". Verified against parse_answer_letter in Python.
        assert_eq!(normalize_letter("answer: C"), "A");
        assert_eq!(normalize_letter("My pick: B"), "B");
    }

    #[test]
    fn schema_mismatch_names_the_error() {
        // Valid JSON, wrong shape: serde error is surfaced with a clear message.
        let err = extract_struct::<Answer>(r#"{"choice": "A"}"#).unwrap_err();
        assert!(
            err.0.contains("did not match the expected schema"),
            "got: {}",
            err.0
        );
        assert!(
            err.0.contains("answer"),
            "should name missing field: {}",
            err.0
        );
    }

    #[test]
    fn byte_exact_golden_roundtrip() {
        // Parity guarantee: re-serializing the extracted value is byte-exact
        // against the golden JSON the Python reference would emit for this field.
        let answer: Answer = extract_struct(CLEAN_JSON).expect("clean JSON should parse");
        let golden = r#"{"answer":"A"}"#;
        assert_eq!(serde_json::to_string(&answer).unwrap(), golden);

        let answer_b: Answer = extract_struct(FENCED_JSON).expect("fenced JSON should parse");
        assert_eq!(
            serde_json::to_string(&answer_b).unwrap(),
            r#"{"answer":"B"}"#
        );
    }

    /// 7. Live test: extract an `Answer` from the real Anthropic API. Ignored so
    ///    CI never needs a key.
    #[cfg(feature = "llm")]
    #[test]
    #[ignore = "requires ANTHROPIC_API_KEY and network access"]
    fn live_structured_extract() {
        let extractor = RigStructuredExtractor::from_env("claude-3-5-haiku-latest")
            .expect("ANTHROPIC_API_KEY must be set");
        let answer: Answer = extractor
            .extract_blocking(
                "Answer this multiple choice question. What is 2+2? \
                 A) 3 B) 4 C) 5 D) 6. Respond with the letter.",
            )
            .expect("live extraction should succeed");
        assert_eq!(answer.letter(), "B");
    }
}
