use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::schemars;
use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::semantic::{SemanticService, SemanticServiceStatus};

use super::{
    SearchRuntimeCache, SearchRuntimeCacheStatus, SyncManager, SyncManagerStatus,
    WorkspaceLockRegistry,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeStatus {
    pub sync: RuntimeSyncStatus,
    pub search_cache: SearchRuntimeCacheStatus,
    pub semantic: SemanticServiceStatus,
    pub background_sync: BackgroundSyncStatus,
    pub process: ProcessStatus,
}

#[derive(Clone)]
pub struct RuntimeState {
    sync_manager: Option<Arc<SyncManager>>,
    workspace_locks: WorkspaceLockRegistry,
    search_cache: SearchRuntimeCache,
    semantic: Arc<Mutex<SemanticService>>,
    background_sync_enabled: Arc<AtomicBool>,
    background_sync_running: Arc<AtomicBool>,
}

impl RuntimeState {
    pub fn standalone() -> Self {
        Self {
            sync_manager: None,
            workspace_locks: WorkspaceLockRegistry::new(),
            search_cache: SearchRuntimeCache::new(),
            semantic: Arc::new(Mutex::new(SemanticService::new())),
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
            semantic: Arc::new(Mutex::new(SemanticService::new())),
            background_sync_enabled: Arc::new(AtomicBool::new(false)),
            background_sync_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn sync_manager(&self) -> Option<&Arc<SyncManager>> {
        self.sync_manager.as_ref()
    }

    pub fn workspace_locks(&self) -> &WorkspaceLockRegistry {
        &self.workspace_locks
    }

    pub fn search_cache(&self) -> &SearchRuntimeCache {
        &self.search_cache
    }

    pub(crate) fn semantic(&self) -> Arc<Mutex<SemanticService>> {
        Arc::clone(&self.semantic)
    }

    pub fn background_sync_enabled(&self) -> bool {
        self.background_sync_enabled.load(Ordering::SeqCst)
    }

    pub fn background_sync_running(&self) -> bool {
        self.background_sync_running.load(Ordering::SeqCst)
    }

    pub async fn status(&self) -> RuntimeStatus {
        let sync = match &self.sync_manager {
            Some(sync_manager) => RuntimeSyncStatus::from_sync_manager(sync_manager.status().await),
            None => RuntimeSyncStatus::disabled(),
        };
        let semantic = self
            .semantic
            .lock()
            .expect("semantic service mutex poisoned")
            .status();
        RuntimeStatus {
            sync,
            search_cache: self.search_cache.status(),
            semantic,
            background_sync: BackgroundSyncStatus {
                enabled: self.background_sync_enabled(),
                running: self.background_sync_running(),
            },
            process: current_process_status(),
        }
    }

    pub async fn clear(&self, request: RuntimeClearRequest) -> RuntimeClearReport {
        let workspace = request
            .workspace
            .as_deref()
            .map(normalize_workspace);
        if let Some(workspace) = &workspace {
            let _workspace_lock = self.workspace_locks.lock_exclusive(workspace).await;
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
        };

        match (scope, workspace) {
            (RuntimeClearScope::All, Some(workspace))
            | (RuntimeClearScope::Workspace, Some(workspace)) => {
                report.search_cache_entries_cleared =
                    self.search_cache.invalidate_workspace(workspace);
                report.semantic_projects_cleared = self
                    .semantic
                    .lock()
                    .expect("semantic service mutex poisoned")
                    .clear_project(workspace);
                report.sync_directories_untracked = self.untrack_workspace(workspace).await;
            }
            (RuntimeClearScope::All, None) => {
                report.search_cache_entries_cleared = self.search_cache.invalidate_all();
                report.semantic_projects_cleared = self
                    .semantic
                    .lock()
                    .expect("semantic service mutex poisoned")
                    .clear_all();
                report.sync_directories_untracked = self.untrack_all().await;
            }
            (RuntimeClearScope::Workspace, None) => {}
            (RuntimeClearScope::SemanticOnly, Some(workspace)) => {
                report.semantic_projects_cleared = self
                    .semantic
                    .lock()
                    .expect("semantic service mutex poisoned")
                    .clear_project(workspace);
            }
            (RuntimeClearScope::SemanticOnly, None) => {
                report.semantic_projects_cleared = self
                    .semantic
                    .lock()
                    .expect("semantic service mutex poisoned")
                    .clear_all();
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

fn current_process_status() -> ProcessStatus {
    ProcessStatus {
        pid: std::process::id(),
        rss_kib: current_rss_kib(),
    }
}

fn current_rss_kib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| parse_proc_status_rss_kib(&status))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn parse_proc_status_rss_kib(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?;
        value.split_whitespace().next()?.parse::<u64>().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .lock()
            .expect("semantic mutex")
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

    #[tokio::test]
    async fn runtime_clear_all_waits_for_workspace_operations() {
        let runtime = ServerRuntime::new(3600);
        let state = runtime.state();
        let workspace = tempfile::tempdir().expect("create temp workspace");

        let guard = state.workspace_locks().lock_exclusive(workspace.path()).await;
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

    #[test]
    fn runtime_proc_status_parser_reads_rss() {
        let status = "Name:\ttest\nVmRSS:\t  12345 kB\n";

        assert_eq!(parse_proc_status_rss_kib(status), Some(12345));
    }
}
