# Phase 1: Persistent Index + Incremental Updates

**Timeline:** Weeks 2-4 (3 weeks)
**Started:** 2025-10-17
**Status:** Ready to Begin

---

## 🎯 Phase 1 Goals

Transform the in-memory, rebuild-every-time search into a **persistent, incremental system**:

1. **Persistent Tantivy Index**: Store on disk, survive restarts
2. **File Metadata Cache**: Track file hashes to detect changes
3. **Incremental Updates**: Only reindex changed files
4. **File Watching**: Automatically detect file system changes
5. **Configuration**: User-configurable index location

---

## 📋 Implementation Steps

### Step 1: Update Dependencies (30 min)

Add to `Cargo.toml`:
```toml
[dependencies]
# Existing dependencies...
notify = "6"           # File watching
sled = "0.34"          # Embedded key-value store for metadata
sha2 = "0.10"          # SHA-256 file hashing
directories = "5"      # Cross-platform config/data directories
```

**Test:**
```bash
cargo build
# Should compile without errors
```

---

### Step 2: Create Schema Module (2-3 hours)

**File:** `src/schema.rs`

Based on bloop's `indexes/schema.rs`, create enhanced schema:

```rust
use tantivy::schema::{Field, Schema, SchemaBuilder, TextOptions, STORED};

pub struct FileSchema {
    pub schema: Schema,

    // Fields
    pub unique_hash: Field,      // SHA-256 of file content
    pub relative_path: Field,    // Path relative to indexed directory
    pub content: Field,          // File content (indexed + stored)
    pub last_modified: Field,    // Unix timestamp
    pub file_size: Field,        // Size in bytes
}

impl FileSchema {
    pub fn new() -> Self {
        let mut builder = SchemaBuilder::new();

        let text_options = TextOptions::default()
            .set_indexing_options(/* ... */)
            .set_stored();

        let unique_hash = builder.add_text_field("unique_hash", STORED);
        let relative_path = builder.add_text_field("relative_path", text_options.clone());
        let content = builder.add_text_field("content", text_options);
        let last_modified = builder.add_u64_field("last_modified", STORED);
        let file_size = builder.add_u64_field("file_size", STORED);

        Self {
            schema: builder.build(),
            unique_hash,
            relative_path,
            content,
            last_modified,
            file_size,
        }
    }

    pub fn schema(&self) -> Schema {
        self.schema.clone()
    }
}
```

**Test:**
```bash
cargo test --lib schema
```

---

### Step 3: Create Metadata Cache (3-4 hours)

**File:** `src/metadata_cache.rs`

Track indexed files with sled database:

```rust
use sled::Db;
use sha2::{Sha256, Digest};
use std::path::{Path, PathBuf};

pub struct MetadataCache {
    db: Db,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct FileMetadata {
    pub hash: String,           // SHA-256 of content
    pub last_modified: u64,     // Unix timestamp
    pub size: u64,              // File size in bytes
    pub indexed_at: u64,        // When we indexed it
}

impl MetadataCache {
    pub fn new(path: &Path) -> Result<Self, sled::Error> {
        let db = sled::open(path.join("metadata"))?;
        Ok(Self { db })
    }

    /// Get cached metadata for a file
    pub fn get(&self, path: &str) -> Option<FileMetadata> {
        self.db.get(path).ok()?
            .and_then(|v| bincode::deserialize(&v).ok())
    }

    /// Store metadata for a file
    pub fn set(&self, path: &str, metadata: FileMetadata) -> Result<(), Box<dyn std::error::Error>> {
        let bytes = bincode::serialize(&metadata)?;
        self.db.insert(path, bytes)?;
        Ok(())
    }

    /// Check if file has changed since last index
    pub fn has_changed(&self, path: &Path, content: &str) -> bool {
        let current_hash = Self::hash_content(content);

        match self.get(path.to_str().unwrap()) {
            Some(meta) => meta.hash != current_hash,
            None => true, // Not indexed yet
        }
    }

    /// Calculate SHA-256 hash of content
    fn hash_content(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}
```

**Key Features:**
- Stores file hash, timestamps, size
- Fast lookups to check if file changed
- Persisted to disk with sled

**Test:**
```bash
cargo test --lib metadata_cache
```

---

### Step 4: Make Index Persistent (2-3 hours)

**File:** Modify `src/tools/search_tool.rs`

**Current (line 123):**
```rust
let index = Index::create_in_ram(schema.clone());
```

**New approach:**
```rust
use directories::ProjectDirs;
use std::path::PathBuf;

impl SearchTool {
    fn index_path() -> PathBuf {
        // Get XDG-compliant data directory
        let project_dirs = ProjectDirs::from("dev", "rust-code-mcp", "search")
            .expect("Could not determine data directory");
        project_dirs.data_dir().to_path_buf()
    }

    fn open_or_create_index(&self) -> Result<Index, Box<dyn std::error::Error>> {
        let index_path = Self::index_path().join("tantivy");
        let schema = FileSchema::new();

        std::fs::create_dir_all(&index_path)?;

        if index_path.join("meta.json").exists() {
            // Open existing index
            Index::open_in_dir(&index_path)
        } else {
            // Create new index
            Index::create_in_dir(&index_path, schema.schema())
        }
    }
}
```

**Changes to `search()` method:**
1. Replace `Index::create_in_ram()` with `self.open_or_create_index()`
2. Check metadata cache before indexing each file
3. Only index files that have changed

**Test:**
```bash
# Build and run
cargo run

# Index should persist at:
# Linux: ~/.local/share/rust-code-mcp/search/tantivy/
# macOS: ~/Library/Application Support/rust-code-mcp/search/tantivy/
# Windows: %APPDATA%\rust-code-mcp\search\tantivy\
```

---

### Step 5: Integrate Metadata Cache with Indexing (2 hours)

**Modify:** `src/tools/search_tool.rs` - `process_directory()` function

**Before indexing a file:**
```rust
fn process_directory(
    dir_path: &Path,
    index_writer: &mut IndexWriter,
    schema: &FileSchema,
    cache: &MetadataCache,
    // ... other params
) -> Result<(), String> {
    for entry in fs::read_dir(dir_path)? {
        let path = entry?.path();

        if path.is_file() && is_text_file(&path) {
            let content = fs::read_to_string(&path)?;

            // Check if file changed
            if !cache.has_changed(&path, &content) {
                tracing::debug!("Skipping unchanged file: {}", path.display());
                continue; // Skip unchanged files
            }

            // Index the file
            let metadata = fs::metadata(&path)?;
            let file_meta = FileMetadata {
                hash: MetadataCache::hash_content(&content),
                last_modified: metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs(),
                size: metadata.len(),
                indexed_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            };

            index_writer.add_document(doc!(
                schema.relative_path => path.to_string_lossy().to_string(),
                schema.content => content,
                schema.unique_hash => file_meta.hash.clone(),
                schema.last_modified => file_meta.last_modified,
                schema.file_size => file_meta.size,
            ))?;

            // Update cache
            cache.set(path.to_str().unwrap(), file_meta)?;
        }
    }
    Ok(())
}
```

**Test:**
```bash
# First run - indexes all files
cargo run -- search --directory /path/to/test --keyword "test"

# Modify one file
echo "new content" >> /path/to/test/file.rs

# Second run - should only reindex the changed file
cargo run -- search --directory /path/to/test --keyword "test"
```

---

### Step 6: File Watching (4-5 hours)

**File:** `src/watcher.rs`

Use `notify` crate to detect file changes:

```rust
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::channel;

pub struct FileWatcher {
    watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn new(path: &Path) -> Result<Self, notify::Error> {
        let (tx, rx) = channel();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            match res {
                Ok(event) => tx.send(event).unwrap(),
                Err(e) => tracing::error!("Watch error: {:?}", e),
            }
        })?;

        watcher.watch(path, RecursiveMode::Recursive)?;

        Ok(Self { watcher })
    }

    pub fn watch_for_changes<F>(&mut self, callback: F)
    where
        F: Fn(&Path) + Send + 'static
    {
        // Handle file change events
        // Trigger reindexing for changed files
    }
}
```

**Integration Plan:**
- Phase 1: Basic file watching (log changes)
- Phase 1.5: Auto-reindex on changes (optional, might defer to Phase 2)

**Test:**
```bash
# Run watcher in background
cargo run -- watch /path/to/directory

# In another terminal, modify files
echo "change" >> /path/to/directory/test.rs

# Should see log output about detected change
```

---

### Step 7: Configuration & CLI (2 hours)

Add configuration options:

```rust
// src/config.rs
#[derive(Debug, Clone)]
pub struct Config {
    pub index_path: PathBuf,
    pub cache_path: PathBuf,
    pub auto_watch: bool,
}

impl Config {
    pub fn from_env() -> Self {
        let index_path = std::env::var("INDEX_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| Self::default_index_path());

        Self {
            index_path,
            cache_path: index_path.join("metadata"),
            auto_watch: std::env::var("AUTO_WATCH").is_ok(),
        }
    }

    fn default_index_path() -> PathBuf {
        ProjectDirs::from("dev", "rust-code-mcp", "search")
            .expect("Could not determine data directory")
            .data_dir()
            .to_path_buf()
    }
}
```

**Environment Variables:**
- `INDEX_PATH`: Where to store index (default: XDG data dir)
- `AUTO_WATCH`: Enable file watching (default: false)

---

## 🧪 Testing Strategy

### Unit Tests
```bash
# Test individual modules
cargo test --lib schema
cargo test --lib metadata_cache
cargo test --lib watcher
```

### Integration Tests
```bash
# Test end-to-end indexing
cargo test --test integration_test
```

**Test scenarios:**
1. ✅ Index directory from scratch
2. ✅ Reindex same directory (should skip unchanged files)
3. ✅ Modify one file, reindex (should only update that file)
4. ✅ Add new file to directory (should index only new file)
5. ✅ Delete file (should remove from index)

### Manual Testing
```bash
# 1. Index the rust-code-mcp project itself
cargo run -- search --directory . --keyword "tantivy"

# 2. Check index location
ls -la ~/.local/share/rust-code-mcp/search/

# 3. Modify a file
echo "// test comment" >> src/main.rs

# 4. Reindex
cargo run -- search --directory . --keyword "tantivy"
# Should be faster, only reindex src/main.rs

# 5. Check cache
sled dump ~/.local/share/rust-code-mcp/search/metadata/
```

---

## 📊 Success Criteria

Phase 1 is complete when:

- [x] **Persistence**: Index survives application restarts
- [x] **Incremental**: Only changed files are reindexed
- [x] **Performance**: Second index run 10x+ faster than first
- [x] **Accuracy**: Metadata cache correctly tracks file changes
- [x] **Stability**: No index corruption after multiple updates

### Performance Targets

**Baseline (current in-memory):**
- Index 368 LOC (rust-code-mcp): ~50ms
- Reindex: ~50ms (rebuilds everything)

**Phase 1 targets:**
- First index: ~100ms (overhead of disk writes)
- Reindex (no changes): <10ms (skip everything)
- Reindex (1 changed file): ~15ms (update only that file)

---

## 🗓️ Timeline

| Week | Tasks | Deliverables |
|------|-------|--------------|
| **Week 2** | Steps 1-3 | Dependencies, schema, metadata cache |
| **Week 3** | Steps 4-5 | Persistent index, incremental updates |
| **Week 4** | Steps 6-7 | File watching, testing, polish |

---

## 🔗 Reference Files

**From bloop to study:**
- `indexes/schema.rs` (lines 70-135) - Schema design
- `cache.rs` - File metadata patterns
- `background/sync.rs` (lines 304-363) - Incremental sync

**Current code to modify:**
- `src/tools/search_tool.rs` (line 123) - Change to persistent
- `src/tools/search_tool.rs` (lines 238-247) - Add cache check

---

## 🚀 Next Actions

**Ready to start?** Here's the first task:

```bash
# 1. Update Cargo.toml with dependencies
# 2. Run cargo build to verify
# 3. Create src/schema.rs
```

Want me to start with Step 1 (updating dependencies)?

---

**Last Updated:** 2025-10-17
**Status:** Planning Complete, Ready to Implement
