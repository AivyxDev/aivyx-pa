//! Retry with exponential backoff and full jitter.
//!
//! Provides a generic `retry()` function for any async operation that may
//! fail with transient errors. Uses full jitter (random delay between 0
//! and the exponential cap) to avoid thundering herd effects.

use std::future::Future;
use std::time::Duration;

use rand::Rng;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries, just run once).
    pub max_retries: u32,
    /// Base delay for exponential backoff (e.g., 500ms).
    pub base_delay: Duration,
    /// Maximum delay cap (e.g., 30s).
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryConfig {
    /// Quick config for LLM calls — fewer retries, longer base delay.
    pub fn llm() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
        }
    }

    /// Quick config for network calls (IMAP, HTTP) — more retries, shorter base.
    pub fn network() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(15),
        }
    }
}

/// Retry an async operation with exponential backoff and full jitter.
///
/// The `should_retry` predicate inspects each error to decide if it's
/// transient (retry) or permanent (fail immediately). This prevents
/// retrying on auth failures, bad input, etc.
///
/// Returns the successful result or the last error if all retries are
/// exhausted.
pub async fn retry<F, Fut, T, E>(
    config: &RetryConfig,
    mut operation: F,
    should_retry: fn(&E) -> bool,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut last_error = None;

    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt == config.max_retries || !should_retry(&e) {
                    return Err(e);
                }

                let delay = jittered_delay(config, attempt);
                tracing::debug!(
                    "Retry {}/{}: {} — waiting {:.1}s",
                    attempt + 1,
                    config.max_retries,
                    e,
                    delay.as_secs_f64(),
                );
                tokio::time::sleep(delay).await;
                last_error = Some(e);
            }
        }
    }

    // Unreachable in practice, but satisfies the compiler
    Err(last_error.unwrap())
}

/// Calculate jittered delay: random value between 0 and min(max_delay, base * 2^attempt).
fn jittered_delay(config: &RetryConfig, attempt: u32) -> Duration {
    let exp_delay = config.base_delay.saturating_mul(1u32 << attempt.min(10));
    let cap = exp_delay.min(config.max_delay);
    let jitter_ms = rand::thread_rng().gen_range(0..=cap.as_millis() as u64);
    Duration::from_millis(jitter_ms)
}

/// Check if an AivyxError is likely transient (worth retrying).
pub fn is_transient(error: &aivyx_core::AivyxError) -> bool {
    use aivyx_core::AivyxError;
    match error {
        // Network/provider failures are usually transient
        AivyxError::LlmProvider(msg) => {
            let m = msg.to_lowercase();
            // Don't retry auth failures or quota exhaustion
            !(m.contains("auth")
                || m.contains("key")
                || m.contains("quota")
                || m.contains("rate limit"))
        }
        AivyxError::Channel(msg) => {
            let m = msg.to_lowercase();
            !(m.contains("auth") || m.contains("password") || m.contains("credentials"))
        }
        AivyxError::Http(msg) => {
            let m = msg.to_lowercase();
            // 4xx errors are not transient, connection errors are
            !(m.contains("404") || m.contains("403") || m.contains("401"))
        }
        // Most other errors are structural, not transient
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn retry_succeeds_on_first_try() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        };
        let count = AtomicU32::new(0);

        let result: Result<&str, String> = retry(
            &config,
            || {
                count.fetch_add(1, Ordering::SeqCst);
                async { Ok("success") }
            },
            |_| true,
        )
        .await;

        assert_eq!(result.unwrap(), "success");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_succeeds_after_failures() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        };
        let count = AtomicU32::new(0);

        let result: Result<&str, String> = retry(
            &config,
            || {
                let n = count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 2 {
                        Err("transient".to_string())
                    } else {
                        Ok("recovered")
                    }
                }
            },
            |_| true,
        )
        .await;

        assert_eq!(result.unwrap(), "recovered");
        assert_eq!(count.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    #[tokio::test]
    async fn retry_exhausts_attempts() {
        let config = RetryConfig {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        };
        let count = AtomicU32::new(0);

        let result: Result<&str, String> = retry(
            &config,
            || {
                count.fetch_add(1, Ordering::SeqCst);
                async { Err("always fails".to_string()) }
            },
            |_| true,
        )
        .await;

        assert_eq!(result.unwrap_err(), "always fails");
        assert_eq!(count.load(Ordering::SeqCst), 3); // initial + 2 retries
    }

    #[tokio::test]
    async fn retry_stops_on_permanent_error() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        };
        let count = AtomicU32::new(0);

        let result: Result<&str, String> = retry(
            &config,
            || {
                count.fetch_add(1, Ordering::SeqCst);
                async { Err("auth failure".to_string()) }
            },
            |e| !e.contains("auth"), // auth is permanent
        )
        .await;

        assert_eq!(result.unwrap_err(), "auth failure");
        assert_eq!(count.load(Ordering::SeqCst), 1); // no retries
    }

    #[test]
    fn jittered_delay_within_bounds() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
        };
        for attempt in 0..4 {
            let delay = jittered_delay(&config, attempt);
            assert!(delay <= config.max_delay);
        }
    }

    #[test]
    fn is_transient_classifies_errors() {
        use aivyx_core::AivyxError;

        assert!(is_transient(&AivyxError::LlmProvider(
            "connection refused".into()
        )));
        assert!(!is_transient(&AivyxError::LlmProvider(
            "invalid auth key".into()
        )));
        assert!(is_transient(&AivyxError::Channel(
            "connection reset".into()
        )));
        assert!(!is_transient(&AivyxError::Channel("bad password".into())));
        assert!(is_transient(&AivyxError::Http("timeout".into())));
        assert!(!is_transient(&AivyxError::Http("404 not found".into())));
    }
}
