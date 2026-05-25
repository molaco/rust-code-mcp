//! Per-workspace operation locks for index/cache state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

fn workspace_key(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// Registry of async locks keyed by canonical workspace directory.
#[derive(Clone, Default)]
pub struct WorkspaceLockRegistry {
    locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl WorkspaceLockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    async fn lock_for(&self, dir: &Path) -> (PathBuf, Arc<Mutex<()>>) {
        let key = workspace_key(dir);
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        (key, lock)
    }

    /// Take an exclusive workspace lock.
    pub async fn lock_exclusive(&self, dir: &Path) -> WorkspaceLockGuard {
        let (workspace, lock) = self.lock_for(dir).await;
        let guard = lock.lock_owned().await;
        WorkspaceLockGuard {
            workspace,
            _guard: guard,
        }
    }

    /// Take a read-side workspace lock.
    ///
    /// This is intentionally backed by the same mutex as exclusive locks. It
    /// keeps the first implementation conservative while preserving a call
    /// shape that can move to a real read/write lock later.
    pub async fn lock_shared(&self, dir: &Path) -> WorkspaceLockGuard {
        self.lock_exclusive(dir).await
    }
}

/// Held workspace operation lock.
pub struct WorkspaceLockGuard {
    workspace: PathBuf,
    _guard: OwnedMutexGuard<()>,
}

impl WorkspaceLockGuard {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}
