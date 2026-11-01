//! File metadata cache for incremental indexing
//!
//! Tracks file hashes and metadata to determine which files have changed
//! and need to be reindexed. Uses sled embedded database for persistence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sled::Db;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// How long [`MetadataCache::new`] waits out a lock still held by a handle
/// that is on its way out, and how often it retries inside that budget.
///
/// Sized to cover thread scheduling, not to outwait a working indexer: a
/// cache that is genuinely busy stays an error, so the caller can report it
/// instead of stalling behind someone else's write.
const LOCK_WAIT_BUDGET: Duration = Duration::from_millis(250);
const LOCK_WAIT_STEP: Duration = Duration::from_millis(10);

/// Whether a failed open is lock contention rather than a broken cache.
///
/// sled flattens this into an `Io` error whose kind is `Other`, so the
/// underlying `WouldBlock` survives only in the message — matching the text
/// is all that is left. Anything else (corruption, permissions, no space) must
/// keep propagating immediately: retrying those just delays the real report.
fn is_lock_contention(error: &sled::Error) -> bool {
    matches!(error, sled::Error::Io(io) if io.to_string().contains("could not acquire lock"))
}

/// Metadata for a single indexed file
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct FileMetadata {
    /// SHA-256 hash of file content
    pub hash: String,

    /// Unix timestamp of last modification
    pub last_modified: u64,

    /// File size in bytes
    pub size: u64,

    /// Unix timestamp when we indexed the file
    pub indexed_at: u64,
}

/// Lightweight stat info for fast change detection (avoids reading file content)
#[derive(Debug, Clone)]
pub(crate) struct FileStat {
    pub last_modified: u64,
    pub size: u64,
}

impl FileStat {
    /// Read stat info from filesystem (cheap: no file content read)
    pub(crate) fn from_path(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            last_modified: metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            size: metadata.len(),
        })
    }
}

impl FileMetadata {
    /// Create new metadata from file content
    pub(crate) fn from_content(content: &str, last_modified: u64, size: u64) -> Self {
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
pub(crate) struct MetadataCache {
    db: Db,
}

impl MetadataCache {
    /// Open or create a metadata cache at the given path
    ///
    /// sled holds an exclusive file lock, and it outlives the `Db` handle by a
    /// moment: background threads still hold the file when `drop` returns. So
    /// an ordinary open-write-close-reopen sequence can lose the lock race on
    /// its own previous handle, and the wider the machine is loaded the more
    /// often it does — this surfaced as a flaky coverage probe, failing on a
    /// different test each parallel run with `WouldBlock` on the reopen.
    ///
    /// A bounded retry closes that window without inventing a queue: past
    /// [`LOCK_WAIT_BUDGET`] the error is returned, and a health probe reports
    /// "unknown" rather than fighting a running indexer for the lock.
    pub(crate) fn new(path: &Path) -> Result<Self, sled::Error> {
        // Ensure parent directories exist (sled only creates the final directory)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let mut waited = Duration::ZERO;
        loop {
            match sled::open(path) {
                Ok(db) => return Ok(Self { db }),
                Err(e) if is_lock_contention(&e) && waited < LOCK_WAIT_BUDGET => {
                    std::thread::sleep(LOCK_WAIT_STEP);
                    waited += LOCK_WAIT_STEP;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Get cached metadata for a file
    pub(crate) fn get(&self, file_path: &str) -> Result<Option<FileMetadata>, Box<dyn std::error::Error>> {
        match self.db.get(file_path)? {
            Some(bytes) => {
                let metadata: FileMetadata = bincode::deserialize(&bytes)?;
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Store metadata for a file
    pub(crate) fn set(&self, file_path: &str, metadata: &FileMetadata) -> Result<(), Box<dyn std::error::Error>> {
        let bytes = bincode::serialize(metadata)?;
        self.db.insert(file_path, bytes)?;
        Ok(())
    }

    /// Remove metadata for a file (e.g., when file is deleted)
    pub(crate) fn remove(&self, file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.db.remove(file_path)?;
        Ok(())
    }

    /// Fast check if a file has likely changed using only stat info (mtime + size).
    ///
    /// This avoids reading file content entirely. Returns:
    /// - `true` if file is not cached or stat differs (may need content hash to confirm)
    /// - `false` if stat matches (file almost certainly unchanged)
    pub(crate) fn has_stat_changed(&self, file_path: &str, stat: &FileStat) -> Result<bool, Box<dyn std::error::Error>> {
        match self.get(file_path)? {
            Some(cached) => Ok(cached.last_modified != stat.last_modified || cached.size != stat.size),
            None => Ok(true), // Not in cache = needs indexing
        }
    }

    /// Check if a file has changed since last indexing
    ///
    /// Returns true if:
    /// - File is not in cache (never indexed)
    /// - Content hash differs from cached hash
    pub(crate) fn has_changed(&self, file_path: &str, content: &str) -> Result<bool, Box<dyn std::error::Error>> {
        let current_hash = FileMetadata::hash_content(content);

        match self.get(file_path)? {
            Some(cached) => Ok(cached.hash != current_hash),
            None => Ok(true), // Not in cache = needs indexing
        }
    }

    /// Get all cached file paths
    pub(crate) fn list_files(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut files = Vec::new();
        for item in self.db.iter() {
            let (key, _) = item?;
            let path = String::from_utf8(key.to_vec())?;
            files.push(path);
        }
        Ok(files)
    }

    /// Clear all cached metadata (useful for re-indexing from scratch)
    pub(crate) fn clear(&self) -> Result<(), sled::Error> {
        self.db.clear()
    }

    /// Cached file paths belonging to one salt, with the salt prefix stripped.
    ///
    /// Cache keys are `{salt}::{path}` (see `FileProcessor::cache_key`), where
    /// the salt is the embedder+chunking identity. One cache therefore holds
    /// keys for several identities at once, and any caller reasoning about
    /// "which files are done" must speak about a single salt — otherwise a
    /// neighbouring profile's keys leak into the answer.
    pub(crate) fn paths_for_salt(keys: Vec<String>, salt: &str) -> HashSet<String> {
        if salt.is_empty() {
            return keys.into_iter().collect();
        }
        let prefix = format!("{}::", salt);
        keys.into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }

    /// Get total number of cached files
    pub(crate) fn len(&self) -> usize {
        self.db.len()
    }

    /// Check if cache is empty
    pub(crate) fn is_empty(&self) -> bool {
        self.db.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A cache held open by someone else must still FAIL to open, and inside
    /// the wait budget rather than whenever the OS feels like it. Without this
    /// half of the check the retry could quietly become an unbounded wait, and
    /// a health probe would hang behind a running indexer instead of saying
    /// "unknown".
    #[test]
    fn a_cache_held_by_a_live_handle_fails_within_the_wait_budget() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cache");
        let holder = MetadataCache::new(&path).expect("first open");

        let started = std::time::Instant::now();
        let contended = MetadataCache::new(&path);
        let waited = started.elapsed();

        let error = contended.err().expect("second open must fail while held");
        assert!(
            is_lock_contention(&error),
            "expected lock contention, got: {error:?}"
        );
        assert!(
            waited < LOCK_WAIT_BUDGET * 4,
            "gave up after {waited:?}, budget is {LOCK_WAIT_BUDGET:?}"
        );

        // Positive control: the same path opens once the holder is gone, so
        // the assertion above is about the lock and not about a bad path.
        drop(holder);
        MetadataCache::new(&path).expect("reopen after release");
    }

    /// The retry must not swallow real failures: a path that cannot be a
    /// database has to come back as an error immediately, not after the
    /// budget.
    #[test]
    fn a_broken_cache_path_is_not_mistaken_for_lock_contention() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("not-a-dir");
        std::fs::write(&path, b"i am a file, not a sled directory").unwrap();

        let error = MetadataCache::new(&path).err().expect("must fail");
        assert!(
            !is_lock_contention(&error),
            "a broken path must not be retried as contention: {error:?}"
        );
    }

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

    #[test]
    fn paths_for_salt_keeps_only_its_own_salt() {
        let keys = vec![
            "saltA::/repo/a.rs".to_string(),
            "saltB::/repo/b.rs".to_string(),
            "saltA::/repo/c.rs".to_string(),
            "/repo/unsalted.rs".to_string(),
        ];

        let mine = MetadataCache::paths_for_salt(keys, "saltA");

        assert_eq!(mine.len(), 2);
        assert!(mine.contains("/repo/a.rs"));
        assert!(mine.contains("/repo/c.rs"));
        // A neighbouring identity's entry says nothing about this index, and
        // an unsalted key belongs to no identity at all.
        assert!(!mine.contains("/repo/b.rs"));
        assert!(!mine.contains("/repo/unsalted.rs"));
    }

    #[test]
    fn paths_for_salt_without_salt_takes_every_key_verbatim() {
        let keys = vec!["/repo/a.rs".to_string(), "saltA::/repo/b.rs".to_string()];

        let all = MetadataCache::paths_for_salt(keys, "");

        assert_eq!(all.len(), 2);
        assert!(all.contains("/repo/a.rs"));
        assert!(all.contains("saltA::/repo/b.rs"));
    }
}
