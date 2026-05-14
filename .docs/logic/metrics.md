# metrics — Detailed Logic

## Module: metrics (src/metrics/mod.rs)

Collects indexing performance counters, timings, and per-file latencies, then emits a single structured `tracing::info!` event so MCP stdio servers never write to stdout (reserved for JSON-RPC frames).

### `IndexingMetrics::new() -> Self`
**Call graph:** IndexingMetrics::default
**Steps:**
1. Delegate to `Self::default()` to construct an `IndexingMetrics` with zeroed counters, empty `file_latencies`/`errors_by_type`, `Duration::ZERO` for all timing fields, and `cache_hit_rate = 0.0`.

### `IndexingMetrics::throughput(&self) -> f64`
**Call graph:** Duration::as_secs_f64
**Steps:**
1. Convert `total_duration` to seconds as `f64`.
2. Return `0.0` early when the elapsed seconds equal `0.0` to avoid division by zero.
3. Otherwise return `indexed_files as f64 / total_duration_secs` as files-per-second throughput.

### `IndexingMetrics::percentile(&self, p: f64) -> Duration`
**Call graph:** Vec::is_empty, slice::clone, slice::sort, Vec::len, usize::min
**Steps:**
1. Return `Duration::ZERO` immediately if `file_latencies` is empty.
2. Clone `file_latencies` into a local `sorted` vector to avoid mutating `self`.
3. Sort `sorted` in ascending order using the natural `Duration` ordering.
4. Compute the integer index as `((len as f64) * p) as usize`.
5. Clamp the index with `min(len - 1)` and return the latency at that position.

### `IndexingMetrics::p50(&self) -> Duration`
**Call graph:** IndexingMetrics::percentile
**Steps:**
1. Call `self.percentile(0.50)` and return the result as the median latency.

### `IndexingMetrics::p95(&self) -> Duration`
**Call graph:** IndexingMetrics::percentile
**Steps:**
1. Call `self.percentile(0.95)` and return the result as the 95th percentile latency.

### `IndexingMetrics::p99(&self) -> Duration`
**Call graph:** IndexingMetrics::percentile
**Steps:**
1. Call `self.percentile(0.99)` and return the result as the 99th percentile latency.

### `IndexingMetrics::error_rate(&self) -> f64`
**Call graph:** (none)
**Steps:**
1. Return `0.0` if `total_files` is zero to avoid division by zero.
2. Otherwise return `error_count as f64 / total_files as f64` as the per-file error rate.

### `IndexingMetrics::record_error(&mut self, error_type: String)`
**Call graph:** HashMap::entry, Entry::or_insert
**Steps:**
1. Increment the global `error_count` by 1.
2. Look up `error_type` in `errors_by_type`, inserting `0` if absent.
3. Increment the count stored at that entry by 1 to track per-type frequency.

### `IndexingMetrics::log_summary(&self)`
**Call graph:** Duration::as_secs_f64, IndexingMetrics::throughput, IndexingMetrics::p50, IndexingMetrics::p95, IndexingMetrics::p99, IndexingMetrics::error_rate, tracing::info!
**Steps:**
1. Compute `total_secs` as `total_duration.as_secs_f64()`.
2. Derive `parse_percent`, `embed_percent`, and `index_percent` as each phase's share of `total_secs`, guarding against a zero total by returning `0.0`.
3. Emit one structured `tracing::info!("Indexing metrics summary", …)` event carrying: `indexed_files`, `total_files`, `skipped_files`, `unchanged_files`, `total_chunks`, `duration_secs`, `throughput_files_per_sec`, `p50_latency_ms`/`p95_latency_ms`/`p99_latency_ms` (milliseconds), `parse_duration_secs`+`parse_percent`, `embed_duration_secs`+`embed_percent`, `index_duration_secs`+`index_percent`, `peak_memory_mb` (bytes / 1_000_000), `cache_hit_rate_percent` (`cache_hit_rate * 100.0`), `error_count`, `error_rate_percent`, and `errors_by_type` formatted with `?`.
4. Routing the report through `tracing` ensures it lands on the subscriber's sink (stderr/file/JSON), never on stdout — required because the MCP stdio transport multiplexes JSON-RPC frames over stdout.

### `IndexingMetrics::print_summary(&self)`
**Call graph:** IndexingMetrics::log_summary
**Steps:**
1. Forward unconditionally to `self.log_summary()`. The function is preserved only for back-compat with callers that still use the older name; per the d43671be fix it must not touch stdout because MCP stdio reserves stdout for JSON-RPC frames.

### `PhaseTimer::new() -> Self`
**Call graph:** Instant::now
**Steps:**
1. Capture the current monotonic `Instant` as the timer's start.
2. Return a `PhaseTimer { start }` value.

### `PhaseTimer::elapsed(&self) -> Duration`
**Call graph:** Instant::elapsed
**Steps:**
1. Return `self.start.elapsed()` to report the wall duration since the timer was created.

### `impl Default for PhaseTimer :: default() -> Self`
**Call graph:** PhaseTimer::new
**Steps:**
1. Delegate to `PhaseTimer::new()` so `PhaseTimer::default()` produces a timer started at the current instant.

## Module: metrics::memory (src/metrics/memory.rs)

Thin wrapper around `sysinfo::System` exposing memory counters refreshed on demand, used to populate `IndexingMetrics::peak_memory_bytes`.

### `MemoryMonitor::new() -> Self`
**Call graph:** System::new_all, System::refresh_memory
**Steps:**
1. Construct a fully populated `sysinfo::System` with `System::new_all()`.
2. Call `system.refresh_memory()` once so the memory counters are current before the first read.
3. Return a `MemoryMonitor` wrapping that `System` instance.

### `MemoryMonitor::refresh(&mut self)`
**Call graph:** System::refresh_memory
**Steps:**
1. Invoke `self.system.refresh_memory()` to update used/total/available counters in place. Callers should poll this between phases to track peak usage.

### `MemoryMonitor::used_bytes(&self) -> u64`
**Call graph:** System::used_memory
**Steps:**
1. Return `self.system.used_memory()` as the currently used memory in bytes (reflects the most recent `refresh`).

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
2. Compute `100.0 * (used as f64 / total as f64)` and return it as a percentage of memory in use.

### `impl Default for MemoryMonitor :: default() -> Self`
**Call graph:** MemoryMonitor::new
**Steps:**
1. Delegate to `MemoryMonitor::new()` so `MemoryMonitor::default()` produces a freshly refreshed monitor.

## Collection, Export, and Reporting Model

- **Collection.** `IndexingMetrics` is a plain owned struct mutated in-process by the indexing pipeline: counters (`total_files`, `indexed_files`, `skipped_files`, `unchanged_files`, `total_chunks`) are bumped as files are visited; phase timings (`parse_duration`, `embed_duration`, `index_duration`, `total_duration`) are filled from `PhaseTimer::elapsed()`; per-file latencies are pushed into `file_latencies`; `peak_memory_bytes` is sampled from `MemoryMonitor::used_bytes()`; cache effectiveness is recorded in `cache_hit_rate`; failures funnel through `record_error` which fans into a global counter and a per-type `HashMap`.
- **Derived metrics.** `throughput`, `error_rate`, and the `percentile`/`p50`/`p95`/`p99` family are all lazy view methods — no state is cached. Percentile calculation clones `file_latencies` per call, so callers should not invoke it in hot loops.
- **Export / reporting.** The only export path is `log_summary`, which emits a single structured `tracing::info!` event with every numeric and categorical field as a key=value pair. `print_summary` is a deprecated alias retained for back-compat; since commit d43671be it forwards to `log_summary` instead of touching stdout, so the MCP stdio JSON-RPC transport stays uncorrupted. Downstream subscribers (stderr layer, file layer, JSON layer) decide where the event actually lands.
