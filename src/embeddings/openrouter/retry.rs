//! HTTP retry classification and backoff helpers for OpenRouter requests.

use reqwest::StatusCode;
use std::time::Duration;

pub(super) const MAX_RETRIES: usize = 3;

pub(super) fn body_snippet(body: &str) -> String {
    let mut snippet: String = body.chars().take(500).collect();
    if body.chars().count() > 500 {
        snippet.push_str("...");
    }
    snippet
}

pub(super) fn is_payload_too_large(status: StatusCode, body: &str) -> bool {
    status == StatusCode::PAYLOAD_TOO_LARGE
        || (status == StatusCode::BAD_REQUEST
            && body.to_ascii_lowercase().contains("too large"))
}

pub(super) fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::INTERNAL_SERVER_ERROR
        || status == StatusCode::BAD_GATEWAY
        || status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::GATEWAY_TIMEOUT
        || status.as_u16() == 529
}

pub(super) fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_timeout()
}

pub(super) async fn sleep_for_retry(attempt: usize) {
    let delay_ms = 250u64.saturating_mul(1u64 << attempt.min(4));
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}
