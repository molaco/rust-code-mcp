# monitoring — Detailed Logic

## Module: monitoring (mod.rs)

The root module file simply declares the two submodules `health` and `backup` as public. It contains no functions, types, or trait impls of its own.

## Module: backup

### `BackupManager::new(backup_dir: PathBuf, retention_count: usize) -> Result<Self>`
**Call graph:** std::fs::create_dir_all -> anyhow::Context::context -> PathBuf::display
**Steps:**
1. Calls `std::fs::create_dir_all` to ensure the backup directory exists, creating intermediate directories as needed.
2. Wraps any error with a context message identifying the failed directory path.
3. Returns a new `BackupManager` initialized with the provided directory and retention count.

### `BackupManager::create_backup(&self, merkle: &FileSystemMerkle) -> Result<PathBuf>`
**Call graph:** std::time::SystemTime::now -> SystemTime::duration_since -> Duration::as_secs -> PathBuf::join -> FileSystemMerkle::version -> FileSystemMerkle::save_snapshot -> Self::rotate_backups -> tracing::info
**Steps:**
1. Captures the current Unix timestamp in seconds via `SystemTime::now().duration_since(UNIX_EPOCH)`.
2. Builds the backup file path as `merkle_v{version}.{timestamp}.snapshot` under the backup directory.
3. Calls `merkle.save_snapshot(&backup_path)` to serialize the Merkle tree to disk, attaching context on failure.
4. Invokes `self.rotate_backups()` to enforce the retention policy by deleting older snapshots.
5. Logs the created backup path at info level and returns the path.

### `BackupManager::restore_latest(&self) -> Result<Option<FileSystemMerkle>>`
**Call graph:** Self::list_backups -> Vec::is_empty -> tracing::info -> Vec::sort_by_key -> DirEntry::metadata -> Metadata::modified -> Vec::reverse -> Vec::first -> DirEntry::path -> FileSystemMerkle::load_snapshot
**Steps:**
1. Calls `self.list_backups()` to enumerate snapshot files in the backup directory.
2. Returns `Ok(None)` immediately if no backups exist, logging the empty state.
3. Sorts the backup entries by modified-time, then reverses to put the newest first.
4. Picks the first (latest) entry's path, logging the chosen file.
5. Calls `FileSystemMerkle::load_snapshot` on that path and wraps any error with a context message.

### `BackupManager::list_backups(&self) -> Result<Vec<std::fs::DirEntry>>`
**Call graph:** std::fs::read_dir -> Iterator::filter_map -> Result::ok -> Iterator::filter -> DirEntry::path -> Path::extension -> OsStr::to_str -> Iterator::collect
**Steps:**
1. Calls `std::fs::read_dir` on the backup directory, attaching context on failure.
2. Filters out unreadable entries via `filter_map(|e| e.ok())`.
3. Retains only entries whose file extension equals `"snapshot"`.
4. Collects the surviving `DirEntry` values into a `Vec` and returns it.

### `BackupManager::rotate_backups(&self) -> Result<()>` (private)
**Call graph:** Self::list_backups -> Vec::len -> Vec::sort_by_key -> DirEntry::metadata -> Metadata::modified -> Iterator::take -> DirEntry::path -> std::fs::remove_file -> tracing::info
**Steps:**
1. Lists all current backups via `self.list_backups()`.
2. Returns early if the backup count is at or below the retention threshold.
3. Sorts entries by modified-time ascending so oldest snapshots come first.
4. Computes how many to remove (`len - retention_count`) and iterates the oldest that many entries.
5. Calls `std::fs::remove_file` on each, logs the deletion, and propagates any IO error with context.

### `BackupManager::backup_dir(&self) -> &Path`
**Call graph:** (none — direct field reference)
**Steps:**
1. Returns a borrowed reference to the stored `backup_dir` path.

### `BackupManager::retention_count(&self) -> usize`
**Call graph:** (none — direct field copy)
**Steps:**
1. Returns the stored retention count by value.

## Module: health

### `ComponentHealth::healthy(message: impl Into<String>, latency_ms: Option<u64>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Converts the supplied message into a `String` via `Into::into`.
2. Constructs a `ComponentHealth` with `Status::Healthy`, the message, and the provided latency value.

### `ComponentHealth::degraded(message: impl Into<String>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Converts the supplied message into a `String`.
2. Returns a `ComponentHealth` with `Status::Degraded`, the message, and `latency_ms = None`.

### `ComponentHealth::unhealthy(message: impl Into<String>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Converts the supplied message into a `String`.
2. Returns a `ComponentHealth` with `Status::Unhealthy`, the message, and `latency_ms = None`.

### `HealthMonitor::new(bm25: Option<Arc<Bm25Search>>, vector_store: Option<Arc<VectorStore>>, merkle_path: PathBuf) -> Self`
**Call graph:** (none — pure struct constructor)
**Steps:**
1. Returns a new `HealthMonitor` with the optional BM25 and vector-store handles plus the Merkle snapshot path.

### `HealthMonitor::check_health(&self) -> HealthStatus` (async)
**Call graph:** tokio::join! -> Self::check_bm25 -> Self::check_vector -> Self::check_merkle -> Self::calculate_overall_status
**Steps:**
1. Uses `tokio::join!` to run `check_bm25`, `check_vector`, and `check_merkle` concurrently.
2. Awaits all three results into a `(bm25_health, vector_health, merkle_health)` tuple.
3. Calls `calculate_overall_status` to derive a single `Status` from the three component results.
4. Returns a `HealthStatus` aggregating overall and per-component health.

### `HealthMonitor::check_bm25(&self) -> ComponentHealth` (async, private)
**Call graph:** ComponentHealth::degraded -> Instant::now -> Bm25Search::search -> Instant::elapsed -> Duration::as_millis -> ComponentHealth::healthy -> ComponentHealth::unhealthy
**Steps:**
1. Returns a degraded status if `self.bm25` is `None` ("BM25 search not configured").
2. Records the start time with `Instant::now()`.
3. Issues a synchronous `bm25.search("__health_check__", 1)` probe query.
4. On success, computes elapsed milliseconds and returns a healthy status with that latency.
5. On error, returns an unhealthy status formatted with the debug-printed error.

### `HealthMonitor::check_vector(&self) -> ComponentHealth` (async, private)
**Call graph:** ComponentHealth::degraded -> Instant::now -> VectorStore::count -> Instant::elapsed -> Duration::as_millis -> ComponentHealth::healthy -> ComponentHealth::unhealthy
**Steps:**
1. Returns a degraded status if `self.vector_store` is `None` ("Vector store not configured").
2. Captures `Instant::now()` as the latency baseline.
3. Awaits `vector_store.count()` to verify the collection is reachable and obtain the vector count.
4. On success, computes elapsed milliseconds and returns a healthy status describing the count.
5. On error, returns an unhealthy status with the formatted error message.

### `HealthMonitor::check_merkle(&self) -> ComponentHealth` (async, private)
**Call graph:** Path::exists -> std::fs::metadata -> Metadata::len -> ComponentHealth::healthy -> ComponentHealth::degraded
**Steps:**
1. Tests whether `self.merkle_path` exists on disk.
2. If present, reads the file metadata; on success returns a healthy status reporting the size in bytes.
3. If metadata read fails, returns a degraded status indicating the file exists but is unreadable.
4. If the path does not exist, returns a degraded status noting that the first index is pending.

### `HealthMonitor::calculate_overall_status(&self, bm25, vector, merkle) -> Status` (private)
**Call graph:** (only `PartialEq` comparisons against `Status` variants)
**Steps:**
1. Marks the system as unhealthy only when both `bm25` and `vector` are `Status::Unhealthy`.
2. Otherwise checks for any degraded condition: any component degraded, or either search engine unhealthy.
3. Returns `Status::Degraded` if any of the above flags are set.
4. Returns `Status::Healthy` when no degraded or unhealthy condition is detected.

### Trait impls (derived only)
- `HealthStatus`: `Debug`, `Clone`, `Serialize` — auto-generated; no custom logic.
- `Status`: `Debug`, `Clone`, `Copy`, `Serialize` (with `rename_all = "lowercase"`), `PartialEq`, `Eq` — auto-generated; serializes variants as lowercase strings.
- `ComponentHealth`: `Debug`, `Clone`, `Serialize` (skipping `latency_ms` when `None`) — auto-generated.
