# metrics — Abstract Logic

## Module: metrics (src/metrics/mod.rs)
**Purpose:** Aggregate indexing counters, latencies, and phase timings, then derive throughput/percentile/error statistics and emit a structured `tracing` summary (never stdout, which MCP reserves for JSON-RPC).

1. **Construct a zeroed metrics container** -> `IndexingMetrics::new()`
2. **Compute files-per-second throughput from totals** -> `IndexingMetrics::throughput()`
3. **Derive latency percentiles from recorded samples** -> `IndexingMetrics::percentile()`, `IndexingMetrics::p50()`, `IndexingMetrics::p95()`, `IndexingMetrics::p99()`
4. **Track and rate failures, fanning into a per-type histogram** -> `IndexingMetrics::error_rate()`, `IndexingMetrics::record_error()`
5. **Emit a single structured summary event of all collected metrics** -> `IndexingMetrics::log_summary()`, `IndexingMetrics::print_summary()`
6. **Provide a monotonic phase timer for parse/embed/index spans** -> `PhaseTimer::new()`, `PhaseTimer::elapsed()`, `<PhaseTimer as Default>::default()`

## Module: metrics::memory (src/metrics/memory.rs)
**Purpose:** Wrap `sysinfo::System` to expose process/system memory usage in bytes and percent, used to populate `IndexingMetrics::peak_memory_bytes`.

1. **Initialize and refresh a system memory snapshot** -> `MemoryMonitor::new()`, `MemoryMonitor::refresh()`, `<MemoryMonitor as Default>::default()`
2. **Report raw memory counters** -> `MemoryMonitor::used_bytes()`, `MemoryMonitor::total_bytes()`, `MemoryMonitor::available_bytes()`
3. **Report memory utilization as a percentage** -> `MemoryMonitor::usage_percent()`
