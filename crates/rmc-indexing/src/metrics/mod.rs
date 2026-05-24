//! Metrics collection and reporting for indexing performance
//!
//! Provides detailed performance metrics including throughput, latency percentiles,
//! phase breakdown, and memory usage tracking.

mod memory;

pub use memory::MemoryMonitor;

use std::time::{Duration, Instant};
use std::collections::HashMap;

/// Metrics collected during indexing operations
#[derive(Debug, Clone, Default)]
pub struct IndexingMetrics {
    // Throughput
    pub total_files: usize,
    pub indexed_files: usize,
    pub skipped_files: usize,
    pub unchanged_files: usize,
    pub total_chunks: usize,

    // Timing
    pub total_duration: Duration,
    pub parse_duration: Duration,
    pub embed_duration: Duration,
    pub index_duration: Duration,

    // Per-file latencies (for percentile calculation)
    pub file_latencies: Vec<Duration>,

    // Memory
    pub peak_memory_bytes: u64,

    // Errors
    pub error_count: usize,
    pub errors_by_type: HashMap<String, usize>,

    // Cache (Merkle)
    pub cache_hit_rate: f64,
}

impl IndexingMetrics {
    /// Create a new empty metrics collection
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate throughput (files per second)
    pub fn throughput(&self) -> f64 {
        if self.total_duration.as_secs_f64() == 0.0 {
            return 0.0;
        }
        self.indexed_files as f64 / self.total_duration.as_secs_f64()
    }

    /// Calculate percentile of file latencies
    pub fn percentile(&self, p: f64) -> Duration {
        if self.file_latencies.is_empty() {
            return Duration::ZERO;
        }

        let mut sorted = self.file_latencies.clone();
        sorted.sort();

        let idx = ((sorted.len() as f64) * p) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    /// Get median (p50) latency
    pub fn p50(&self) -> Duration {
        self.percentile(0.50)
    }

    /// Get 95th percentile latency
    pub fn p95(&self) -> Duration {
        self.percentile(0.95)
    }

    /// Get 99th percentile latency
    pub fn p99(&self) -> Duration {
        self.percentile(0.99)
    }

    /// Calculate error rate
    pub fn error_rate(&self) -> f64 {
        if self.total_files == 0 {
            return 0.0;
        }
        self.error_count as f64 / self.total_files as f64
    }

    /// Record an error by type
    pub fn record_error(&mut self, error_type: String) {
        self.error_count += 1;
        *self.errors_by_type.entry(error_type).or_insert(0) += 1;
    }

    /// Log a detailed summary of metrics.
    pub fn log_summary(&self) {
        let total_secs = self.total_duration.as_secs_f64();
        let parse_percent = if total_secs > 0.0 {
            self.parse_duration.as_secs_f64() / total_secs * 100.0
        } else {
            0.0
        };
        let embed_percent = if total_secs > 0.0 {
            self.embed_duration.as_secs_f64() / total_secs * 100.0
        } else {
            0.0
        };
        let index_percent = if total_secs > 0.0 {
            self.index_duration.as_secs_f64() / total_secs * 100.0
        } else {
            0.0
        };

        tracing::info!(
            indexed_files = self.indexed_files,
            total_files = self.total_files,
            skipped_files = self.skipped_files,
            unchanged_files = self.unchanged_files,
            total_chunks = self.total_chunks,
            duration_secs = total_secs,
            throughput_files_per_sec = self.throughput(),
            p50_latency_ms = self.p50().as_secs_f64() * 1000.0,
            p95_latency_ms = self.p95().as_secs_f64() * 1000.0,
            p99_latency_ms = self.p99().as_secs_f64() * 1000.0,
            parse_duration_secs = self.parse_duration.as_secs_f64(),
            parse_percent,
            embed_duration_secs = self.embed_duration.as_secs_f64(),
            embed_percent,
            index_duration_secs = self.index_duration.as_secs_f64(),
            index_percent,
            peak_memory_mb = self.peak_memory_bytes as f64 / 1_000_000.0,
            cache_hit_rate_percent = self.cache_hit_rate * 100.0,
            error_count = self.error_count,
            error_rate_percent = self.error_rate() * 100.0,
            errors_by_type = ?self.errors_by_type,
            "Indexing metrics summary"
        );
    }

    /// Log a detailed summary of metrics.
    ///
    /// Kept for callers that still use the old name. This must not write to
    /// stdout because MCP stdio reserves stdout for JSON-RPC frames.
    pub fn print_summary(&self) {
        self.log_summary();
    }
}

/// Timer for measuring phase durations
#[derive(Debug)]
pub(crate) struct PhaseTimer {
    start: Instant,
}

impl PhaseTimer {
    /// Start a new timer
    fn new() -> Self {
        Self { start: Instant::now() }
    }

    /// Get elapsed time since timer started
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Default for PhaseTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = IndexingMetrics::new();
        assert_eq!(metrics.total_files, 0);
        assert_eq!(metrics.throughput(), 0.0);
    }

    #[test]
    fn test_throughput_calculation() {
        let mut metrics = IndexingMetrics::new();
        metrics.indexed_files = 100;
        metrics.total_duration = Duration::from_secs(10);

        assert_eq!(metrics.throughput(), 10.0);
    }

    #[test]
    fn test_percentile_calculation() {
        let mut metrics = IndexingMetrics::new();
        metrics.file_latencies = vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
            Duration::from_millis(40),
            Duration::from_millis(50),
        ];

        assert_eq!(metrics.p50(), Duration::from_millis(30));
        assert_eq!(metrics.p95(), Duration::from_millis(50));
    }

    #[test]
    fn test_error_tracking() {
        let mut metrics = IndexingMetrics::new();
        metrics.total_files = 100;

        metrics.record_error("parse_error".to_string());
        metrics.record_error("parse_error".to_string());
        metrics.record_error("io_error".to_string());

        assert_eq!(metrics.error_count, 3);
        assert_eq!(metrics.error_rate(), 0.03);
        assert_eq!(metrics.errors_by_type.get("parse_error"), Some(&2));
        assert_eq!(metrics.errors_by_type.get("io_error"), Some(&1));
    }

    #[test]
    fn test_phase_timer() {
        let timer = PhaseTimer::new();
        std::thread::sleep(Duration::from_millis(10));
        assert!(timer.elapsed() >= Duration::from_millis(10));
    }
}
