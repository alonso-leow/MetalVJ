//! Error classification and the retry wrapper, ported from ZeroClaw's
//! `reliable.rs`. The rule that matters: a rate limit, a 5xx or a timeout is
//! worth another try; an auth or bad-request error is not — retrying it just
//! burns the operator's money on the same 401.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    RateLimit,
    Server,
    Timeout,
    Network,
    Auth,
    BadRequest,
    /// The provider says the request did not fit the context window.
    ContextOverflow,
    /// The model refused to comply (a safety stop, not a transport error).
    Refusal,
    Other,
}

impl ErrorKind {
    pub fn wire(self) -> &'static str {
        match self {
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::Server => "server",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Network => "network",
            ErrorKind::Auth => "auth",
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::ContextOverflow => "context_overflow",
            ErrorKind::Refusal => "refusal",
            ErrorKind::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub status: Option<u16>,
    pub message: String,
    /// The `Retry-After` the provider sent, already parsed to a duration.
    pub retry_after: Option<Duration>,
}

impl ProviderError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        ProviderError { kind, status: None, message: message.into(), retry_after: None }
    }

    pub fn status(status: u16, message: impl Into<String>) -> Self {
        ProviderError {
            kind: classify_status(status),
            status: Some(status),
            message: message.into(),
            retry_after: None,
        }
    }

    /// A retryable error is one more attempt might fix: rate limit, server,
    /// timeout, network. Everything else fails fast.
    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::RateLimit | ErrorKind::Server | ErrorKind::Timeout | ErrorKind::Network
        )
    }

    /// Context overflow is retried once after the history is pruned, not by
    /// the generic backoff loop.
    pub fn is_context_overflow(&self) -> bool {
        self.kind == ErrorKind::ContextOverflow
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(s) => write!(f, "{} (HTTP {s}): {}", self.kind.wire(), self.message),
            None => write!(f, "{}: {}", self.kind.wire(), self.message),
        }
    }
}

impl std::error::Error for ProviderError {}

/// The non-retryable set from ZeroClaw's `is_non_retryable`: 400, 401, 403,
/// 404, 422 and any other 4xx except 408/429. 408 and 429 are retryable;
/// every 5xx is retryable.
pub fn classify_status(status: u16) -> ErrorKind {
    match status {
        408 => ErrorKind::Timeout,
        429 => ErrorKind::RateLimit,
        401 | 403 => ErrorKind::Auth,
        400 | 404 | 422 => ErrorKind::BadRequest,
        500..=599 => ErrorKind::Server,
        _ if (400..500).contains(&status) => ErrorKind::BadRequest,
        _ => ErrorKind::Other,
    }
}

/// Parse a `Retry-After` header — seconds only (the HTTP-date form is rare on
/// provider APIs and not worth the date parser).
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: usize,
    pub base_ms: u64,
    pub cap_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // 3 attempts, min(30 s, 1 s·2^n) ± 20 % jitter.
        RetryPolicy { max_attempts: 3, base_ms: 1000, cap_ms: 30_000 }
    }
}

impl RetryPolicy {
    /// The wait before attempt `n` (1-based): exponential, capped, ± 20 %.
    /// `retry_after` wins when the provider sent one.
    pub fn wait_for(&self, attempt: usize, retry_after: Option<Duration>) -> Duration {
        if let Some(d) = retry_after {
            return d.min(Duration::from_millis(self.cap_ms));
        }
        let exp = self.base_ms.saturating_mul(1u64 << (attempt.saturating_sub(1)).min(10));
        let capped = exp.min(self.cap_ms);
        let jitter = jitter_factor();
        let ms = ((capped as f64) * jitter) as u64;
        Duration::from_millis(ms.clamp(1, self.cap_ms))
    }
}

/// A ± 20 % factor derived from the clock — no `rand` dependency in this
/// crate, and the exact spread does not matter, only that a fleet of retries
/// does not line up on the same millisecond.
fn jitter_factor() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    0.8 + (nanos % 4001) as f64 / 10_000.0
}

/// Run `f`, retrying retryable errors according to `policy`. `on_retry` is
/// called before each wait with `(attempt, wait, error)` so the runner can
/// emit [`crate::chat::events::Event::Retrying`]. `sleep` is injected so a
/// test drives the schedule without waiting a real minute.
pub fn with_retry<T>(
    policy: RetryPolicy,
    mut on_retry: impl FnMut(usize, Duration, &ProviderError),
    mut sleep: impl FnMut(Duration),
    mut f: impl FnMut() -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    let mut attempt = 1;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if e.retryable() && attempt < policy.max_attempts => {
                let wait = policy.wait_for(attempt, e.retry_after);
                on_retry(attempt, wait, &e);
                sleep(wait);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn statuses_classify_the_way_zeroclaw_does() {
        assert_eq!(classify_status(429), ErrorKind::RateLimit);
        assert_eq!(classify_status(408), ErrorKind::Timeout);
        assert_eq!(classify_status(401), ErrorKind::Auth);
        assert_eq!(classify_status(403), ErrorKind::Auth);
        assert_eq!(classify_status(400), ErrorKind::BadRequest);
        assert_eq!(classify_status(503), ErrorKind::Server);
        assert!(ProviderError::status(429, "x").retryable());
        assert!(ProviderError::status(500, "x").retryable());
        assert!(!ProviderError::status(401, "x").retryable());
        assert!(!ProviderError::status(400, "x").retryable());
    }

    #[test]
    fn retry_after_is_honoured() {
        let policy = RetryPolicy::default();
        let d = policy.wait_for(1, parse_retry_after("4"));
        assert_eq!(d, Duration::from_secs(4));
    }

    #[test]
    fn a_429_then_200_succeeds_after_one_retry() {
        let attempts = Cell::new(0);
        let slept = Cell::new(0u32);
        let out: Result<u32, _> = with_retry(
            RetryPolicy::default(),
            |_, _, _| {},
            |_| slept.set(slept.get() + 1),
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() < 2 {
                    Err(ProviderError::status(429, "slow down"))
                } else {
                    Ok(7)
                }
            },
        );
        assert_eq!(out.unwrap(), 7);
        assert_eq!(slept.get(), 1);
    }

    #[test]
    fn a_401_is_not_retried() {
        let attempts = Cell::new(0);
        let out: Result<u32, _> = with_retry(
            RetryPolicy::default(),
            |_, _, _| {},
            |_| {},
            || {
                attempts.set(attempts.get() + 1);
                Err(ProviderError::status(401, "bad key"))
            },
        );
        assert!(out.is_err());
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn four_500s_give_up_after_three_attempts() {
        let attempts = Cell::new(0);
        let out: Result<u32, _> = with_retry(
            RetryPolicy::default(),
            |_, _, _| {},
            |_| {},
            || {
                attempts.set(attempts.get() + 1);
                Err(ProviderError::status(500, "boom"))
            },
        );
        assert!(out.is_err());
        assert_eq!(attempts.get(), 3);
    }
}
