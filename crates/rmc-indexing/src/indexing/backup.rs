//! Backup trait — indexing-side seam so `monitoring::backup::BackupManager`
//! can be passed into `UnifiedIndexer` without `indexing` depending on `monitoring`.
//!
//! `monitoring` implements this trait for `BackupManager`; `indexing` only knows
//! about the trait.

use std::path::PathBuf;

use crate::indexing::merkle::FileSystemMerkle;

pub(crate) trait Backup {
    fn create_backup(&self, merkle: &FileSystemMerkle) -> anyhow::Result<PathBuf>;
}
