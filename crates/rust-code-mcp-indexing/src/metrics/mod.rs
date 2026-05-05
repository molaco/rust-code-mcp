//! Metrics collection and reporting for indexing performance
//!
//! Provides detailed performance metrics including throughput, latency percentiles,
//! phase breakdown, and memory usage tracking.

pub mod memory;

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

    /// Print a detailed summary of metrics
    pub fn print_summary(&self) {
        println!("\n=== Indexing Metrics ===");
        println!("Files: {}/{} indexed ({} skipped, {} unchanged)",
            self.indexed_files,
            self.total_files,
            self.skipped_files,
            self.unchanged_files
        );
        println!("Chunks: {}", self.total_chunks);
        println!("Duration: {:.2}s", self.total_duration.as_secs_f64());
        println!("Throughput: {:.1} files/sec", self.throughput());

        if !self.file_latencies.is_empty() {
            println!("\nLatency:");
            println!("  p50: {:?}", self.p50());
            println!("  p95: {:?}", self.p95());
            println!("  p99: {:?}", self.p99());
        }

        println!("\nPhase breakdown:");
        let total_secs = self.total_duration.as_secs_f64();
        if total_secs > 0.0 {
            println!("  Parse:  {:.2}s ({:.1}%)",
                self.parse_duration.as_secs_f64(),
                self.parse_duration.as_secs_f64() / total_secs * 100.0
            );
            println!("  Embed:  {:.2}s ({:.1}%)",
                self.embed_duration.as_secs_f64(),
                self.embed_duration.as_secs_f64() / total_secs * 100.0
            );
            println!("  Index:  {:.2}s ({:.1}%)",
                self.index_duration.as_secs_f64(),
                self.index_duration.as_secs_f64() / total_secs * 100.0
            );
        }

        println!("\nMemory: {:.2} MB peak", self.peak_memory_bytes as f64 / 1_000_000.0);
        println!("Cache hit rate: {:.1}%", self.cache_hit_rate * 100.0);
        println!("Errors: {} ({:.2}%)", self.error_count, self.error_rate() * 100.0);

        if !self.errors_by_type.is_empty() {
            println!("\nErrors by type:");
            for (error_type, count) in &self.errors_by_type {
                println!("  {}: {}", error_type, count);
            }
        }
        println!("========================\n");
    }
}

/// Timer for measuring phase durations
#[derive(Debug)]
pub struct PhaseTimer {
    start: Instant,
}

impl PhaseTimer {
    /// Start a new timer
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }

    /// Get elapsed time since timer started
    pub fn elapsed(&self) -> Duration {
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
