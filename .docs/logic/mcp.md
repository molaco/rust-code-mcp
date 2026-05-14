# mcp — Detailed Logic

## Module: mcp/mod.rs

This file is the module root. It declares the `sync` submodule and re-exports its public items via `pub use sync::*;`. There are no functions or types defined directly here.

## Module: mcp/sync.rs

### `SyncManager` (struct)
Holds a thread-safe set of tracked directories (`Arc<RwLock<HashSet<PathBuf>>>`) and a `Duration` representing the periodic sync interval. Used to drive background incremental reindexing.

### `SyncManager::new(interval_secs: u64) -> Self`
**Call graph:** `Arc::new` -> `RwLock::new` -> `HashSet::new`, `Duration::from_secs`
**Steps:**
1. Construct an empty `HashSet<PathBuf>` wrapped in `RwLock` and `Arc`.
2. Convert `interval_secs` into a `Duration`.
3. Return a new `SyncManager` with these fields initialized.

### `SyncManager::with_defaults(interval_secs: u64) -> Self`
**Call graph:** `Arc::new` -> `RwLock::new` -> `HashSet::new`, `Duration::from_secs`
**Steps:**
1. Initialize the same fields as `new` (empty tracked set, given interval).
2. Return the constructed `SyncManager` (currently behaves identically to `new`, reserved for future XDG-default behavior).

### `SyncManager::track_directory(&self, dir: PathBuf)` (async)
**Call graph:** `RwLock::write` -> `HashSet::insert`, `PathBuf::clone`, `Path::display`, `tracing::info!`
**Steps:**
1. Acquire a write lock on `tracked_dirs`.
2. Insert the cloned `dir` into the set.
3. If insertion was new (returned `true`), log an info message that tracking has begun for that directory.

### `SyncManager::untrack_directory(&self, dir: &Path)` (async)
**Call graph:** `RwLock::write` -> `HashSet::remove`, `Path::display`, `tracing::info!`
**Steps:**
1. Acquire a write lock on `tracked_dirs`.
2. Remove `dir` from the set.
3. If a removal occurred, log an info message that tracking has stopped for that directory.

### `SyncManager::get_tracked_directories(&self) -> Vec<PathBuf>` (async)
**Call graph:** `RwLock::read` -> `HashSet::iter` -> `Iterator::cloned` -> `Iterator::collect`
**Steps:**
1. Acquire a read lock on `tracked_dirs`.
2. Iterate over the set and clone each `PathBuf`.
3. Collect the clones into a `Vec<PathBuf>` and return it.

### `SyncManager::run(self: Arc<Self>)` (async)
**Call graph:** `Duration::as_secs`, `tracing::info!`, `tokio::time::sleep`, `SyncManager::handle_sync_all`, `tokio::time::interval` -> `Interval::tick`
**Steps:**
1. Log that the background sync loop is starting with its interval.
2. Sleep 5 seconds to give the system time to start.
3. Run an initial sync of all tracked directories via `handle_sync_all`.
4. Build a tokio `interval` timer from `self.interval`.
5. Enter an infinite loop that ticks the interval and re-invokes `handle_sync_all` each cycle.

### `SyncManager::handle_sync_all(&self)` (async, private)
**Call graph:** `SyncManager::get_tracked_directories`, `Vec::is_empty`, `tracing::debug!`, `tracing::info!`, `Vec::iter` -> `Iterator::enumerate`, `Path::display`, `SyncManager::sync_directory`, `tracing::error!`
**Steps:**
1. Snapshot the current tracked directories list.
2. If empty, log debug "No directories to sync" and return early.
3. Log info with the count of directories being synced.
4. Iterate (with index) over directories, logging progress per directory.
5. Call `sync_directory` for each; on error, log it but continue with remaining directories.
6. Log info that the sync cycle is complete.

### `SyncManager::sync_directory(&self, dir: &Path) -> Result<()>` (async, private)
**Call graph:** `ProjectPaths::from_directory`, `IncrementalIndexer::new`, `IncrementalIndexer::index_with_change_detection`, `Path::display`, `tracing::info!`, `tracing::debug!`
**Steps:**
1. Derive `ProjectPaths` for the target directory (cache, tantivy index, collection name).
2. Construct an `IncrementalIndexer` configured with these paths, the `EMBEDDING_DIM`, and no override.
3. Run `index_with_change_detection` against the directory to detect and apply changes incrementally.
4. If `stats.indexed_files > 0`, log info with files indexed and total chunks.
5. Otherwise, log a debug message stating no changes were detected.
6. Return `Ok(())` on success or propagate any error via `?`.

### `SyncManager::sync_now(&self)` (async)
**Call graph:** `tracing::info!`, `SyncManager::handle_sync_all`
**Steps:**
1. Log an info message that a manual sync was triggered.
2. Delegate to `handle_sync_all` to sync every tracked directory immediately.

### `SyncManager::sync_directory_now(&self, dir: &Path) -> Result<()>` (async)
**Call graph:** `tracing::info!`, `Path::display`, `SyncManager::sync_directory`
**Steps:**
1. Log an info message indicating a manual sync was triggered for the given directory.
2. Delegate to `sync_directory` for that single path and return its `Result`.
