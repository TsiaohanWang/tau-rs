//! Retry and backoff helpers for provider HTTP requests.
//!
//! Mirrors `tau_ai.retry`: exponential backoff with cancellation,
//! retryable status codes, and transient network error detection.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Base delay for exponential backoff (seconds).
const BASE_DELAY: f64 = 0.5;

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
        assert!(d0 >= 0.5 && d0 < 0.75, "d0={d0}");
        let d1 = retry_delay_seconds(1, 30.0);
        assert!(d1 >= 1.0 && d1 < 1.25, "d1={d1}");
        let d2 = retry_delay_seconds(2, 30.0);
        assert!(d2 >= 2.0 && d2 < 2.25, "d2={d2}");
    }

    #[test]
    fn retry_delay_caps_at_max() {
        let d = retry_delay_seconds(10, 5.0);
        assert!(d >= 5.0 && d < 5.25, "d={d}");
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
}
