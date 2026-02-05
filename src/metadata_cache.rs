//! File metadata cache for incremental indexing
//!
//! Tracks file hashes and metadata to determine which files have changed
//! and need to be reindexed. Uses sled embedded database for persistence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sled::Db;
use std::path::Path;
use std::time::SystemTime;

/// Metadata for a single indexed file
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMetadata {
    /// SHA-256 hash of file content
    pub hash: String,

    /// Unix timestamp of last modification
    pub last_modified: u64,

    /// File size in bytes
    pub size: u64,

    /// Unix timestamp when we indexed the file
    pub indexed_at: u64,
}

impl FileMetadata {
    /// Create new metadata from file content
    pub fn from_content(content: &str, last_modified: u64, size: u64) -> Self {
        Self {
            hash: Self::hash_content(content),
            last_modified,
            size,
            indexed_at: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    /// Calculate SHA-256 hash of content
    fn hash_content(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// Cache for tracking file metadata
pub struct MetadataCache {
    db: Db,
}

impl MetadataCache {
    /// Open or create a metadata cache at the given path
    pub fn new(path: &Path) -> Result<Self, sled::Error> {
        // Ensure parent directories exist (sled only creates the final directory)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let db = sled::open(path)?;
        Ok(Self { db })
    }

    /// Get cached metadata for a file
    pub fn get(&self, file_path: &str) -> Result<Option<FileMetadata>, Box<dyn std::error::Error>> {
        match self.db.get(file_path)? {
            Some(bytes) => {
                let metadata: FileMetadata = bincode::deserialize(&bytes)?;
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Store metadata for a file
    pub fn set(&self, file_path: &str, metadata: &FileMetadata) -> Result<(), Box<dyn std::error::Error>> {
        let bytes = bincode::serialize(metadata)?;
        self.db.insert(file_path, bytes)?;
        Ok(())
    }

    /// Remove metadata for a file (e.g., when file is deleted)
    pub fn remove(&self, file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.db.remove(file_path)?;
        Ok(())
    }

    /// Check if a file has changed since last indexing
    ///
    /// Returns true if:
    /// - File is not in cache (never indexed)
    /// - Content hash differs from cached hash
    pub fn has_changed(&self, file_path: &str, content: &str) -> Result<bool, Box<dyn std::error::Error>> {
        let current_hash = FileMetadata::hash_content(content);

        match self.get(file_path)? {
            Some(cached) => Ok(cached.hash != current_hash),
            None => Ok(true), // Not in cache = needs indexing
        }
    }

    /// Get all cached file paths
    pub fn list_files(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut files = Vec::new();
        for item in self.db.iter() {
            let (key, _) = item?;
            let path = String::from_utf8(key.to_vec())?;
            files.push(path);
        }
        Ok(files)
    }

    /// Clear all cached metadata (useful for re-indexing from scratch)
    pub fn clear(&self) -> Result<(), sled::Error> {
        self.db.clear()
    }

    /// Get total number of cached files
    pub fn len(&self) -> usize {
        self.db.len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_metadata_creation() {
        let content = "test content";
        let metadata = FileMetadata::from_content(content, 12345, 100);

        assert_eq!(metadata.last_modified, 12345);
        assert_eq!(metadata.size, 100);
        assert!(!metadata.hash.is_empty());
        assert!(metadata.indexed_at > 0);
    }

    #[test]
    fn test_hash_consistency() {
        let content = "same content";
        let hash1 = FileMetadata::hash_content(content);
        let hash2 = FileMetadata::hash_content(content);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_uniqueness() {
        let hash1 = FileMetadata::hash_content("content1");
        let hash2 = FileMetadata::hash_content("content2");

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_metadata_cache_new() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let cache_path = temp_dir.path().join("cache");

        let cache = MetadataCache::new(&cache_path)?;
        assert!(cache.is_empty());

        Ok(())
    }

    #[test]
    fn test_metadata_cache_set_get() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let cache_path = temp_dir.path().join("cache");
        let cache = MetadataCache::new(&cache_path)?;

        let metadata = FileMetadata::from_content("test", 123, 10);
        cache.set("test.txt", &metadata)?;

        let retrieved = cache.get("test.txt")?.unwrap();
        assert_eq!(retrieved, metadata);

        Ok(())
    }

    #[test]
    fn test_metadata_cache_has_changed() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let cache_path = temp_dir.path().join("cache");
        let cache = MetadataCache::new(&cache_path)?;

        let content1 = "original content";
        let content2 = "modified content";

        // File not in cache yet
        assert!(cache.has_changed("test.txt", content1)?);

        // Add to cache
        let metadata = FileMetadata::from_content(content1, 123, 10);
        cache.set("test.txt", &metadata)?;

        // Same content - no change
        assert!(!cache.has_changed("test.txt", content1)?);

        // Different content - has changed
        assert!(cache.has_changed("test.txt", content2)?);

        Ok(())
    }

    #[test]
    fn test_metadata_cache_remove() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let cache_path = temp_dir.path().join("cache");
        let cache = MetadataCache::new(&cache_path)?;

        let metadata = FileMetadata::from_content("test", 123, 10);
        cache.set("test.txt", &metadata)?;

        assert!(cache.get("test.txt")?.is_some());

        cache.remove("test.txt")?;

        assert!(cache.get("test.txt")?.is_none());

        Ok(())
    }

    #[test]
    fn test_metadata_cache_persistence() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let cache_path = temp_dir.path().join("cache");

        // Create cache and add data
        {
            let cache = MetadataCache::new(&cache_path)?;
            let metadata = FileMetadata::from_content("test", 123, 10);
            cache.set("test.txt", &metadata)?;
        }

        // Reopen cache and verify data persists
        {
            let cache = MetadataCache::new(&cache_path)?;
            let retrieved = cache.get("test.txt")?.unwrap();
            assert_eq!(retrieved.hash, FileMetadata::hash_content("test"));
        }

        Ok(())
    }
}
