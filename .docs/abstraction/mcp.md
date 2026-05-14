# mcp — Abstract Logic

## Module: mcp/mod.rs
**Purpose:** Module root that exposes the `sync` submodule's public API via re-export.

1. **Declare and re-export sync submodule** -> `pub use sync::*`

## Module: mcp/sync.rs
**Purpose:** Manages a tracked set of project directories and drives periodic incremental reindexing in the background.

1. **Construct a sync manager with an empty tracked-directory set and configured interval** -> `SyncManager::new()`, `SyncManager::with_defaults()`
2. **Add or remove directories from the tracked set under a write lock** -> `SyncManager::track_directory()`, `SyncManager::untrack_directory()`
3. **Snapshot the current tracked directories under a read lock** -> `SyncManager::get_tracked_directories()`
4. **Run the background loop that periodically syncs all tracked directories** -> `SyncManager::run()`
5. **Iterate tracked directories and incrementally reindex each one, tolerating per-directory errors** -> `SyncManager::handle_sync_all()`, `SyncManager::sync_directory()`
6. **Trigger a manual sync of all directories or a single directory on demand** -> `SyncManager::sync_now()`, `SyncManager::sync_directory_now()`
