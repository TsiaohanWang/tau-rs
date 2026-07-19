//! Retry and backoff helpers for provider HTTP requests.
//!
//! Mirrors `tau_ai.retry`: exponential backoff with cancellation,
//! retryable status codes, and transient network error detection.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Base delay for exponential backoff (seconds).
const BASE_DELAY: f64 = 0.5;

/// Base delay for rate-limit (429) backoff (seconds). Free tiers (e.g. the
/// OpenCode Zen free models) need a longer initial wait than generic 5xx
/// retries, otherwise the agent exhausts its attempts on the very first
/// cold-start 429.
const RATE_LIMIT_BASE_DELAY: f64 = 2.0;

/// Hard ceiling for rate-limit backoff (seconds).
const RATE_LIMIT_MAX_DELAY: f64 = 60.0;

/// Polling interval during backoff sleep (seconds).
const POLL_INTERVAL: f64 = 0.05;

/// Compute exponential backoff delay with jitter for a given attempt number.
///
/// `min(max_delay, 0.5 * 2^attempt) + random jitter [0, 0.25]`
pub fn retry_delay_seconds(attempt: u32, max_delay_seconds: f64) -> f64 {
    let delay = BASE_DELAY * 2.0_f64.powi(attempt as i32);
    let capped = delay.min(max_delay_seconds);
    // Add jitter: random value in [0, 0.25] seconds
    let jitter = (attempt as f64 * 0.137).fract() * 0.25;
    capped + jitter
}

/// Compute backoff for a `429` rate-limit, honoring the server's `Retry-After`
/// header when present (delta-seconds form). Falls back to a longer exponential
/// curve (`2.0 * 2^attempt`, capped) when the header is absent or unparsable.
/// Jitter in `[0, 0.5]` is added so parallel retries don't synchronize.
pub fn rate_limit_delay_seconds(
    attempt: u32,
    retry_after: Option<f64>,
    max_delay_seconds: f64,
) -> f64 {
    let ceiling = max_delay_seconds.max(RATE_LIMIT_MAX_DELAY);
    let base = match retry_after {
        Some(secs) if secs > 0.0 => secs,
        _ => {
            let delay = RATE_LIMIT_BASE_DELAY * 2.0_f64.powi(attempt as i32);
            delay.min(ceiling)
        }
    };
    let jitter = (attempt as f64 * 0.211).fract() * 0.5;
    (base + jitter).min(ceiling)
}

/// Extract a `Retry-After` delay in seconds from response headers.
///
/// Supports the delta-seconds form (`Retry-After: 30`). The HTTP-date form is
/// ignored (returns `None`) since computing absolute-time deltas adds little
/// value for the short backoffs we apply; callers fall back to the computed
/// curve in that case.
pub fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<f64> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
}

/// Sleep for `delay_seconds`, checking for cancellation every POLL_INTERVAL.
///
/// Returns `true` if the sleep completed normally, `false` if cancelled.
pub async fn wait_for_retry(delay_seconds: f64, signal: Option<&CancellationToken>) -> bool {
    let total = Duration::from_secs_f64(delay_seconds);
    let poll = Duration::from_secs_f64(POLL_INTERVAL);
    let mut elapsed = Duration::ZERO;
    while elapsed < total {
        if let Some(s) = signal {
            if s.is_cancelled() {
                return false;
            }
        }
        let remaining = total - elapsed;
        let step = remaining.min(poll);
        tokio::time::sleep(step).await;
        elapsed += step;
    }
    true
}

/// Check if an HTTP status code is retryable.
///
/// Retryable: 408 (timeout), 409 (conflict), 425 (too early),
/// 429 (rate limit), >= 500 (server error).
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500..=599)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_grows_exponentially() {
        // Base delay is 0.5, with small jitter
        let d0 = retry_delay_seconds(0, 30.0);
        assert!((0.5..0.75).contains(&d0), "d0={d0}");
        let d1 = retry_delay_seconds(1, 30.0);
        assert!((1.0..1.25).contains(&d1), "d1={d1}");
        let d2 = retry_delay_seconds(2, 30.0);
        assert!((2.0..2.25).contains(&d2), "d2={d2}");
    }

    #[test]
    fn retry_delay_caps_at_max() {
        let d = retry_delay_seconds(10, 5.0);
        assert!((5.0..5.25).contains(&d), "d={d}");
    }

    #[test]
    fn retryable_status_codes() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(404));
    }

    #[test]
    fn rate_limit_backoff_is_longer_than_generic() {
        // 429 backoff should start at ~2s, well above the generic 0.5s base.
        let generic = retry_delay_seconds(0, 30.0);
        let rl = rate_limit_delay_seconds(0, None, 30.0);
        assert!(
            rl > generic,
            "rate-limit delay {rl} should exceed generic {generic}"
        );
        assert!((2.0..2.5).contains(&rl), "rl={rl}");
    }

    #[test]
    fn rate_limit_backoff_grows_and_caps() {
        let d0 = rate_limit_delay_seconds(0, None, 30.0);
        let d1 = rate_limit_delay_seconds(1, None, 30.0);
        let d2 = rate_limit_delay_seconds(2, None, 30.0);
        assert!(d1 > d0, "d1={d1} d0={d0}");
        assert!(d2 > d1, "d2={d2} d1={d1}");
        let capped = rate_limit_delay_seconds(10, None, 30.0);
        // Floor for rate-limit backoff is RATE_LIMIT_MAX_DELAY (60s).
        assert!((60.0..60.5).contains(&capped), "capped={capped}");
    }

    #[test]
    fn rate_limit_honors_retry_after() {
        // Retry-After (15s) should win over the computed curve.
        let d = rate_limit_delay_seconds(0, Some(15.0), 30.0);
        assert!((15.0..15.5).contains(&d), "d={d}");
        // Negative / zero Retry-After falls back to the curve.
        let fb = rate_limit_delay_seconds(0, Some(0.0), 30.0);
        assert!(fb < 15.0, "fb={fb}");
    }

    #[test]
    fn retry_after_parsing() {
        use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("30"));
        assert_eq!(retry_after_seconds(&h), Some(30.0));
        let mut empty = HeaderMap::new();
        assert_eq!(retry_after_seconds(&empty), None);
        let mut bad = HeaderMap::new();
        bad.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Wed, 21 Oct 2030 07:28:00 GMT"),
        );
        assert_eq!(retry_after_seconds(&bad), None);
    }
}
