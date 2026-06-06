//! Retry & resilience middleware for the synchronous LLM transports (issue #48).
//!
//! A single transient failure — a 429 rate limit, a request timeout, a 5xx, a
//! dropped connection — should not abort a long SIA run. This module provides a
//! small, configurable retry/backoff layer that wraps the existing transport
//! seams ([`MessagesTransport`] / [`ChatTransport`]) so the retry behaviour is
//! drop-in: the decorators implement the same traits as the wrapped transport.
//!
//! # Why synchronous backoff
//!
//! The transports are built on `reqwest::blocking` and their trait methods are
//! synchronous (`fn create_message(&self, ...) -> SiaResult<...>`). There is no
//! async runtime in the call path, so this layer uses synchronous
//! [`std::thread::sleep`] for backoff rather than an async helper such as
//! `tokio-retry`. This keeps the layer dependency-free and matches the blocking
//! call site exactly.
//!
//! # Pieces
//!
//! - [`RetryPolicy`] + [`backoff_delay_ms`]: a pure, deterministic exponential
//!   backoff schedule (jitter is applied separately, only in the sleeping path,
//!   so the schedule itself is unit-testable exactly).
//! - [`is_transient_error`]: a heuristic classifier over [`SiaError`]'s message.
//! - [`run_with_retry`]: the reusable retry loop, with an `on_attempt_failed`
//!   callback that lets a caller feed issue #51 trajectory capture.
//! - [`RetryMessagesTransport`] / [`RetryChatTransport`]: drop-in decorators with
//!   an optional secondary-transport fallback and an [`attempts`](RetryMessagesTransport::attempts)
//!   counter.
//!
//! The whole module is gated behind the non-default `llm` cargo feature.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::SiaError;
use crate::error::SiaResult;

use super::anthropic_api::{MessagesRequest, MessagesResponse, MessagesTransport};
use super::openai_api::{ChatRequest, ChatResponse, ChatTransport};

/// Configurable retry / exponential-backoff policy.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Number of *retries* after the first attempt. Total attempts are
    /// `max_retries + 1`.
    pub max_retries: u32,
    /// Backoff applied before the first retry, in milliseconds.
    pub initial_backoff_ms: u64,
    /// Hard cap on a single backoff delay, in milliseconds.
    pub max_backoff_ms: u64,
    /// Exponential growth factor between successive retries (e.g. `2.0`).
    pub multiplier: f64,
    /// When `true`, the slept delay is multiplied by a random factor in
    /// `[0.5, 1.0]` to spread out retries. The pure schedule from
    /// [`backoff_delay_ms`] is unaffected; jitter is applied only while sleeping.
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 4,
            initial_backoff_ms: 500,
            max_backoff_ms: 16_000,
            multiplier: 2.0,
            jitter: true,
        }
    }
}

/// Pure exponential backoff schedule: `min(initial * multiplier^attempt, max)`.
///
/// `attempt` is 0-based for the first retry (so `attempt == 0` returns
/// `initial_backoff_ms`, `attempt == 1` returns `initial * multiplier`, ...).
///
/// This is deterministic — jitter is intentionally *not* applied here so the
/// schedule can be unit-tested exactly. Jitter is layered on only in the
/// sleeping path inside [`run_with_retry`].
pub fn backoff_delay_ms(policy: &RetryPolicy, attempt: u32) -> u64 {
    let factor = policy.multiplier.powi(attempt as i32);
    let raw = policy.initial_backoff_ms as f64 * factor;
    // Clamp to the cap before casting; guard against NaN / overflow.
    let capped = raw.min(policy.max_backoff_ms as f64);
    if !capped.is_finite() || capped < 0.0 {
        return policy.max_backoff_ms;
    }
    capped as u64
}

/// Heuristic: does this error look like a *transient* failure worth retrying?
///
/// The classification is a case-insensitive substring match over the error
/// message ([`SiaError`] wraps a single `String`). It matches the signatures the
/// HTTP transports surface for retryable conditions:
///
/// - `429`, `rate limit` — rate limiting,
/// - `timeout`, `timed out` — request timeouts,
/// - `temporarily`, `unavailable`, `overloaded` — provider-side soft failures,
/// - `connection`, `connect`, `reset` — connection / socket errors,
/// - `502`, `503`, `504` — gateway / upstream 5xx.
///
/// Non-transient failures (e.g. `400 bad request`, `invalid api key`,
/// `model not found`, `401`) deliberately do *not* match. Callers that want
/// different behaviour can supply their own predicate to [`run_with_retry`].
pub fn is_transient_error(err: &SiaError) -> bool {
    let msg = err.0.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "429",
        "rate limit",
        "timeout",
        "timed out",
        "temporarily",
        "connection",
        "connect",
        "reset",
        "502",
        "503",
        "504",
        "overloaded",
        "unavailable",
    ];
    NEEDLES.iter().any(|needle| msg.contains(needle))
}

/// Tiny self-contained PRNG (SplitMix64) seeded from the wall clock.
///
/// Used only to compute a jitter factor; we deliberately avoid adding the `rand`
/// crate. The quality is irrelevant here — we just need a cheap spread.
fn jitter_factor() -> f64 {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    // SplitMix64 step.
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Map the high 53 bits to a float in [0, 1), then into [0.5, 1.0].
    let unit = (z >> 11) as f64 / (1u64 << 53) as f64;
    0.5 + 0.5 * unit
}

/// Sleep for `base_ms` (optionally jittered into `[0.5, 1.0] * base_ms`).
fn sleep_backoff(policy: &RetryPolicy, base_ms: u64) {
    if base_ms == 0 {
        return;
    }
    let ms = if policy.jitter {
        (base_ms as f64 * jitter_factor()) as u64
    } else {
        base_ms
    };
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Run `op`, retrying transient failures according to `policy`.
///
/// Behaviour:
///
/// - `op` is attempted up to `policy.max_retries + 1` times total.
/// - On `Ok`, the value is returned immediately.
/// - On `Err`, `on_attempt_failed(attempt, &err)` is called for *every* failed
///   attempt (the `attempt` index is 0-based), including the final one that is
///   ultimately returned. This makes it straightforward to mirror each failure
///   into issue #51 trajectory capture.
/// - After a failed attempt, if the error is classified transient by
///   `is_transient` *and* retries remain, the loop sleeps for
///   [`backoff_delay_ms`] (plus jitter when `policy.jitter`) and retries.
/// - If the error is non-transient, the loop returns it immediately (no further
///   attempts).
///
/// So `on_attempt_failed` is invoked exactly once per failed attempt, and the
/// returned `Err` always corresponds to the last `on_attempt_failed` call.
pub fn run_with_retry<T>(
    policy: &RetryPolicy,
    is_transient: &dyn Fn(&SiaError) -> bool,
    on_attempt_failed: &mut dyn FnMut(u32, &SiaError),
    mut op: impl FnMut() -> SiaResult<T>,
) -> SiaResult<T> {
    let mut attempt: u32 = 0;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) => {
                on_attempt_failed(attempt, &err);
                let retries_remain = attempt < policy.max_retries;
                if retries_remain && is_transient(&err) {
                    let delay = backoff_delay_ms(policy, attempt);
                    sleep_backoff(policy, delay);
                    attempt += 1;
                    continue;
                }
                return Err(err);
            }
        }
    }
}

/// Retry decorator for a [`MessagesTransport`].
///
/// Implements [`MessagesTransport`] itself, so it is a drop-in replacement for
/// the wrapped transport. `create_message` runs the inner call through
/// [`run_with_retry`]. If a `fallback` transport is configured and the primary
/// (with all its retries) still fails, the fallback is tried *once with its own
/// retry policy* (the same `policy`); its result — success or failure — is
/// returned.
///
/// The decorator does not take a per-attempt observer; instead it exposes an
/// internal attempt counter via [`attempts`](Self::attempts) (see the
/// trajectory-integration test for how to feed [`run_with_retry`]'s callback
/// into a `TrajectoryMiddleware` directly).
pub struct RetryMessagesTransport<T: MessagesTransport> {
    inner: T,
    policy: RetryPolicy,
    fallback: Option<Box<dyn MessagesTransport>>,
    attempts: AtomicU32,
}

impl<T: MessagesTransport> RetryMessagesTransport<T> {
    /// Wrap `inner` with `policy` and no fallback.
    pub fn new(inner: T, policy: RetryPolicy) -> Self {
        Self {
            inner,
            policy,
            fallback: None,
            attempts: AtomicU32::new(0),
        }
    }

    /// Wrap `inner` with `policy` and a secondary `fallback` transport tried
    /// after the primary's retries are exhausted.
    pub fn with_fallback(
        inner: T,
        policy: RetryPolicy,
        fallback: Box<dyn MessagesTransport>,
    ) -> Self {
        Self {
            inner,
            policy,
            fallback: Some(fallback),
            attempts: AtomicU32::new(0),
        }
    }

    /// Total number of *failed* attempts observed so far across all calls (i.e.
    /// the number of retries that were triggered, including failures that fell
    /// through to the fallback).
    pub fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::Relaxed)
    }
}

impl<T: MessagesTransport> MessagesTransport for RetryMessagesTransport<T> {
    fn create_message(&self, req: &MessagesRequest) -> SiaResult<MessagesResponse> {
        let counter = &self.attempts;
        let mut on_failed = move |_attempt: u32, _err: &SiaError| {
            counter.fetch_add(1, Ordering::Relaxed);
        };
        let result = run_with_retry(&self.policy, &is_transient_error, &mut on_failed, || {
            self.inner.create_message(req)
        });
        match result {
            Ok(resp) => Ok(resp),
            Err(primary_err) => match &self.fallback {
                Some(fallback) => {
                    run_with_retry(&self.policy, &is_transient_error, &mut on_failed, || {
                        fallback.create_message(req)
                    })
                }
                None => Err(primary_err),
            },
        }
    }
}

/// Retry decorator for a [`ChatTransport`]. Mirror of [`RetryMessagesTransport`]
/// for the OpenAI-compatible chat transport seam.
pub struct RetryChatTransport<T: ChatTransport> {
    inner: T,
    policy: RetryPolicy,
    fallback: Option<Box<dyn ChatTransport>>,
    attempts: AtomicU32,
}

impl<T: ChatTransport> RetryChatTransport<T> {
    /// Wrap `inner` with `policy` and no fallback.
    pub fn new(inner: T, policy: RetryPolicy) -> Self {
        Self {
            inner,
            policy,
            fallback: None,
            attempts: AtomicU32::new(0),
        }
    }

    /// Wrap `inner` with `policy` and a secondary `fallback` transport tried
    /// after the primary's retries are exhausted.
    pub fn with_fallback(inner: T, policy: RetryPolicy, fallback: Box<dyn ChatTransport>) -> Self {
        Self {
            inner,
            policy,
            fallback: Some(fallback),
            attempts: AtomicU32::new(0),
        }
    }

    /// Total number of *failed* attempts observed so far across all calls.
    pub fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::Relaxed)
    }
}

impl<T: ChatTransport> ChatTransport for RetryChatTransport<T> {
    fn create(&self, req: &ChatRequest) -> SiaResult<ChatResponse> {
        let counter = &self.attempts;
        let mut on_failed = move |_attempt: u32, _err: &SiaError| {
            counter.fetch_add(1, Ordering::Relaxed);
        };
        let result = run_with_retry(&self.policy, &is_transient_error, &mut on_failed, || {
            self.inner.create(req)
        });
        match result {
            Ok(resp) => Ok(resp),
            Err(primary_err) => match &self.fallback {
                Some(fallback) => {
                    run_with_retry(&self.policy, &is_transient_error, &mut on_failed, || {
                        fallback.create(req)
                    })
                }
                None => Err(primary_err),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::anthropic_api::{ApiUsage, MessagesResponse};
    use crate::llm::trajectory_middleware::{TrajectoryEvent, TrajectoryMiddleware};
    use std::cell::Cell;

    /// Policy with zero sleeps and no jitter, for fast deterministic tests.
    fn fast_policy(max_retries: u32) -> RetryPolicy {
        RetryPolicy {
            max_retries,
            initial_backoff_ms: 0,
            max_backoff_ms: 0,
            multiplier: 2.0,
            jitter: false,
        }
    }

    #[test]
    fn backoff_delay_ms_exact_sequence() {
        let policy = RetryPolicy {
            max_retries: 10,
            initial_backoff_ms: 100,
            max_backoff_ms: 1000,
            multiplier: 2.0,
            jitter: false,
        };
        let seq: Vec<u64> = (0..6).map(|a| backoff_delay_ms(&policy, a)).collect();
        assert_eq!(seq, vec![100, 200, 400, 800, 1000, 1000]);
    }

    #[test]
    fn is_transient_error_classification() {
        // Transient.
        for msg in [
            "HTTP 429 rate limit",
            "connection reset",
            "503 service unavailable",
            "request timed out",
            "model is temporarily overloaded",
            "502 bad gateway",
        ] {
            assert!(
                is_transient_error(&SiaError(msg.into())),
                "expected transient: {msg}"
            );
        }
        // Non-transient.
        for msg in ["400 bad request", "invalid api key", "model not found"] {
            assert!(
                !is_transient_error(&SiaError(msg.into())),
                "expected non-transient: {msg}"
            );
        }
    }

    #[test]
    fn run_with_retry_success_after_failures() {
        let policy = fast_policy(4);
        let calls = Cell::new(0u32);
        let mut failed = 0u32;
        let result: SiaResult<&str> = run_with_retry(
            &policy,
            &is_transient_error,
            &mut |_attempt, _err| failed += 1,
            || {
                let n = calls.get();
                calls.set(n + 1);
                if n < 2 {
                    Err(SiaError("429 rate limit".into()))
                } else {
                    Ok("ok")
                }
            },
        );
        assert_eq!(result, Ok("ok"));
        assert_eq!(calls.get(), 3, "op called 3 times (2 fail + 1 success)");
        assert_eq!(failed, 2, "on_attempt_failed called exactly twice");
    }

    #[test]
    fn run_with_retry_exhaustion() {
        let policy = fast_policy(2);
        let calls = Cell::new(0u32);
        let mut failed = 0u32;
        let result: SiaResult<()> = run_with_retry(
            &policy,
            &is_transient_error,
            &mut |_attempt, _err| failed += 1,
            || {
                calls.set(calls.get() + 1);
                Err(SiaError("503 unavailable".into()))
            },
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 3, "exactly max_retries + 1 = 3 attempts");
        assert_eq!(failed, 3, "every failed attempt observed");
    }

    #[test]
    fn run_with_retry_non_transient_no_retry() {
        let policy = fast_policy(4);
        let calls = Cell::new(0u32);
        let mut failed = 0u32;
        let result: SiaResult<()> = run_with_retry(
            &policy,
            &is_transient_error,
            &mut |_attempt, _err| failed += 1,
            || {
                calls.set(calls.get() + 1);
                Err(SiaError("invalid api key".into()))
            },
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 1, "non-transient: no retries");
        assert_eq!(failed, 1, "single failure observed");
    }

    /// A flaky [`MessagesTransport`] that fails `fail_n` times (transiently) then
    /// succeeds.
    struct FlakyMessages {
        fail_n: u32,
        calls: std::cell::Cell<u32>,
    }

    fn ok_messages_response() -> MessagesResponse {
        MessagesResponse {
            id: "msg_1".into(),
            role: "assistant".into(),
            content: Vec::new(),
            stop_reason: Some("end_turn".into()),
            usage: ApiUsage::default(),
        }
    }

    impl MessagesTransport for FlakyMessages {
        fn create_message(&self, _req: &MessagesRequest) -> SiaResult<MessagesResponse> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            if n < self.fail_n {
                Err(SiaError("429 rate limit, please retry".into()))
            } else {
                Ok(ok_messages_response())
            }
        }
    }

    /// A [`MessagesTransport`] that always fails transiently.
    struct AlwaysFailMessages;
    impl MessagesTransport for AlwaysFailMessages {
        fn create_message(&self, _req: &MessagesRequest) -> SiaResult<MessagesResponse> {
            Err(SiaError("503 service unavailable".into()))
        }
    }

    /// A [`MessagesTransport`] that always succeeds.
    struct AlwaysOkMessages;
    impl MessagesTransport for AlwaysOkMessages {
        fn create_message(&self, _req: &MessagesRequest) -> SiaResult<MessagesResponse> {
            Ok(ok_messages_response())
        }
    }

    fn dummy_request() -> MessagesRequest {
        MessagesRequest {
            model: "m".into(),
            max_tokens: 16,
            messages: Vec::new(),
            tools: Vec::new(),
            system: None,
        }
    }

    #[test]
    fn retry_messages_transport_recovers_from_flaky() {
        let flaky = FlakyMessages {
            fail_n: 2,
            calls: std::cell::Cell::new(0),
        };
        let transport = RetryMessagesTransport::new(flaky, fast_policy(4));
        let resp = transport.create_message(&dummy_request());
        assert!(resp.is_ok(), "should recover after 2 transient failures");
        assert_eq!(
            transport.attempts(),
            2,
            "two retries reflected in attempts()"
        );
    }

    #[test]
    fn retry_messages_transport_falls_back() {
        let transport = RetryMessagesTransport::with_fallback(
            AlwaysFailMessages,
            fast_policy(2),
            Box::new(AlwaysOkMessages),
        );
        let resp = transport.create_message(&dummy_request());
        assert!(resp.is_ok(), "fallback should produce success");
        // Primary failed 3 times (max_retries + 1); fallback succeeded first try.
        assert_eq!(transport.attempts(), 3);
    }

    #[test]
    fn trajectory_integration_records_each_failure() {
        // Show how run_with_retry's callback feeds #51 trajectory capture: each
        // failed attempt records a TrajectoryEvent::Error.
        let policy = fast_policy(2);
        let mut mw = TrajectoryMiddleware::new();
        let calls = Cell::new(0u32);
        let result: SiaResult<()> = run_with_retry(
            &policy,
            &is_transient_error,
            &mut |attempt, err| {
                mw.record(TrajectoryEvent::Error {
                    message: format!("attempt {attempt} failed: {err}"),
                });
            },
            || {
                calls.set(calls.get() + 1);
                Err(SiaError("504 gateway timeout".into()))
            },
        );
        assert!(result.is_err());
        // 3 failed attempts -> 3 recorded errors.
        assert_eq!(calls.get(), 3);
        assert_eq!(mw.metrics().num_errors, 3);
    }
}
