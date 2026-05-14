# metrics — Abstract Logic

## Module: metrics (src/metrics/mod.rs)
**Purpose:** Aggregate indexing counters, latencies, and phase timings, then derive throughput/percentile/error statistics for reporting.

1. **Construct a zeroed metrics container** -> `IndexingMetrics::new()`
2. **Compute throughput from totals** -> `IndexingMetrics::throughput()`
3. **Derive latency percentiles from recorded samples** -> `IndexingMetrics::percentile()`, `IndexingMetrics::p50()`, `IndexingMetrics::p95()`, `IndexingMetrics::p99()`
4. **Track and rate errors by type** -> `IndexingMetrics::error_rate()`, `IndexingMetrics::record_error()`
5. **Emit a structured summary of all collected metrics** -> `IndexingMetrics::log_summary()`, `IndexingMetrics::print_summary()`
6. **Provide a monotonic phase timer** -> `PhaseTimer::new()`, `PhaseTimer::elapsed()`, `<PhaseTimer as Default>::default()`

## Module: metrics::memory (src/metrics/memory.rs)
**Purpose:** Wrap `sysinfo::System` to expose current process/system memory usage in bytes and percent.

1. **Initialize and refresh a system memory snapshot** -> `MemoryMonitor::new()`, `MemoryMonitor::refresh()`, `<MemoryMonitor as Default>::default()`
2. **Report raw memory counters** -> `MemoryMonitor::used_bytes()`, `MemoryMonitor::total_bytes()`, `MemoryMonitor::available_bytes()`
3. **Report memory utilization as a percentage** -> `MemoryMonitor::usage_percent()`
