//! The daemon's lock model: every lock a tool call can wait on, in one order.
//!
//! The workspace locks are declared here; the process locks live beside what
//! they guard and are listed here, so this table is the whole model. A new
//! lock joins this table and the order below, not a call site.
//!
//! # Lock order
//!
//! ```text
//! workspace (async, tokio; held across .await):
//!   global (read) → graph cache → maintenance → operation
//! process (taken inside a workspace lock or alone, never the reverse):
//!   analysis gate (read) → project slot → project Vfs → semantic registry
//!   GPU broker grant → GPU flock
//!   search cache, tracked directories, graph env cache — leaves: held for a lookup, nothing taken under them
//! ```
//!
//! Each workspace guard is built in that order by one constructor below, and
//! the only inner lock handed out separately comes from a guard of the level
//! above it ([`GraphCacheWriterGuard::read_index`],
//! [`StoreWriterGuard::operation_lock`]), so a guard cannot be taken out of
//! order. No workspace lock is taken under a process lock: rust-analyzer work
//! takes none, and a process lock is let go before its holder awaits anything
//! but itself (the tracked directories are a tokio lock, held for a set
//! update only).
//!
//! ## Workspace locks ([`WorkspaceLockRegistry`], keyed by canonical directory)
//!
//! | Lock | Taken by | Protects |
//! |---|---|---|
//! | `global` read | every guard below | a whole-cache `lock_all` (directory-less `clear_cache` / `clear_runtime`, shutdown) |
//! | `global` write | [`WorkspaceLockRegistry::lock_all`] | everything: waits for every guard below |
//! | graph cache | [`WorkspaceLockRegistry::lock_graph_cache_writer`]: `semantic_overlaps`, `build_codemap` with `compute_missing` | the hypergraph's embedding cache (LMDB) against two writers embedding the same missing items at once; its readers take nothing (LMDB MVCC), and it keeps no reader of the index out |
//! | maintenance | store writers, index writers, [`MaintenanceGuard`] | the stores against two writers: indexing runs, clears and maintenance passes (compaction, index extension, prune) one at a time |
//! | operation read | [`WorkspaceLockRegistry::lock_shared`], a graph cache writer that also reads the index | the workspace's index and caches against a writer, while readers run side by side |
//! | operation write | index writers for their whole run, store writers only while they publish | readers kept off what a writer has half done |
//!
//! The graph cache comes before the operation lock on purpose: a graph cache
//! writer queued behind another one (embedding for minutes) holds no read
//! lock while it waits, so it cannot hold up a sync's publish — and with it,
//! through tokio's fair queue, every reader arriving after the publish.
//!
//! ## Process locks
//!
//! | Lock | Where | Held for |
//! |---|---|---|
//! | analysis gate | `crate::deep_stack` (`ANALYSIS_GATE`) | read: one rust-analyzer analysis; write: one garbage collection, `try_write` only — it never queues ahead of analyses |
//! | project slot | `crate::semantic::SemanticService` (`ProjectSlot::context`) | the load, refresh or snapshot of ONE project; queries run on the snapshot without it |
//! | project Vfs | the slot's `Vfs` lock | read beside a query's snapshot; write by a refresh under the slot |
//! | semantic registry | `SemanticService::registry` | lookup, insert, touch, evict of slots; never waits for a slot (eviction `try_lock`s) |
//! | GPU broker grant | `rmc_engine::embeddings::gpu_broker` | one embedding run; search ahead of indexing ahead of graph |
//! | GPU flock | `rmc_engine::embeddings::gpu_lock` (file lock, between daemons) | one GPU run or session creation; waits ≤ 5 min |
//! | search cache | [`crate::mcp::SearchRuntimeCache`] | get / insert / invalidate of an entry |
//! | tracked directories | [`crate::mcp::SyncManager`] | add / remove / list the synced directories |
//! | graph env cache | `rmc_graph::graph::snapshot` (`OPENED_GRAPH_ENVS`) | opening a hypergraph's LMDB env once per process (heed requires it); its transactions are LMDB's own |
//!
//! Operations that clear in-memory state (`clear_runtime` of one workspace)
//! take only these process locks, not the workspace operation lock: a reader
//! in flight keeps what it already holds until it answers, as a query keeps
//! its rust-analyzer snapshot when its project is cleared.
//!
//! Writers of the stores come in two kinds:
//!
//! - A **store writer** ([`StoreWriterGuard`], `lock_store_writer`: a sync,
//!   an ordinary `index_codebase`) holds the maintenance lock only and hands
//!   the operation lock to its run ([`StoreWriterGuard::operation_lock`],
//!   `ReaderExclusion::AroundPublish`), which takes it for write around its
//!   writes alone. The walk and the embedding — nearly all of a sync —
//!   run beside the readers.
//! - An **index writer** ([`IndexWriterGuard`], `lock_index_writer`: a
//!   force or repair `index_codebase`, `clear_cache` of a workspace, `search`
//!   building a missing index) holds both for its whole run: it deletes before
//!   it writes, and a reader must not see the gap.
//!
//! Either turns into a [`MaintenanceGuard`] when only vector-store maintenance
//! remains: the maintenance lock alone, so readers proceed while the pass runs
//! (LanceDB gives them a committed version), the next writer waits for it, and
//! `lock_all` waits for it too.
//!
//! A read guard does not upgrade. An operation that learns under its read lock
//! that it has to write (`search` on a missing or corrupt index) lets it go,
//! takes [`WorkspaceLockRegistry::lock_index_writer`], and checks again under
//! that: another reader may have done the writing while it waited.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmc_indexing::indexing::VectorMaintenance;
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

fn workspace_key(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// The per-workspace locks, in acquisition order.
#[derive(Clone, Default)]
struct WorkspaceSlot {
    graph_cache: Arc<Mutex<()>>,
    maintenance: Arc<Mutex<()>>,
    operation: Arc<RwLock<()>>,
}

/// Registry of async locks keyed by canonical workspace directory.
///
/// Levels (see the module docs for the order and the table):
/// - `global` is a read/write lock. Every per-workspace guard holds it for
///   READ, so operations on different workspaces run concurrently; `lock_all`
///   (whole-cache operations such as a directory-less `clear_cache`) takes it
///   for WRITE and so excludes everyone.
/// - per workspace, an operation read/write lock — readers side by side, a
///   writing operation alone — a maintenance mutex serialising writers of
///   its vector store (indexing runs and maintenance passes), and a graph
///   cache mutex serialising writers of the hypergraph's embedding cache.
///
/// The global level used to be a plain mutex taken by every workspace
/// operation. On a shared daemon that made the per-workspace mutex
/// meaningless: the first `search` on a not-yet-indexed checkout runs a
/// full index under its lock (an hour on a large repo), and every other
/// session's `search` on its own, already-indexed workspace queued behind
/// it with no log line at all.
#[derive(Clone, Default)]
pub struct WorkspaceLockRegistry {
    global: Arc<RwLock<()>>,
    slots: Arc<Mutex<HashMap<PathBuf, WorkspaceSlot>>>,
}

impl WorkspaceLockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    async fn slot_for(&self, dir: &Path) -> (PathBuf, WorkspaceSlot) {
        let key = workspace_key(dir);
        let slot = {
            let mut slots = self.slots.lock().await;
            slots.entry(key.clone()).or_default().clone()
        };
        (key, slot)
    }

    /// Take a read-side workspace lock: waits for a writing operation on the
    /// workspace, not for other readers, and NOT for a maintenance pass of its
    /// vector store. The global read guard is taken first, so a pending
    /// `lock_all` is never starved by a stream of workspace operations
    /// (tokio's `RwLock` is fair).
    ///
    /// The holder must not write the workspace's index or caches. What the
    /// readers do share is guarded on its own: the search cache has its own
    /// mutex, a BM25 refill takes tantivy's writer lock (a second refill is
    /// refused, not queued, and that search reads vectors alone), and a reader
    /// that turns out to need a writer takes [`Self::lock_index_writer`] after
    /// letting this go.
    pub async fn lock_shared(&self, dir: &Path) -> WorkspaceReadGuard {
        let global_guard = self.global.clone().read_owned().await;
        let (workspace, slot) = self.slot_for(dir).await;
        let operation_guard = slot.operation.clone().read_owned().await;
        WorkspaceReadGuard {
            workspace,
            _global_guard: global_guard,
            _operation_guard: operation_guard,
        }
    }

    /// Take the lock of a writer of the hypergraph's embedding cache
    /// (`semantic_overlaps`; `build_codemap` with `compute_missing`, which
    /// then also reads the index through [`GraphCacheWriterGuard::read_index`]).
    ///
    /// Writers of that cache wait for each other, so two calls do not embed
    /// the same missing items twice; nothing else waits for them. The cache
    /// is in the snapshot's LMDB, which commits each write in a transaction
    /// and lets its readers read the last commit, so the index readers have
    /// nothing to be kept off. They used to be: these calls took the
    /// workspace's operation lock for write and parked every `search` of the
    /// workspace for as long as they embedded.
    pub async fn lock_graph_cache_writer(&self, dir: &Path) -> GraphCacheWriterGuard {
        let global_guard = self.global.clone().read_owned().await;
        let (workspace, slot) = self.slot_for(dir).await;
        let graph_cache_guard = slot.graph_cache.clone().lock_owned().await;
        GraphCacheWriterGuard {
            workspace,
            _global_guard: global_guard,
            _graph_cache_guard: graph_cache_guard,
            operation: slot.operation,
        }
    }

    /// Take the lock of an operation that writes the workspace's stores and
    /// keeps its readers out for the whole run (clearing, a force or repair
    /// reindex, building a missing index): the maintenance lock — so it waits
    /// for a writer or maintenance pass in flight and no pass starts under it
    /// — and then the operation lock for write.
    pub async fn lock_index_writer(&self, dir: &Path) -> IndexWriterGuard {
        let global_guard = self.global.clone().read_owned().await;
        let (workspace, slot) = self.slot_for(dir).await;
        let maintenance_guard = slot.maintenance.clone().lock_owned().await;
        let operation_guard = slot.operation.clone().write_owned().await;
        IndexWriterGuard {
            workspace,
            _global_guard: global_guard,
            operation_guard,
            maintenance_guard,
        }
    }

    /// Take the lock of a run that writes the workspace's stores and keeps
    /// its readers out only while it publishes (a sync, an ordinary
    /// `index_codebase`): the maintenance lock, which serialises it with every
    /// other writer, and NOT the operation lock — the run takes that for write
    /// itself, around its writes ([`StoreWriterGuard::operation_lock`]).
    ///
    /// Measured 2026-10-31: a sync of 16 files held readers out 23 s when it
    /// took the operation lock for the whole run; its writes are 0.4 s.
    pub async fn lock_store_writer(&self, dir: &Path) -> StoreWriterGuard {
        let global_guard = self.global.clone().read_owned().await;
        let (workspace, slot) = self.slot_for(dir).await;
        let maintenance_guard = slot.maintenance.clone().lock_owned().await;
        StoreWriterGuard {
            workspace,
            _global_guard: global_guard,
            maintenance_guard,
            operation: slot.operation,
        }
    }

    /// Take the global operation lock: waits for every in-flight workspace
    /// operation and maintenance pass, and blocks new ones until dropped.
    pub async fn lock_all(&self) -> WorkspaceGlobalLockGuard {
        WorkspaceGlobalLockGuard {
            _global_guard: self.global.clone().write_owned().await,
        }
    }
}

/// Held read side of a workspace's operation lock (with the global read guard).
pub struct WorkspaceReadGuard {
    workspace: PathBuf,
    _global_guard: OwnedRwLockReadGuard<()>,
    _operation_guard: OwnedRwLockReadGuard<()>,
}

impl WorkspaceReadGuard {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}

/// Held lock of a hypergraph embedding-cache writer: global read and the
/// workspace's graph cache lock, no operation lock.
pub struct GraphCacheWriterGuard {
    workspace: PathBuf,
    _global_guard: OwnedRwLockReadGuard<()>,
    _graph_cache_guard: OwnedMutexGuard<()>,
    operation: Arc<RwLock<()>>,
}

impl GraphCacheWriterGuard {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Also read the workspace's index, as a reader does: the operation lock
    /// for read, taken AFTER the graph cache lock — the order this type
    /// enforces (see the module docs for why the other way round would let a
    /// queued cache writer hold up a sync's publish).
    pub async fn read_index(self) -> GraphCacheReadingIndexGuard {
        let operation_guard = self.operation.clone().read_owned().await;
        GraphCacheReadingIndexGuard {
            _graph_cache_writer: self,
            _operation_guard: operation_guard,
        }
    }
}

/// A [`GraphCacheWriterGuard`] that also reads the workspace's index.
pub struct GraphCacheReadingIndexGuard {
    _graph_cache_writer: GraphCacheWriterGuard,
    _operation_guard: OwnedRwLockReadGuard<()>,
}

/// Held lock of an index writer: global read, maintenance, operation write.
pub struct IndexWriterGuard {
    workspace: PathBuf,
    _global_guard: OwnedRwLockReadGuard<()>,
    operation_guard: OwnedRwLockWriteGuard<()>,
    maintenance_guard: OwnedMutexGuard<()>,
}

impl IndexWriterGuard {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Let the operation lock go and keep the rest: the writer is done
    /// indexing and only its vector-store maintenance remains. No other
    /// writer slips in between — they need the maintenance lock this keeps.
    pub fn into_maintenance(self) -> MaintenanceGuard {
        drop(self.operation_guard);
        MaintenanceGuard {
            workspace: self.workspace,
            _global_guard: self._global_guard,
            _maintenance_guard: self.maintenance_guard,
        }
    }
}

/// Held lock of a store writer that publishes on its own: global read and
/// maintenance. Its run takes [`Self::operation_lock`] for write around its
/// writes (`ReaderExclusion::AroundPublish`).
pub struct StoreWriterGuard {
    workspace: PathBuf,
    _global_guard: OwnedRwLockReadGuard<()>,
    maintenance_guard: OwnedMutexGuard<()>,
    operation: Arc<RwLock<()>>,
}

impl StoreWriterGuard {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The workspace's operation lock, for the run to take for write while it
    /// publishes. Taking it here keeps the lock order: maintenance (held)
    /// before operation.
    pub fn operation_lock(&self) -> &RwLock<()> {
        &self.operation
    }

    /// Only vector-store maintenance remains.
    pub fn into_maintenance(self) -> MaintenanceGuard {
        MaintenanceGuard {
            workspace: self.workspace,
            _global_guard: self._global_guard,
            _maintenance_guard: self.maintenance_guard,
        }
    }
}

/// Held maintenance lock of a workspace (with the global read guard): a
/// vector-store maintenance pass runs under it, outside the operation lock.
pub struct MaintenanceGuard {
    workspace: PathBuf,
    _global_guard: OwnedRwLockReadGuard<()>,
    _maintenance_guard: OwnedMutexGuard<()>,
}

impl MaintenanceGuard {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}

/// Run a writer's vector-store maintenance outside the operation lock.
///
/// The one place a writer's maintenance runs: it takes a
/// [`MaintenanceGuard`], which holds no operation lock, so the workspace's
/// readers do not wait for the pass (measured 2026-10-31: `search` waited
/// 413.9 s for a 425.2 s pass that ran under the operation lock), while the
/// next writer and `lock_all` still do.
pub async fn run_outside_operation_lock<F>(guard: MaintenanceGuard, maintenance: F) -> F::Output
where
    F: Future,
{
    let _maintenance_guard = guard;
    maintenance.await
}

/// Finish a writer that answers a tool call: its operation lock is already
/// gone (the guard is a [`MaintenanceGuard`]), and the vector-store
/// maintenance its run calls for (if any) runs in the background under the
/// maintenance lock — the caller answers without waiting for the pass, and so
/// do the workspace's readers. A panic in the pass is logged like a failed
/// sync, not lost with the task.
pub fn finish_index_writer(guard: MaintenanceGuard, maintenance: Option<VectorMaintenance>) {
    let Some(pass) = maintenance else {
        return;
    };
    let workspace_path = guard.workspace().to_path_buf();
    let workspace = workspace_path.display().to_string();
    tokio::spawn(async move {
        let run =
            run_outside_operation_lock(guard, async move { pass.run(&workspace_path).await });
        if let Err(message) = crate::panic_boundary::catch_panic(run).await {
            tracing::error!(
                target: "panic",
                "vector store maintenance of {workspace} panicked: {message}; the next write runs it again"
            );
        }
    });
}

/// Held global operation lock.
pub struct WorkspaceGlobalLockGuard {
    _global_guard: OwnedRwLockWriteGuard<()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    #[tokio::test]
    async fn workspace_lock_blocks_same_workspace() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();

        let guard = registry.lock_index_writer(&workspace).await;
        let waiter_registry = registry.clone();
        let waiter_workspace = workspace.clone();
        let waiter = tokio::spawn(async move {
            let _guard = waiter_registry.lock_index_writer(&waiter_workspace).await;
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());

        drop(guard);
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn global_lock_blocks_workspace_lock() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();

        let guard = registry.lock_all().await;
        let waiter_registry = registry.clone();
        let waiter_workspace = workspace.clone();
        let waiter = tokio::spawn(async move {
            let _guard = waiter_registry.lock_index_writer(&waiter_workspace).await;
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());

        drop(guard);
        waiter.await.unwrap();
    }

    /// The regression this registry exists to prevent: a long operation on
    /// one workspace (first `search` on an unindexed checkout = full index)
    /// must not stall an operation on another workspace.
    #[tokio::test]
    async fn different_workspaces_do_not_block_each_other() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let a = temp_dir.path().join("a");
        let b = temp_dir.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        let _held_a = registry.lock_index_writer(&a).await;
        let other = tokio::time::timeout(Duration::from_secs(5), registry.lock_shared(&b)).await;
        assert!(other.is_ok(), "lock on workspace b waited for workspace a");
    }

    #[tokio::test]
    async fn global_lock_waits_for_workspace_lock() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();

        let guard = registry.lock_index_writer(&workspace).await;
        let waiter_registry = registry.clone();
        let waiter = tokio::spawn(async move {
            let _guard = waiter_registry.lock_all().await;
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());

        drop(guard);
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn workspace_lock_reports_canonical_workspace() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();

        let guard = registry.lock_index_writer(&workspace.join(".")).await;

        assert_eq!(guard.workspace(), std::fs::canonicalize(&workspace).unwrap());
    }

    /// A maintenance pass that runs until the test lets it finish, through
    /// [`run_outside_operation_lock`] as the index writers run theirs.
    /// Returns the release switch and the task; the pass has started when
    /// this returns.
    async fn hold_maintenance(
        registry: &WorkspaceLockRegistry,
        workspace: &Path,
    ) -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let writer = registry.lock_index_writer(workspace).await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(run_outside_operation_lock(writer.into_maintenance(), async move {
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
        }));
        started_rx.await.unwrap();
        (release_tx, task)
    }

    fn workspace_in(temp_dir: &TempDir) -> PathBuf {
        let workspace = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        workspace
    }

    /// The point of the maintenance lock: a reader of the workspace does not
    /// wait for the vector-store maintenance a sync left behind (it waited
    /// 413.9 s on 2026-10-31). Mutant: `MaintenanceGuard` keeping the
    /// operation lock → this times out.
    #[tokio::test]
    async fn reader_passes_a_maintenance_pass_in_flight() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let (release, pass) = hold_maintenance(&registry, &workspace).await;
        let reader =
            tokio::time::timeout(Duration::from_secs(5), registry.lock_shared(&workspace)).await;
        assert!(reader.is_ok(), "lock_shared waited for the maintenance pass");
        drop(reader);

        release.send(()).unwrap();
        pass.await.unwrap();
    }

    /// Two writers of one vector store stay serialised: the next indexing run
    /// waits for the maintenance pass in flight.
    #[tokio::test]
    async fn index_writer_waits_for_a_maintenance_pass() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let (release, pass) = hold_maintenance(&registry, &workspace).await;
        let waiter_registry = registry.clone();
        let waiter_workspace = workspace.clone();
        let writer = tokio::spawn(async move {
            let _guard = waiter_registry.lock_index_writer(&waiter_workspace).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !writer.is_finished(),
            "an index writer started while a maintenance pass was running"
        );

        release.send(()).unwrap();
        pass.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), writer)
            .await
            .expect("the index writer did not get the lock after the pass")
            .unwrap();
    }

    /// Readers of one workspace do not wait for each other: a long `search`
    /// (or `get_similar_code`, `codemap`) does not hold up the next one.
    /// Mutant (checked 2026-10-31): `lock_shared` taking the write side → this
    /// times out.
    #[tokio::test]
    async fn readers_of_one_workspace_do_not_wait_for_each_other() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let _first = registry.lock_shared(&workspace).await;
        let second =
            tokio::time::timeout(Duration::from_secs(5), registry.lock_shared(&workspace)).await;
        assert!(second.is_ok(), "a second reader waited for the first");
    }

    /// The other half of a read/write lock: a writing operation (a sync's
    /// index run, `index_codebase`, `clear_cache` of the workspace) waits for
    /// the readers inside, and a reader arriving meanwhile waits for it — a
    /// reader never sees an index mid-write.
    #[tokio::test]
    async fn a_writer_waits_for_readers_and_readers_wait_for_a_writer() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let reader = registry.lock_shared(&workspace).await;
        let writer = tokio::spawn({
            let registry = registry.clone();
            let workspace = workspace.clone();
            async move {
                let _guard = registry.lock_index_writer(&workspace).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!writer.is_finished(), "an index writer ran beside a reader");

        drop(reader);
        // The writer now holds the lock (or is about to): a reader waits.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let late_reader = tokio::spawn({
            let registry = registry.clone();
            let workspace = workspace.clone();
            async move {
                let _guard = registry.lock_shared(&workspace).await;
            }
        });
        tokio::time::timeout(Duration::from_secs(5), writer)
            .await
            .expect("the writer never got the lock after the reader left")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), late_reader)
            .await
            .expect("the reader never got the lock after the writer left")
            .unwrap();
    }

    /// A store writer (a sync) serialises with the other writers but lets the
    /// readers in until it publishes: `lock_shared` passes beside it, another
    /// writer waits for it, and its own publish waits for the readers inside.
    /// Mutant: `lock_store_writer` taking the operation lock for write → the
    /// first assertion times out.
    #[tokio::test]
    async fn a_store_writer_lets_readers_in_until_it_publishes() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let writer = registry.lock_store_writer(&workspace).await;
        let reader =
            tokio::time::timeout(Duration::from_secs(5), registry.lock_shared(&workspace)).await;
        assert!(reader.is_ok(), "a reader waited for a store writer that was not publishing");
        let reader = reader.unwrap();

        let other = tokio::spawn({
            let registry = registry.clone();
            let workspace = workspace.clone();
            async move {
                let _guard = registry.lock_index_writer(&workspace).await;
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!other.is_finished(), "a second writer ran beside a store writer");

        // Publishing waits for the reader inside.
        let publish = writer.operation_lock().try_write();
        assert!(publish.is_err(), "the publish did not wait for the reader inside");
        drop(publish);
        drop(reader);
        let publish = tokio::time::timeout(Duration::from_secs(5), writer.operation_lock().write())
            .await
            .expect("the publish never got in after the reader left");
        drop(publish);

        drop(writer);
        tokio::time::timeout(Duration::from_secs(5), other)
            .await
            .expect("the second writer never got the lock")
            .unwrap();
    }

    /// A writer of the hypergraph's embedding cache (`semantic_overlaps`
    /// embedding for minutes) keeps no reader of the workspace's index out,
    /// and no store writer's publish either. Mutant (checked 2026-10-31):
    /// `lock_graph_cache_writer` taking the operation lock for write, as
    /// `lock_exclusive` did → both time out.
    #[tokio::test]
    async fn a_graph_cache_writer_keeps_no_reader_or_publish_out() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let _cache_writer = registry.lock_graph_cache_writer(&workspace).await;
        let reader =
            tokio::time::timeout(Duration::from_secs(5), registry.lock_shared(&workspace)).await;
        assert!(reader.is_ok(), "a reader waited for a graph cache writer");
        drop(reader);

        let store_writer = registry.lock_store_writer(&workspace).await;
        let publish =
            tokio::time::timeout(Duration::from_secs(5), store_writer.operation_lock().write())
                .await;
        assert!(publish.is_ok(), "a sync's publish waited for a graph cache writer");
    }

    /// Two writers of the graph cache wait for each other (so they do not
    /// embed the same missing items twice), and the one waiting holds no read
    /// lock on the index meanwhile: a sync's publish — and every reader queued
    /// behind it — gets in. Mutant (checked 2026-10-31): the second writer
    /// taking the order the type rules out — `lock_shared`, then
    /// `lock_graph_cache_writer` — → the publish times out.
    #[tokio::test]
    async fn a_queued_graph_cache_writer_does_not_hold_up_a_publish() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let first = registry.lock_graph_cache_writer(&workspace).await;
        let second = tokio::spawn({
            let registry = registry.clone();
            let workspace = workspace.clone();
            async move {
                let _guard = registry
                    .lock_graph_cache_writer(&workspace)
                    .await
                    .read_index()
                    .await;
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !second.is_finished(),
            "a second graph cache writer ran beside the first"
        );

        let store_writer = registry.lock_store_writer(&workspace).await;
        let publish =
            tokio::time::timeout(Duration::from_secs(5), store_writer.operation_lock().write())
                .await;
        assert!(
            publish.is_ok(),
            "a sync's publish waited for a graph cache writer queued behind another"
        );
        drop(publish);
        drop(store_writer);

        drop(first);
        tokio::time::timeout(Duration::from_secs(5), second)
            .await
            .expect("the second graph cache writer never got the lock")
            .unwrap();
    }

    /// A graph cache writer that reads the index is a reader of it: it waits
    /// for an index writer, as `search` does.
    #[tokio::test]
    async fn a_graph_cache_writer_reading_the_index_waits_for_an_index_writer() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let index_writer = registry.lock_index_writer(&workspace).await;
        let cache_writer = tokio::spawn({
            let registry = registry.clone();
            let workspace = workspace.clone();
            async move {
                let _guard = registry
                    .lock_graph_cache_writer(&workspace)
                    .await
                    .read_index()
                    .await;
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !cache_writer.is_finished(),
            "a graph cache writer read the index under an index writer"
        );
        drop(index_writer);
        tokio::time::timeout(Duration::from_secs(5), cache_writer)
            .await
            .expect("the graph cache writer never got the index after the writer left")
            .unwrap();
    }

    /// `clear_cache` without a directory deletes stores wholesale: it must
    /// not run under a maintenance pass.
    #[tokio::test]
    async fn global_lock_waits_for_a_maintenance_pass() {
        let registry = WorkspaceLockRegistry::new();
        let temp_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&temp_dir);

        let (release, pass) = hold_maintenance(&registry, &workspace).await;
        let waiter_registry = registry.clone();
        let global = tokio::spawn(async move {
            let _guard = waiter_registry.lock_all().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!global.is_finished(), "lock_all ran under a maintenance pass");

        release.send(()).unwrap();
        pass.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), global)
            .await
            .expect("lock_all did not get the lock after the pass")
            .unwrap();
    }
}
