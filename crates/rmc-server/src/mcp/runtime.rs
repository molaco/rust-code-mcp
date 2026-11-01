use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::schemars;
use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::deep_stack::try_run_exclusive_analysis;
use crate::semantic::{SemanticService, SemanticServiceStatus};

use super::memory::{MemoryRelease, release_and_measure, rss_kib};
use super::{
    SearchRuntimeCache, SearchRuntimeCacheStatus, SyncManager, SyncManagerStatus,
    WorkspaceLockRegistry,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeClearScope {
    All,
    Workspace,
    SemanticOnly,
    SearchCacheOnly,
    SyncTrackingOnly,
}

impl Default for RuntimeClearScope {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeClearRequest {
    pub scope: RuntimeClearScope,
    pub workspace: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeClearReport {
    pub scope: RuntimeClearScope,
    pub workspace: Option<String>,
    pub search_cache_entries_cleared: usize,
    pub semantic_projects_cleared: usize,
    pub sync_directories_untracked: usize,
    /// What clearing actually did to the process's memory.
    ///
    /// Without this the report counted objects it had dropped and said nothing
    /// about the only quantity the operator came for. A run that clears one
    /// project and returns zero bytes — the normal case, see
    /// [`super::memory`] — has to be visible in the answer, not discovered
    /// later with `ps`.
    pub memory: MemoryRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeSyncStatus {
    pub enabled: bool,
    pub tracked_count: usize,
    pub tracked_directories: Vec<String>,
}

impl RuntimeSyncStatus {
    fn disabled() -> Self {
        Self {
            enabled: false,
            tracked_count: 0,
            tracked_directories: Vec::new(),
        }
    }

    fn from_sync_manager(status: SyncManagerStatus) -> Self {
        Self {
            enabled: true,
            tracked_count: status.tracked_count,
            tracked_directories: status.tracked_directories,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackgroundSyncStatus {
    pub enabled: bool,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessStatus {
    pub pid: u32,
    pub rss_kib: Option<u64>,
}

/// The BM25 index of one sync-tracked directory, as the probe reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bm25IndexStatus {
    pub directory: String,
    /// The probe's report, or why it could not read the index (no index yet
    /// is an `Err` too — the message says so).
    pub stats: Result<rmc_indexing::indexing::index_stats::Bm25IndexStats, String>,
}

/// What the keyword indexes of this daemon look like and cost right now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bm25Status {
    /// Threads of this process named `merge_thread_*` — tantivy's segment
    /// merge pools, one per live index writer (up to four threads each; the
    /// pool starts them as merges arrive, so an idle writer shows one). Several
    /// `merge_thread_0` at once = several writers alive. The only reading of merge
    /// activity that does not depend on a writer telling the truth: a writer
    /// dropped with merges in flight (the bug that kept three pools busy on a
    /// daemon for hours) leaves its threads here, and nowhere else.
    pub merge_threads: usize,
    pub indexes: Vec<Bm25IndexStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeStatus {
    pub sync: RuntimeSyncStatus,
    pub search_cache: SearchRuntimeCacheStatus,
    pub semantic: SemanticServiceStatus,
    pub background_sync: BackgroundSyncStatus,
    pub process: ProcessStatus,
    /// GPU embedding sessions by owner and the VRAM they were charged,
    /// against this process's and the card's meters.
    pub gpu: rmc_engine::embeddings::GpuLedgerReport,
    pub bm25: Bm25Status,
}

#[derive(Clone)]
pub struct RuntimeState {
    sync_manager: Option<Arc<SyncManager>>,
    workspace_locks: WorkspaceLockRegistry,
    search_cache: SearchRuntimeCache,
    semantic: Arc<SemanticService>,
    background_sync_enabled: Arc<AtomicBool>,
    background_sync_running: Arc<AtomicBool>,
}

impl RuntimeState {
    pub fn standalone() -> Self {
        Self {
            sync_manager: None,
            workspace_locks: WorkspaceLockRegistry::new(),
            search_cache: SearchRuntimeCache::new(),
            semantic: Arc::new(SemanticService::new()),
            background_sync_enabled: Arc::new(AtomicBool::new(false)),
            background_sync_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_sync_manager(sync_manager: Arc<SyncManager>) -> Self {
        let workspace_locks = sync_manager.workspace_locks();
        Self {
            sync_manager: Some(sync_manager),
            workspace_locks,
            search_cache: SearchRuntimeCache::new(),
            semantic: Arc::new(SemanticService::new()),
            background_sync_enabled: Arc::new(AtomicBool::new(false)),
            background_sync_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn sync_manager(&self) -> Option<&Arc<SyncManager>> {
        self.sync_manager.as_ref()
    }

    pub fn background_sync_manager(&self) -> Option<&Arc<SyncManager>> {
        if self.background_sync_enabled() {
            self.sync_manager.as_ref()
        } else {
            None
        }
    }

    pub fn workspace_locks(&self) -> &WorkspaceLockRegistry {
        &self.workspace_locks
    }

    pub fn search_cache(&self) -> &SearchRuntimeCache {
        &self.search_cache
    }

    pub(crate) fn semantic(&self) -> Arc<SemanticService> {
        Arc::clone(&self.semantic)
    }

    pub fn background_sync_enabled(&self) -> bool {
        self.background_sync_enabled.load(Ordering::SeqCst)
    }

    pub fn background_sync_running(&self) -> bool {
        self.background_sync_running.load(Ordering::SeqCst)
    }

    pub async fn status(&self) -> RuntimeStatus {
        let bm25 = match &self.sync_manager {
            Some(sync_manager) => {
                bm25_status(&sync_manager.get_tracked_directories().await)
            }
            None => bm25_status(&[]),
        };
        let sync = match &self.sync_manager {
            Some(sync_manager) => RuntimeSyncStatus::from_sync_manager(sync_manager.status().await),
            None => RuntimeSyncStatus::disabled(),
        };
        // Registry only: answers while any project is loading.
        let semantic = self.semantic.status();
        RuntimeStatus {
            sync,
            search_cache: self.search_cache.status(),
            semantic,
            background_sync: BackgroundSyncStatus {
                enabled: self.background_sync_enabled(),
                running: self.background_sync_running(),
            },
            process: current_process_status(),
            gpu: rmc_engine::embeddings::gpu_ledger_report(),
            bm25,
        }
    }

    /// Bump the revision of every loaded analysis and sweep the type interner.
    ///
    /// The reason this exists at all is in [`SemanticService::collect_garbage`]:
    /// salsa only evicts LRU-capped memos when the revision bumps, and a project
    /// nobody edits never bumps on its own. Called on a timer by the daemon's
    /// watchdog; returns the number of contexts collected, for its log line.
    ///
    /// # Two things about *how* it is called
    ///
    /// It runs on the analysis thread, not on a runtime worker: the interner
    /// sweep recurses over the shape of the analyzed types, and a 2 MiB tokio
    /// stack is what took the whole server down with an `abort` the last time
    /// rust-analyzer work was run on one.
    ///
    /// It takes that thread *exclusively*
    /// ([`crate::deep_stack::try_run_exclusive_analysis`]): the semantic
    /// service is not the only rust-analyzer in the process — the graph tools,
    /// the audits and the skeleton builder each load a workspace of their own,
    /// and this method runs on a timer that knows nothing about them. The gate
    /// is what makes "no query is in flight" true rather than assumed; without
    /// it the overlap is a `SIGSEGV`, which is how it was found.
    ///
    /// And only if it can have the gate *now*: `None` means analyses were
    /// inside and the sweep was skipped — the caller tries again on its next
    /// tick. Waiting instead would park every analysis that arrives behind the
    /// waiting sweep, on every project, for as long as the longest one inside
    /// runs (a load: minutes).
    ///
    /// It does *not* take the workspace locks that [`Self::clear`] takes. Those
    /// guard the search index and the sync manager, neither of which this
    /// touches; taking them on a timer would stall indexing for no reason.
    pub async fn collect_garbage(&self) -> Option<usize> {
        let semantic = Arc::clone(&self.semantic);
        match try_run_exclusive_analysis("collect_garbage", move || semantic.collect_garbage())
            .await
        {
            Ok(collected) => collected,
            Err(error) => {
                tracing::warn!("garbage collection did not run: {}", error.message);
                Some(0)
            }
        }
    }

    /// Stop loading rust-analyzer in this process, for good: the daemon retired.
    ///
    /// Every later semantic, graph-building, audit or skeleton call is refused
    /// with a message telling the session how to reach the successor; garbage
    /// collection and everything that loads no analysis keep working. Call it
    /// BEFORE unloading, so no load can slip in between the unload and the flag.
    ///
    /// ⚠ Process-wide rather than scoped to this `RuntimeState`, and the method
    /// takes `&self` only so a caller that holds the runtime finds it here —
    /// see `deep_stack::RETIRED` for why the flag cannot live in the state.
    pub fn refuse_new_analyses(&self) {
        crate::deep_stack::refuse_new_analyses();
    }

    pub async fn clear(&self, request: RuntimeClearRequest) -> RuntimeClearReport {
        let workspace = request.workspace.as_deref().map(normalize_workspace);
        if let Some(workspace) = &workspace {
            // No workspace lock: the search cache, the rust-analyzer projects and
            // the tracked directories each sit behind a lock of their own, and a
            // reader in flight keeps what it already holds until it answers. The
            // write side this took before made the clear wait for the longest
            // `search` of the workspace and parked every new one behind it.
            self.clear_locked(request.scope, Some(workspace)).await
        } else {
            let _workspace_lock = self.workspace_locks.lock_all().await;
            self.clear_locked(request.scope, None).await
        }
    }

    async fn clear_locked(
        &self,
        scope: RuntimeClearScope,
        workspace: Option<&Path>,
    ) -> RuntimeClearReport {
        let mut report = RuntimeClearReport {
            scope,
            workspace: workspace.map(|path| path.display().to_string()),
            search_cache_entries_cleared: 0,
            semantic_projects_cleared: 0,
            sync_directories_untracked: 0,
            memory: MemoryRelease::skipped(),
        };

        match (scope, workspace) {
            (RuntimeClearScope::All, Some(workspace))
            | (RuntimeClearScope::Workspace, Some(workspace)) => {
                report.search_cache_entries_cleared =
                    self.search_cache.invalidate_workspace(workspace);
                report.semantic_projects_cleared = self.semantic.clear_project(workspace);
                report.sync_directories_untracked = self.untrack_workspace(workspace).await;
            }
            (RuntimeClearScope::All, None) => {
                report.search_cache_entries_cleared = self.search_cache.invalidate_all();
                report.semantic_projects_cleared = self.semantic.clear_all();
                report.sync_directories_untracked = self.untrack_all().await;
            }
            (RuntimeClearScope::Workspace, None) => {}
            (RuntimeClearScope::SemanticOnly, Some(workspace)) => {
                report.semantic_projects_cleared = self.semantic.clear_project(workspace);
            }
            (RuntimeClearScope::SemanticOnly, None) => {
                report.semantic_projects_cleared = self.semantic.clear_all();
            }
            (RuntimeClearScope::SearchCacheOnly, Some(workspace)) => {
                report.search_cache_entries_cleared =
                    self.search_cache.invalidate_workspace(workspace);
            }
            (RuntimeClearScope::SearchCacheOnly, None) => {
                report.search_cache_entries_cleared = self.search_cache.invalidate_all();
            }
            (RuntimeClearScope::SyncTrackingOnly, Some(workspace)) => {
                report.sync_directories_untracked = self.untrack_workspace(workspace).await;
            }
            (RuntimeClearScope::SyncTrackingOnly, None) => {
                report.sync_directories_untracked = self.untrack_all().await;
            }
        }

        // After the drops, not before: the allocator can only hand back what the
        // clear has already released. Called unconditionally, including for the
        // no-op scopes, so the report always carries a real measurement rather
        // than sometimes a real one and sometimes a placeholder.
        report.memory = release_and_measure();

        report
    }

    async fn untrack_workspace(&self, workspace: &Path) -> usize {
        match &self.sync_manager {
            Some(sync_manager) if sync_manager.untrack_directory(workspace).await => 1,
            _ => 0,
        }
    }

    async fn untrack_all(&self) -> usize {
        match &self.sync_manager {
            Some(sync_manager) => sync_manager.untrack_all_directories().await,
            None => 0,
        }
    }
}

pub struct ServerRuntime {
    state: RuntimeState,
    shutdown_tx: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl ServerRuntime {
    pub fn new(sync_interval_secs: u64) -> Self {
        Self::with_sync_manager(Arc::new(SyncManager::with_defaults(sync_interval_secs)))
    }

    pub fn with_sync_manager(sync_manager: Arc<SyncManager>) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            state: RuntimeState::with_sync_manager(sync_manager),
            shutdown_tx,
            tasks: Mutex::new(Vec::new()),
        }
    }

    pub fn state(&self) -> RuntimeState {
        self.state.clone()
    }

    pub fn start_background_sync(&self) {
        let Some(sync_manager) = self.state.sync_manager.clone() else {
            return;
        };
        if self
            .state
            .background_sync_enabled
            .swap(true, Ordering::SeqCst)
        {
            return;
        }

        self.state
            .background_sync_running
            .store(true, Ordering::SeqCst);

        let shutdown = self.shutdown_tx.subscribe();
        let running = Arc::clone(&self.state.background_sync_running);
        let handle = tokio::spawn(async move {
            sync_manager.run_until_shutdown(shutdown).await;
            running.store(false, Ordering::SeqCst);
        });

        self.tasks
            .lock()
            .expect("runtime task mutex poisoned")
            .push(handle);
    }

    /// Stop the background tasks — today that is the background sync and
    /// nothing else. A retiring daemon calls this long before it exits, so a
    /// task added to this channel later must be one a draining daemon can do
    /// without.
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub async fn wait_for_tasks(&self, timeout: Duration) -> RuntimeTaskReport {
        let mut handles = {
            let mut tasks = self.tasks.lock().expect("runtime task mutex poisoned");
            tasks.drain(..).collect::<Vec<_>>()
        };

        let mut report = RuntimeTaskReport {
            tasks_total: handles.len(),
            tasks_completed: 0,
            tasks_aborted: 0,
            join_errors: 0,
            timed_out: false,
        };

        let deadline = tokio::time::Instant::now() + timeout;
        while let Some(mut handle) = handles.pop() {
            if tokio::time::Instant::now() >= deadline {
                handle.abort();
                handles.push(handle);
                report.timed_out = true;
                break;
            }

            match tokio::time::timeout_at(deadline, &mut handle).await {
                Ok(Ok(())) => report.tasks_completed += 1,
                Ok(Err(_)) => report.join_errors += 1,
                Err(_) => {
                    handle.abort();
                    handles.push(handle);
                    report.timed_out = true;
                    break;
                }
            }
        }

        if report.timed_out {
            for handle in handles {
                if !handle.is_finished() {
                    handle.abort();
                    report.tasks_aborted += 1;
                }
            }
            self.state
                .background_sync_running
                .store(false, Ordering::SeqCst);
        }

        report
    }

    pub async fn shutdown_gracefully(&self, timeout: Duration) -> RuntimeTaskReport {
        self.request_shutdown();
        let report = self.wait_for_tasks(timeout).await;
        let _ = self
            .state
            .clear(RuntimeClearRequest {
                scope: RuntimeClearScope::All,
                workspace: None,
            })
            .await;
        report
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeTaskReport {
    pub tasks_total: usize,
    pub tasks_completed: usize,
    pub tasks_aborted: usize,
    pub join_errors: usize,
    pub timed_out: bool,
}

fn normalize_workspace(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The BM25 index of every directory background sync tracks, plus the merge
/// threads alive in this process. Reads `meta.json` and file sizes only (see
/// `index_stats`), so it is safe while the sync writes.
fn bm25_status(tracked: &[PathBuf]) -> Bm25Status {
    let indexes = tracked
        .iter()
        .map(|dir| {
            // The BM25 directory belongs to a collection: probe the one
            // search and background sync use for the automatic profile.
            let stats = super::project_paths::ProjectPaths::in_use(
                dir,
                &super::defaults::automatic_embedding_backend(),
            )
            .and_then(|paths| {
                rmc_indexing::indexing::index_stats::probe_bm25_index(&paths.tantivy_path)
                    .map_err(|e| format!("{e:#}"))
            });
            Bm25IndexStatus {
                directory: dir.display().to_string(),
                stats,
            }
        })
        .collect();
    Bm25Status {
        merge_threads: merge_thread_count(),
        indexes,
    }
}

/// Threads of this process whose name starts with `merge_thread` (Linux:
/// `/proc/self/task/*/comm`; elsewhere there is nothing to read and the
/// count is 0).
fn merge_thread_count() -> usize {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return 0;
    };
    tasks
        .filter_map(Result::ok)
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|name| name.starts_with("merge_thread"))
        })
        .count()
}

fn current_process_status() -> ProcessStatus {
    ProcessStatus {
        pid: std::process::id(),
        rss_kib: rss_kib(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The merge-thread reading sees a live writer's pool. Other tests may
    /// hold writers of their own at the same time, so the check is on this
    /// writer's contribution, not on an absolute.
    #[test]
    #[cfg(target_os = "linux")]
    fn merge_thread_count_sees_a_live_writer() {
        let dir = tempfile::tempdir().unwrap();
        let index = tantivy::Index::create_in_dir(
            dir.path(),
            rmc_engine::schema::ChunkSchema::new().schema(),
        )
        .unwrap();
        let before = merge_thread_count();
        let writer: tantivy::IndexWriter = index.writer(15_000_000).unwrap();
        let with_writer = merge_thread_count();
        writer.wait_merging_threads().unwrap();
        assert!(with_writer > before, "{before} -> {with_writer}");
    }

    #[tokio::test]
    async fn runtime_status_reports_owned_empty_state() {
        let runtime = ServerRuntime::new(3600);
        let status = runtime.state().status().await;

        assert!(status.sync.enabled);
        assert_eq!(status.sync.tracked_count, 0);
        assert_eq!(status.search_cache.entry_count, 0);
        assert_eq!(status.semantic.project_count, 0);
        assert!(!status.background_sync.enabled);
        assert!(!status.background_sync.running);
        assert_eq!(status.process.pid, std::process::id());
    }

    #[tokio::test]
    async fn runtime_clear_workspace_clears_semantic_and_sync_tracking() {
        let runtime = ServerRuntime::new(3600);
        let state = runtime.state();
        let workspace = tempfile::tempdir().expect("create temp workspace");

        state
            .sync_manager()
            .expect("runtime has sync manager")
            .track_directory(workspace.path().join("."))
            .await;
        state
            .semantic()
            .insert_test_project_fast(workspace.path().join("."));

        let report = state
            .clear(RuntimeClearRequest {
                scope: RuntimeClearScope::Workspace,
                workspace: Some(workspace.path().to_path_buf()),
            })
            .await;

        assert_eq!(report.semantic_projects_cleared, 1);
        assert_eq!(report.sync_directories_untracked, 1);

        let status = state.status().await;
        assert_eq!(status.semantic.project_count, 0);
        assert_eq!(status.sync.tracked_count, 0);
    }

    /// `clear_runtime` of one workspace clears what has locks of its own and
    /// does not wait for the workspace's readers — before 2026-10-31 it took the
    /// operation lock for write, waited for the longest `search` in flight and
    /// parked every new one behind it. Mutant: the write lock back → timeout.
    #[tokio::test]
    async fn runtime_clear_of_a_workspace_does_not_wait_for_its_readers() {
        let runtime = ServerRuntime::new(3600);
        let state = runtime.state();
        let workspace = tempfile::tempdir().expect("create temp workspace");

        let _reader = state.workspace_locks().lock_shared(workspace.path()).await;
        let report = tokio::time::timeout(
            Duration::from_secs(5),
            state.clear(RuntimeClearRequest {
                scope: RuntimeClearScope::Workspace,
                workspace: Some(workspace.path().to_path_buf()),
            }),
        )
        .await
        .expect("clear_runtime of a workspace waited for a reader of it");
        assert_eq!(report.scope, RuntimeClearScope::Workspace);
    }

    #[tokio::test]
    async fn runtime_clear_all_waits_for_workspace_operations() {
        let runtime = ServerRuntime::new(3600);
        let state = runtime.state();
        let workspace = tempfile::tempdir().expect("create temp workspace");

        let guard = state
            .workspace_locks()
            .lock_index_writer(workspace.path())
            .await;
        let clear_state = state.clone();
        let clear_task = tokio::spawn(async move {
            clear_state
                .clear(RuntimeClearRequest {
                    scope: RuntimeClearScope::All,
                    workspace: None,
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !clear_task.is_finished(),
            "global runtime clear should wait for in-flight workspace operations"
        );

        drop(guard);
        let report = tokio::time::timeout(Duration::from_secs(1), clear_task)
            .await
            .expect("runtime clear should complete after workspace operation finishes")
            .expect("clear task should not panic");
        assert_eq!(report.scope, RuntimeClearScope::All);
    }

    #[tokio::test]
    async fn runtime_background_sync_shutdown_stops_task() {
        let runtime = ServerRuntime::new(3600);
        runtime.start_background_sync();

        let running_status = runtime.state().status().await;
        assert!(running_status.background_sync.enabled);
        assert!(running_status.background_sync.running);

        let report = runtime.shutdown_gracefully(Duration::from_secs(1)).await;

        assert_eq!(report.tasks_total, 1);
        assert_eq!(report.tasks_completed, 1);
        assert!(!report.timed_out);

        let stopped_status = runtime.state().status().await;
        assert!(!stopped_status.background_sync.running);
    }

    /// The defect this closes: `clear` used to report only its own bookkeeping,
    /// so "one project cleared" and "not one byte returned to the kernel" — the
    /// normal outcome on glibc — were indistinguishable in the answer.
    #[tokio::test]
    async fn runtime_clear_reports_a_real_memory_measurement() {
        let runtime = ServerRuntime::new(3600);

        let report = runtime
            .state()
            .clear(RuntimeClearRequest {
                scope: RuntimeClearScope::SemanticOnly,
                workspace: None,
            })
            .await;

        assert!(
            report.memory.attempted,
            "a glibc or mimalloc build must actually attempt the release"
        );
        #[cfg(target_os = "linux")]
        assert!(
            report.memory.rss_kib_before.is_some_and(|rss| rss > 0),
            "RSS must be read, not guessed: {:?}",
            report.memory
        );
    }
}
