//! Request metrics aggregation and logging for the OpenRouter embeddings client.

use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub(super) struct OpenRouterRequestMetrics {
    pub(super) request_count: usize,
    pub(super) retry_count: usize,
    pub(super) split_count: usize,
    pub(super) failed_request_count: usize,
    pub(super) total_latency: Duration,
    pub(super) min_latency: Option<Duration>,
    pub(super) max_latency: Duration,
    pub(super) total_request_inputs: usize,
    pub(super) max_request_inputs: usize,
    pub(super) total_estimated_tokens: usize,
    pub(super) max_estimated_tokens: usize,
    pub(super) response_vector_count: usize,
    pub(super) response_dim: Option<usize>,
}

pub(super) type OpenRouterMetricsHandle = Arc<Mutex<OpenRouterRequestMetrics>>;

impl OpenRouterRequestMetrics {
    pub(super) fn start_request(&mut self) -> usize {
        self.request_count += 1;
        self.request_count
    }

    pub(super) fn record_request(
        &mut self,
        latency: Duration,
        input_count: usize,
        estimated_tokens: usize,
        response_vectors: usize,
        response_dim: usize,
        failed: bool,
    ) {
        self.total_latency += latency;
        self.min_latency = Some(
            self.min_latency
                .map(|min_latency| min_latency.min(latency))
                .unwrap_or(latency),
        );
        self.max_latency = self.max_latency.max(latency);
        self.total_request_inputs += input_count;
        self.max_request_inputs = self.max_request_inputs.max(input_count);
        self.total_estimated_tokens += estimated_tokens;
        self.max_estimated_tokens = self.max_estimated_tokens.max(estimated_tokens);
        self.response_vector_count += response_vectors;
        if response_vectors > 0 {
            self.response_dim = Some(response_dim);
        }
        if failed {
            self.failed_request_count += 1;
        }
    }

    pub(super) fn record_retry(&mut self) {
        self.retry_count += 1;
    }

    pub(super) fn record_split(&mut self) {
        self.split_count += 1;
    }

    pub(super) fn avg_latency(&self) -> Duration {
        if self.request_count == 0 {
            Duration::ZERO
        } else {
            self.total_latency / self.request_count as u32
        }
    }
}

pub(super) fn log_openrouter_request_metrics(
    metrics: &OpenRouterRequestMetrics,
    elapsed: Duration,
    embedding_count: usize,
) {
    let min_latency = metrics.min_latency.unwrap_or(Duration::ZERO);
    let avg_latency = metrics.avg_latency();
    let padded_tokens_per_sec = if elapsed.is_zero() {
        0.0
    } else {
        metrics.total_estimated_tokens as f64 / elapsed.as_secs_f64()
    };

    tracing::info!(
        openrouter_request_count = metrics.request_count,
        openrouter_retry_count = metrics.retry_count,
        openrouter_split_count = metrics.split_count,
        openrouter_failed_request_count = metrics.failed_request_count,
        openrouter_total_request_latency_secs = metrics.total_latency.as_secs_f64(),
        openrouter_min_request_latency_secs = min_latency.as_secs_f64(),
        openrouter_avg_request_latency_secs = avg_latency.as_secs_f64(),
        openrouter_max_request_latency_secs = metrics.max_latency.as_secs_f64(),
        openrouter_total_request_inputs = metrics.total_request_inputs,
        openrouter_max_request_inputs = metrics.max_request_inputs,
        openrouter_total_estimated_tokens = metrics.total_estimated_tokens,
        openrouter_max_estimated_tokens = metrics.max_estimated_tokens,
        openrouter_response_vector_count = metrics.response_vector_count,
        openrouter_response_dim = metrics.response_dim.unwrap_or(0),
        openrouter_embedding_count = embedding_count,
        openrouter_elapsed_secs = elapsed.as_secs_f64(),
        openrouter_padded_tokens_per_sec = padded_tokens_per_sec,
        "OpenRouter embedding request metrics"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_metrics_tracks_counts_and_latency() {
        let mut metrics = OpenRouterRequestMetrics::default();

        assert_eq!(metrics.start_request(), 1);
        metrics.record_request(Duration::from_millis(100), 4, 40, 4, 4096, false);
        metrics.record_retry();
        metrics.record_split();
        assert_eq!(metrics.start_request(), 2);
        metrics.record_request(Duration::from_millis(300), 2, 20, 0, 4096, true);

        assert_eq!(metrics.request_count, 2);
        assert_eq!(metrics.retry_count, 1);
        assert_eq!(metrics.split_count, 1);
        assert_eq!(metrics.failed_request_count, 1);
        assert_eq!(metrics.total_request_inputs, 6);
        assert_eq!(metrics.max_request_inputs, 4);
        assert_eq!(metrics.total_estimated_tokens, 60);
        assert_eq!(metrics.max_estimated_tokens, 40);
        assert_eq!(metrics.response_vector_count, 4);
        assert_eq!(metrics.response_dim, Some(4096));
        assert_eq!(metrics.min_latency, Some(Duration::from_millis(100)));
        assert_eq!(metrics.max_latency, Duration::from_millis(300));
        assert_eq!(metrics.avg_latency(), Duration::from_millis(200));
    }
}
