//! Retry logic with exponential backoff for resilient indexing
//!
//! Provides retry wrappers for operations that may fail transiently

use std::time::Duration;
use tokio::time::sleep;

/// Retry an async operation with exponential backoff
///
/// # Arguments
/// * `operation` - The operation to retry (must be FnMut to allow mutation)
/// * `max_attempts` - Maximum number of attempts (including first try)
/// * `initial_delay` - Initial delay before first retry
///
/// # Returns
/// * `Ok(T)` if operation succeeds within max_attempts
/// * `Err(E)` if all attempts fail
pub async fn retry_with_backoff<F, Fut, T, E>(
    mut operation: F,
    max_attempts: u32,
    initial_delay: Duration,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut delay = initial_delay;

    for attempt in 1..=max_attempts {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_attempts => {
                tracing::warn!(
                    "Attempt {}/{} failed: {}, retrying after {:?}",
                    attempt,
                    max_attempts,
                    e,
                    delay
                );
                sleep(delay).await;
                delay *= 2; // Exponential backoff
            }
            Err(e) => {
                tracing::error!(
                    "All {} attempts failed. Last error: {}",
                    max_attempts,
                    e
                );
                return Err(e);
            }
        }
    }

    unreachable!()
}

/// Retry a sync operation with exponential backoff
///
/// # Arguments
/// * `operation` - The operation to retry
/// * `max_attempts` - Maximum number of attempts
/// * `initial_delay_ms` - Initial delay in milliseconds
pub fn retry_sync_with_backoff<F, T, E>(
    mut operation: F,
    max_attempts: u32,
    initial_delay_ms: u64,
) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
    E: std::fmt::Display,
{
    let mut delay_ms = initial_delay_ms;

    for attempt in 1..=max_attempts {
        match operation() {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_attempts => {
                tracing::warn!(
                    "Attempt {}/{} failed: {}, retrying after {}ms",
                    attempt,
                    max_attempts,
                    e,
                    delay_ms
                );
                std::thread::sleep(Duration::from_millis(delay_ms));
                delay_ms *= 2; // Exponential backoff
            }
            Err(e) => {
                tracing::error!(
                    "All {} attempts failed. Last error: {}",
                    max_attempts,
                    e
                );
                return Err(e);
            }
        }
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_retry_succeeds_first_attempt() {
        let result = retry_with_backoff(
            || async { Ok::<_, String>(42) },
            3,
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(result, Ok(42));
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_with_backoff(
            || {
                let attempts = attempts_clone.clone();
                async move {
                    let count = attempts.fetch_add(1, Ordering::SeqCst);
                    if count < 2 {
                        Err("Temporary failure")
                    } else {
                        Ok(42)
                    }
                }
            },
            5,
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(result, Ok(42));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_fails_all_attempts() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_with_backoff(
            || {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<i32, _>("Permanent failure")
                }
            },
            3,
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_retry_sync_succeeds() {
        let result = retry_sync_with_backoff(
            || Ok::<_, String>(42),
            3,
            10,
        );

        assert_eq!(result, Ok(42));
    }

    #[test]
    fn test_retry_sync_fails_then_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_sync_with_backoff(
            || {
                let count = attempts_clone.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    Err("Temporary failure")
                } else {
                    Ok(42)
                }
            },
            5,
            10,
        );

        assert_eq!(result, Ok(42));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
