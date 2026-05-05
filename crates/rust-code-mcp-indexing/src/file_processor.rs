//! File processing, security filtering, and metadata cache operations
//!
//! Extracted from `IndexerCore` to encapsulate file-level concerns:
//! sensitive file filtering, secrets scanning, size checks, and
//! change detection via the metadata cache.

use crate::metadata_cache::MetadataCache;
use crate::security::secrets::SecretsScanner;
use crate::security::SensitiveFileFilter;
use crate::IndexingError;
use std::path::Path;

/// Handles file filtering, security scanning, and change detection.
pub(crate) struct FileProcessor {
    /// Metadata cache for incremental change detection
    metadata_cache: MetadataCache,
    /// Secrets scanner for content-level security
    secrets_scanner: SecretsScanner,
    /// File filter for path-level security
    file_filter: SensitiveFileFilter,
    /// Maximum file size to process (bytes)
    max_file_size: u64,
}

impl FileProcessor {
    /// Create a new FileProcessor
    pub(crate) fn new(
        cache_path: &Path,
        max_file_size: u64,
    ) -> Result<Self, IndexingError> {
        let metadata_cache = MetadataCache::new(cache_path)
            .map_err(|e| IndexingError::Cache(e.to_string()))?;
        let secrets_scanner = SecretsScanner::new();
        let file_filter = SensitiveFileFilter::default();

        Ok(Self {
            metadata_cache,
            secrets_scanner,
            file_filter,
            max_file_size,
        })
    }

    /// Check if a file should be processed (security and size checks)
    pub(crate) fn should_process_file(&self, file_path: &Path) -> Result<bool, IndexingError> {
        // Check sensitive file filter
        if !self.file_filter.should_index(file_path) {
            tracing::warn!("Excluding sensitive file: {}", file_path.display());
            return Ok(false);
        }

        // Check file size
        let metadata = std::fs::metadata(file_path)?;

        if metadata.len() > self.max_file_size {
            tracing::warn!(
                "Skipping large file: {} ({:.2} MB exceeds {:.2} MB limit)",
                file_path.display(),
                metadata.len() as f64 / 1_000_000.0,
                self.max_file_size as f64 / 1_000_000.0
            );
            return Ok(false);
        }

        Ok(true)
    }

    /// Fast check if file has likely changed using only stat info (mtime + size).
    /// Avoids reading file content. Use as a pre-filter before `has_file_changed`.
    pub(crate) fn has_stat_changed(&self, file_path: &Path) -> Result<bool, IndexingError> {
        let file_path_str = file_path.to_string_lossy().to_string();
        let stat = crate::metadata_cache::FileStat::from_path(file_path)
            .map_err(|e| IndexingError::Cache(e.to_string()))?;
        self.metadata_cache.has_stat_changed(&file_path_str, &stat)
            .map_err(|e| IndexingError::Cache(e.to_string()))
    }

    /// Check if file has changed (using metadata cache, reads content hash)
    pub(crate) fn has_file_changed(&self, file_path: &Path, content: &str) -> Result<bool, IndexingError> {
        let file_path_str = file_path.to_string_lossy().to_string();
        self.metadata_cache.has_changed(&file_path_str, content)
            .map_err(|e| IndexingError::Cache(e.to_string()))
    }

    /// Update metadata cache for a file
    pub(crate) fn update_file_metadata(&self, file_path: &Path, content: &str) -> Result<(), IndexingError> {
        let file_path_str = file_path.to_string_lossy().to_string();
        let metadata = std::fs::metadata(file_path)?;
        let file_meta = crate::metadata_cache::FileMetadata::from_content(
            content,
            metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| IndexingError::Cache(e.to_string()))?
                .as_secs(),
            metadata.len(),
        );
        self.metadata_cache.set(&file_path_str, &file_meta)
            .map_err(|e| IndexingError::Cache(e.to_string()))
    }

    /// Check content for secrets; returns Err if secrets detected
    pub(crate) fn check_secrets(&self, file_path: &Path, content: &str) -> Result<(), IndexingError> {
        if self.secrets_scanner.should_exclude(content) {
            let summary = self.secrets_scanner.scan_summary(content);
            tracing::warn!(
                "Excluding file with secrets: {}\n{}",
                file_path.display(),
                summary
            );
            return Err(IndexingError::Parser("Contains secrets".into()));
        }
        Ok(())
    }

    /// Get reference to metadata cache
    pub(crate) fn metadata_cache(&self) -> &MetadataCache {
        &self.metadata_cache
    }

    /// Clear metadata cache
    pub(crate) fn clear_metadata_cache(&self) -> Result<(), IndexingError> {
        self.metadata_cache
            .clear()
            .map_err(|e| IndexingError::Cache(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_processor_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");

        let fp = FileProcessor::new(&cache_path, 10_000_000);
        assert!(fp.is_ok(), "Failed to create FileProcessor: {:?}", fp.err());
    }

    #[test]
    fn test_should_process_file() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let fp = FileProcessor::new(&cache_path, 10_000_000).unwrap();

        let test_file = temp_dir.path().join("test.rs");
        std::fs::write(&test_file, "fn test() {}").unwrap();

        let result = fp.should_process_file(&test_file);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_should_process_file_too_large() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let fp = FileProcessor::new(&cache_path, 10).unwrap(); // 10 byte limit

        let test_file = temp_dir.path().join("test.rs");
        std::fs::write(&test_file, "fn this_is_longer_than_ten_bytes() {}").unwrap();

        let result = fp.should_process_file(&test_file);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_check_secrets_clean() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let fp = FileProcessor::new(&cache_path, 10_000_000).unwrap();

        let test_file = temp_dir.path().join("test.rs");
        let result = fp.check_secrets(&test_file, "fn normal_code() {}");
        assert!(result.is_ok());
    }

    #[test]
    fn test_clear_metadata_cache() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let fp = FileProcessor::new(&cache_path, 10_000_000).unwrap();

        let result = fp.clear_metadata_cache();
        assert!(result.is_ok());
    }
}
