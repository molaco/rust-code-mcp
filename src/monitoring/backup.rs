//! Backup and restore functionality for Merkle tree snapshots
//!
//! Provides automatic backup rotation with configurable retention

use crate::indexing::merkle::FileSystemMerkle;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing;

/// Manages backups of Merkle tree snapshots
pub struct BackupManager {
    backup_dir: PathBuf,
    retention_count: usize,
}

impl BackupManager {
    /// Create a new backup manager
    ///
    /// # Arguments
    /// * `backup_dir` - Directory to store backups
    /// * `retention_count` - Number of backups to keep (default: 7)
    pub fn new(backup_dir: PathBuf, retention_count: usize) -> Result<Self> {
        std::fs::create_dir_all(&backup_dir)
            .context(format!("Failed to create backup directory: {}", backup_dir.display()))?;

        Ok(Self {
            backup_dir,
            retention_count,
        })
    }

    /// Create a backup of the Merkle snapshot
    ///
    /// Returns the path to the created backup file.
    /// Automatically rotates old backups according to retention policy.
    pub fn create_backup(&self, merkle: &FileSystemMerkle) -> Result<PathBuf> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("Failed to get system time")?
            .as_secs();

        let backup_path = self.backup_dir.join(format!(
            "merkle_v{}.{}.snapshot",
            merkle.version(),
            timestamp
        ));

        merkle
            .save_snapshot(&backup_path)
            .context(format!("Failed to save backup to {}", backup_path.display()))?;

        // Rotate old backups
        self.rotate_backups()
            .context("Failed to rotate old backups")?;

        tracing::info!("Created backup: {}", backup_path.display());

        Ok(backup_path)
    }

    /// Restore from the latest backup
    ///
    /// Returns `Ok(Some(merkle))` if a backup was found and restored,
    /// `Ok(None)` if no backups exist.
    pub fn restore_latest(&self) -> Result<Option<FileSystemMerkle>> {
        let mut backups = self.list_backups()?;

        if backups.is_empty() {
            tracing::info!("No backups found in {}", self.backup_dir.display());
            return Ok(None);
        }

        // Sort by modification time (newest first)
        backups.sort_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        backups.reverse();

        let latest = backups
            .first()
            .context("No backups available")?
            .path();

        tracing::info!("Restoring from backup: {}", latest.display());

        FileSystemMerkle::load_snapshot(&latest)
            .context(format!("Failed to load snapshot from {}", latest.display()))
    }

    /// List all backup files
    pub fn list_backups(&self) -> Result<Vec<std::fs::DirEntry>> {
        let backups: Vec<_> = std::fs::read_dir(&self.backup_dir)
            .context(format!("Failed to read backup directory: {}", self.backup_dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    == Some("snapshot")
            })
            .collect();

        Ok(backups)
    }

    /// Remove old backups to maintain retention policy
    fn rotate_backups(&self) -> Result<()> {
        let mut backups = self.list_backups()?;

        if backups.len() <= self.retention_count {
            return Ok(());
        }

        // Sort by modification time (oldest first)
        backups.sort_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        // Remove oldest backups
        let to_remove = backups.len() - self.retention_count;
        for backup in backups.iter().take(to_remove) {
            let path = backup.path();
            std::fs::remove_file(&path)
                .context(format!("Failed to remove old backup: {}", path.display()))?;
            tracing::info!("Deleted old backup: {}", path.display());
        }

        Ok(())
    }

    /// Get the backup directory path
    pub fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    /// Get the retention count
    pub fn retention_count(&self) -> usize {
        self.retention_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_backup_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");

        let manager = BackupManager::new(backup_dir.clone(), 7).unwrap();

        assert_eq!(manager.retention_count(), 7);
        assert_eq!(manager.backup_dir(), backup_dir.as_path());
        assert!(backup_dir.exists());
    }

    #[test]
    fn test_list_backups_empty() {
        let temp_dir = TempDir::new().unwrap();
        let manager = BackupManager::new(temp_dir.path().to_path_buf(), 7).unwrap();

        let backups = manager.list_backups().unwrap();
        assert_eq!(backups.len(), 0);
    }

    #[test]
    fn test_restore_latest_when_empty() {
        let temp_dir = TempDir::new().unwrap();
        let manager = BackupManager::new(temp_dir.path().to_path_buf(), 7).unwrap();

        let result = manager.restore_latest().unwrap();
        assert!(result.is_none());
    }
}
