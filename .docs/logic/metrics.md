# metrics — Detailed Logic

## Module: metrics (src/metrics/mod.rs)

### `IndexingMetrics::new() -> Self`
**Call graph:** IndexingMetrics::default
**Steps:**
1. Delegate to `Self::default()` to construct an `IndexingMetrics` with zeroed counters, empty collections, and `Duration::ZERO` for all timing fields.

### `IndexingMetrics::throughput(&self) -> f64`
**Call graph:** Duration::as_secs_f64
**Steps:**
1. Convert `total_duration` to seconds as `f64`.
2. Return `0.0` early if the elapsed seconds equal `0.0` to avoid division by zero.
3. Otherwise return `indexed_files / total_duration_secs` as files-per-second throughput.

### `IndexingMetrics::percentile(&self, p: f64) -> Duration`
**Call graph:** Vec::is_empty, Vec::clone, slice::sort, Vec::len, usize::min
**Steps:**
1. Return `Duration::ZERO` immediately if `file_latencies` is empty.
2. Clone `file_latencies` into a local `sorted` vector to avoid mutating self.
3. Sort `sorted` in ascending order using the natural `Duration` ordering.
4. Compute the integer index as `(len * p) as usize`.
5. Clamp the index with `min(len - 1)` and return the latency at that position.

### `IndexingMetrics::p50(&self) -> Duration`
**Call graph:** IndexingMetrics::percentile
**Steps:**
1. Call `self.percentile(0.50)` and return its result as the median latency.

### `IndexingMetrics::p95(&self) -> Duration`
**Call graph:** IndexingMetrics::percentile
**Steps:**
1. Call `self.percentile(0.95)` and return its result as the 95th percentile latency.

### `IndexingMetrics::p99(&self) -> Duration`
**Call graph:** IndexingMetrics::percentile
**Steps:**
1. Call `self.percentile(0.99)` and return its result as the 99th percentile latency.

### `IndexingMetrics::error_rate(&self) -> f64`
**Call graph:** (none)
**Steps:**
1. Return `0.0` if `total_files` is zero to avoid division by zero.
2. Otherwise return `error_count / total_files` cast to `f64` as the error rate.

### `IndexingMetrics::record_error(&mut self, error_type: String)`
**Call graph:** HashMap::entry, Entry::or_insert
**Steps:**
1. Increment the global `error_count` by 1.
2. Look up `error_type` in `errors_by_type`, inserting `0` if absent.
3. Increment the count stored at that entry by 1 to track per-type frequency.

### `IndexingMetrics::log_summary(&self)`
**Call graph:** Duration::as_secs_f64, IndexingMetrics::throughput, IndexingMetrics::p50, IndexingMetrics::p95, IndexingMetrics::p99, IndexingMetrics::error_rate, tracing::info!
**Steps:**
1. Compute `total_secs` as the total duration in seconds (`f64`).
2. Compute `parse_percent`, `embed_percent`, and `index_percent` as the share of total time spent in each phase, guarding against zero total.
3. Emit a single `tracing::info!` event carrying file counts, throughput, p50/p95/p99 latencies in milliseconds, per-phase durations and percentages, peak memory in MB, cache hit rate percent, error count, error rate percent, and the `errors_by_type` map.

### `IndexingMetrics::print_summary(&self)`
**Call graph:** IndexingMetrics::log_summary
**Steps:**
1. Forward to `self.log_summary()` so legacy callers do not write to stdout (preserved for MCP stdio compatibility where stdout is reserved for JSON-RPC frames).

### `PhaseTimer::new() -> Self`
**Call graph:** Instant::now
**Steps:**
1. Capture the current monotonic `Instant` as the timer's start.
2. Return a `PhaseTimer { start }` value.

### `PhaseTimer::elapsed(&self) -> Duration`
**Call graph:** Instant::elapsed
**Steps:**
1. Return `self.start.elapsed()` to report the duration since the timer was created.

### `impl Default for PhaseTimer :: default() -> Self`
**Call graph:** PhaseTimer::new
**Steps:**
1. Delegate to `PhaseTimer::new()` so `PhaseTimer::default()` produces a timer started at the current instant.

## Module: metrics::memory (src/metrics/memory.rs)

### `MemoryMonitor::new() -> Self`
**Call graph:** System::new_all, System::refresh_memory
**Steps:**
1. Create a fully populated `sysinfo::System` with `System::new_all()`.
2. Call `refresh_memory()` once to ensure memory counters are current.
3. Return a `MemoryMonitor` wrapping that `System` instance.

### `MemoryMonitor::refresh(&mut self)`
**Call graph:** System::refresh_memory
**Steps:**
1. Invoke `self.system.refresh_memory()` to update used/total/available counters in place.

### `MemoryMonitor::used_bytes(&self) -> u64`
**Call graph:** System::used_memory
**Steps:**
1. Return `self.system.used_memory()` as the currently used memory in bytes.

### `MemoryMonitor::total_bytes(&self) -> u64`
**Call graph:** System::total_memory
**Steps:**
1. Return `self.system.total_memory()` as the total physical memory in bytes.

### `MemoryMonitor::available_bytes(&self) -> u64`
**Call graph:** System::available_memory
**Steps:**
1. Return `self.system.available_memory()` as the memory available for allocation in bytes.

### `MemoryMonitor::usage_percent(&self) -> f64`
**Call graph:** MemoryMonitor::used_bytes, MemoryMonitor::total_bytes
**Steps:**
1. Read used and total bytes via the corresponding accessor methods.
2. Compute `100.0 * (used / total)` cast to `f64` and return it as a percentage of memory in use.

### `impl Default for MemoryMonitor :: default() -> Self`
**Call graph:** MemoryMonitor::new
**Steps:**
1. Delegate to `MemoryMonitor::new()` so `MemoryMonitor::default()` produces a freshly refreshed monitor.
