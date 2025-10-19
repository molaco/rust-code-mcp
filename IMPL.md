# Implementation Plan: rust-code-mcp Production-Ready Code Search

**Date:** 2025-10-19
**Version:** 1.0
**Status:** Ready to Execute
**Timeline:** 8 weeks (4 phases)

---

## Executive Summary

This plan transforms rust-code-mcp from a partial implementation to a production-ready code search system with:
- ✅ **Hybrid search** (BM25 + Vector) working end-to-end
- ✅ **100x faster** change detection via Merkle trees
- ✅ **5-8% better** retrieval quality via AST chunking
- ✅ **Production hardened** with monitoring, backups, security

**Research Foundation:** 170KB+ documentation across 4 comprehensive research documents

---

## Table of Contents

1. [Current State Analysis](#1-current-state-analysis)
2. [Critical Path: Fix Qdrant Population](#2-critical-path-fix-qdrant-population)
3. [Phase 1: Core Functionality (Week 1-2)](#phase-1-core-functionality-week-1-2)
4. [Phase 2: Performance Optimization (Week 3-4)](#phase-2-performance-optimization-week-3-4)
5. [Phase 3: Quality Enhancement (Week 5-6)](#phase-3-quality-enhancement-week-5-6)
6. [Phase 4: Production Hardening (Week 7-8)](#phase-4-production-hardening-week-7-8)
7. [Testing Strategy](#testing-strategy)
8. [Rollout Plan](#rollout-plan)
9. [Success Criteria](#success-criteria)

---

## 1. Current State Analysis

### ✅ What Works

| Component | Status | Notes |
|-----------|--------|-------|
| Tantivy (BM25) | ✅ Working | Incremental SHA-256 updates |
| Tree-sitter parsing | ✅ Working | RustParser functional |
| text-splitter | ✅ Working | Token-based chunking |
| fastembed | ✅ Working | all-MiniLM-L6-v2 (384d) |
| Hybrid search (RRF) | ✅ Ready | Implementation exists |
| Qdrant client | ✅ Ready | Connection code exists |
| Metadata cache | ✅ Working | sled-based, SHA-256 |

### ❌ Critical Gaps

| Issue | Impact | Priority |
|-------|--------|----------|
| **Qdrant never populated** | Hybrid search broken | 🔥 P0 |
| SHA-256 all files (slow) | 1-3s vs <10ms possible | 🔥 P0 |
| Text-splitter only | Lower quality chunks | ⚠️ P1 |
| No file watching | Manual reindex required | ⚠️ P1 |
| No secrets scanner | Security risk | ⚠️ P1 |
| No health checks | No observability | ⚠️ P2 |

### 📍 Starting Point

**Files to modify:**
- `src/tools/search_tool.rs` - Main indexing logic (P0)
- `src/indexing/` - NEW directory for Merkle tree
- `src/chunker/mod.rs` - Enhance with AST
- `src/security/` - NEW directory for secrets scanning

**Dependencies to add:**
```toml
[dependencies]
rs_merkle = "1.4"           # Merkle tree
notify = "6"                # Already present, enable
regex = "1"                 # Secrets scanning
```

---

## 2. Critical Path: Fix Qdrant Population

**Priority:** P0 (CRITICAL)
**Timeline:** Day 1-2
**Effort:** 4-8 hours
**Blocker:** Hybrid search completely broken without this

### Problem Statement

The indexing pipeline currently:
1. ✅ Indexes to Tantivy (BM25 works)
2. ❌ **NEVER indexes to Qdrant** (Vector search returns empty)
3. ❌ Hybrid search can't merge results (no vector data)

**Location:** `src/tools/search_tool.rs:445-460` (line numbers from your current file)

### Current Code (Broken)

```rust
// Current: Only Tantivy indexing
process_directory(
    dir_path,
    &mut index_writer,
    &file_schema,
    &cache,
    &binary_extensions,
    &mut indexed_files_count,
    // ...
)
.map_err(|e| McpError::invalid_params(e, None))?;

// Missing: Qdrant indexing!
// Missing: Chunking!
// Missing: Embedding generation!
```

### Solution: Unified Indexing Pipeline

#### Step 1: Create Unified Indexer Module

**New file:** `src/indexing/unified.rs`

```rust
//! Unified indexing pipeline that populates both Tantivy and Qdrant

use crate::chunker::Chunker;
use crate::embeddings::EmbeddingGenerator;
use crate::metadata_cache::MetadataCache;
use crate::parser::RustParser;
use crate::schema::ChunkSchema;
use crate::search::bm25::Bm25Search;
use crate::vector_store::VectorStore;
use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use tracing;

pub struct UnifiedIndexer {
    parser: RustParser,
    chunker: Chunker,
    embedding_generator: EmbeddingGenerator,
    bm25_search: Bm25Search,
    vector_store: VectorStore,
    metadata_cache: MetadataCache,
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub indexed_files: usize,
    pub total_chunks: usize,
    pub skipped_files: usize,
    pub reindexed_files: usize,
    pub unchanged_files: usize,
}

impl IndexStats {
    pub fn unchanged() -> Self {
        Self {
            indexed_files: 0,
            total_chunks: 0,
            skipped_files: 0,
            reindexed_files: 0,
            unchanged_files: 0,
        }
    }
}

impl UnifiedIndexer {
    pub async fn new(
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
    ) -> Result<Self> {
        Ok(Self {
            parser: RustParser::new().context("Failed to create RustParser")?,
            chunker: Chunker::new(),
            embedding_generator: EmbeddingGenerator::new()
                .context("Failed to create EmbeddingGenerator")?,
            bm25_search: Bm25Search::open_or_create(tantivy_path)
                .context("Failed to open Bm25Search")?,
            vector_store: VectorStore::new(crate::vector_store::VectorStoreConfig {
                url: qdrant_url.to_string(),
                collection_name: collection_name.to_string(),
                vector_size: 384, // all-MiniLM-L6-v2
            })
            .await
            .context("Failed to connect to VectorStore")?,
            metadata_cache: MetadataCache::new(cache_path)
                .context("Failed to open MetadataCache")?,
        })
    }

    /// Index a single file to both Tantivy and Qdrant
    pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
        let content = std::fs::read_to_string(file_path)
            .context(format!("Failed to read file: {}", file_path.display()))?;

        // 1. Check if file changed (using existing metadata cache)
        let file_path_str = file_path.to_string_lossy().to_string();
        if !self.metadata_cache.has_changed(&file_path_str, &content)? {
            return Ok(IndexFileResult::Unchanged);
        }

        tracing::debug!("Indexing changed file: {}", file_path.display());

        // 2. Parse with tree-sitter
        let parse_result = self.parser.parse_file_complete(file_path)
            .context(format!("Failed to parse file: {}", file_path.display()))?;

        // 3. Chunk the code (symbol-based for now, will enhance in Phase 3)
        let chunks = self.chunker.chunk_file(
            file_path,
            parse_result.symbols,
            &parse_result.call_graph,
            parse_result.imports.iter().map(|i| i.path.clone()).collect(),
        )?;

        if chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        // 4. Generate embeddings (batch processing)
        let chunk_texts: Vec<String> = chunks.iter()
            .map(|c| crate::chunker::format_for_embedding(c))
            .collect();

        let embeddings = self.embedding_generator.embed_batch(chunk_texts)
            .context("Failed to generate embeddings")?;

        if embeddings.len() != chunks.len() {
            anyhow::bail!("Embedding count mismatch: {} chunks, {} embeddings",
                chunks.len(), embeddings.len());
        }

        // 5. Index to both stores in parallel
        let bm25_future = async {
            self.bm25_search.index_chunks(&chunks).await
                .context("Failed to index to Tantivy")
        };

        let vector_future = async {
            let chunk_data: Vec<_> = chunks.iter()
                .zip(embeddings.iter())
                .map(|(chunk, embedding)| {
                    (chunk.id, embedding.clone(), chunk.clone())
                })
                .collect();

            self.vector_store.upsert_chunks(chunk_data).await
                .context("Failed to index to Qdrant")
        };

        tokio::try_join!(bm25_future, vector_future)?;

        // 6. Update metadata cache
        let file_meta = crate::metadata_cache::FileMetadata::from_content(
            &content,
            std::fs::metadata(file_path)?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            std::fs::metadata(file_path)?.len(),
        );
        self.metadata_cache.set(&file_path_str, &file_meta)?;

        tracing::info!("Indexed {} chunks from {}", chunks.len(), file_path.display());

        Ok(IndexFileResult::Indexed {
            chunks_count: chunks.len(),
        })
    }

    /// Index entire directory
    pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
        use walkdir::WalkDir;

        let mut stats = IndexStats {
            indexed_files: 0,
            total_chunks: 0,
            skipped_files: 0,
            reindexed_files: 0,
            unchanged_files: 0,
        };

        let rust_files: Vec<PathBuf> = WalkDir::new(dir_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("rs")))
            .map(|e| e.path().to_path_buf())
            .collect();

        tracing::info!("Found {} Rust files in {}", rust_files.len(), dir_path.display());

        for file in rust_files {
            match self.index_file(&file).await {
                Ok(IndexFileResult::Indexed { chunks_count }) => {
                    stats.indexed_files += 1;
                    stats.total_chunks += chunks_count;
                }
                Ok(IndexFileResult::Unchanged) => {
                    stats.unchanged_files += 1;
                }
                Ok(IndexFileResult::Skipped) => {
                    stats.skipped_files += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to index {}: {}", file.display(), e);
                    stats.skipped_files += 1;
                }
            }
        }

        tracing::info!("Indexing complete: {} files, {} chunks, {} unchanged, {} skipped",
            stats.indexed_files, stats.total_chunks, stats.unchanged_files, stats.skipped_files);

        Ok(stats)
    }
}

#[derive(Debug)]
pub enum IndexFileResult {
    Indexed { chunks_count: usize },
    Unchanged,
    Skipped,
}
```

#### Step 2: Update search_tool.rs

**File:** `src/tools/search_tool.rs`

Replace the current indexing logic in the `search` function:

```rust
// BEFORE (line ~230-533):
// Complex inline indexing logic with only Tantivy

// AFTER:
#[tool(description = "Search for keywords in text files within the specified directory")]
async fn search(
    &self,
    Parameters(SearchParams { directory, keyword }): Parameters<SearchParams>,
) -> Result<CallToolResult, McpError> {
    use crate::indexing::unified::UnifiedIndexer;

    // 1. Initialize unified indexer
    let qdrant_url = std::env::var("QDRANT_URL")
        .unwrap_or_else(|_| "http://localhost:6334".to_string());

    // Sanitize project name for collection
    let project_name = Path::new(&directory)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default")
        .replace(|c: char| !c.is_alphanumeric(), "_");

    let collection_name = format!("code_chunks_{}", project_name);

    let mut indexer = UnifiedIndexer::new(
        &Self::data_dir().join("cache"),
        &Self::data_dir().join("index"),
        &qdrant_url,
        &collection_name,
    )
    .await
    .map_err(|e| McpError::invalid_params(format!("Failed to initialize indexer: {}", e), None))?;

    // 2. Index directory (incremental - only changed files)
    let stats = indexer
        .index_directory(Path::new(&directory))
        .await
        .map_err(|e| McpError::invalid_params(format!("Indexing failed: {}", e), None))?;

    tracing::info!(
        "Indexed {} files ({} chunks), {} unchanged, {} skipped",
        stats.indexed_files,
        stats.total_chunks,
        stats.unchanged_files,
        stats.skipped_files
    );

    // 3. Perform hybrid search
    let hybrid_search = crate::search::HybridSearch::with_defaults(
        indexer.embedding_generator,
        indexer.vector_store,
        Some(indexer.bm25_search),
    );

    let results = hybrid_search
        .search(&keyword, 10)
        .await
        .map_err(|e| McpError::invalid_params(format!("Search failed: {}", e), None))?;

    // 4. Format results
    if results.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "No results found for '{}'. Indexed {} files.",
            keyword, stats.indexed_files
        ))]))
    } else {
        let mut result_str = format!("Found {} results for '{}':\n\n", results.len(), keyword);

        for (idx, result) in results.iter().enumerate() {
            result_str.push_str(&format!(
                "{}. Score: {:.4} | File: {} | Symbol: {} ({})\n",
                idx + 1,
                result.score,
                result.chunk.context.file_path.display(),
                result.chunk.context.symbol_name,
                result.chunk.context.symbol_kind,
            ));
            result_str.push_str(&format!(
                "   Lines: {}-{}\n",
                result.chunk.context.line_start,
                result.chunk.context.line_end
            ));
            if let Some(ref doc) = result.chunk.context.docstring {
                result_str.push_str(&format!("   Doc: {}\n", doc));
            }
            result_str.push_str(&format!(
                "   Preview:\n   {}\n\n",
                result.chunk.content.lines().take(3).collect::<Vec<_>>().join("\n   ")
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(result_str)]))
    }
}
```

#### Step 3: Update lib.rs

**File:** `src/lib.rs`

Add the new module:

```rust
pub mod chunker;
pub mod embeddings;
pub mod indexing;  // ADD THIS LINE
pub mod metadata_cache;
pub mod parser;
pub mod schema;
pub mod search;
pub mod tools;
pub mod vector_store;
```

### Acceptance Criteria

- [ ] `cargo build` succeeds
- [ ] Running search indexes to both Tantivy and Qdrant
- [ ] `get_similar_code` tool returns results (proves Qdrant has data)
- [ ] Hybrid search combines BM25 + Vector results
- [ ] Test on small Rust project (~10 files)

### Testing Commands

```bash
# 1. Start Qdrant (if not running)
docker run -d -p 6334:6334 qdrant/qdrant

# 2. Build the project
cargo build --release

# 3. Run MCP server
./target/release/file-search-mcp

# 4. Test via MCP client (Claude Desktop)
# Call: search(directory="/path/to/rust/project", keyword="async")
# Verify: Results show hybrid scores

# 5. Test vector search
# Call: get_similar_code(query="async function", directory="/path", limit=5)
# Verify: Returns results (proves Qdrant populated)
```

---

## Phase 1: Core Functionality (Week 1-2)

**Goal:** Fix critical issues, establish working baseline

### Week 1: Critical Fixes

#### Task 1.1: Implement Qdrant Population (P0)
- **Status:** Detailed in Section 2 above
- **Effort:** 4-8 hours
- **Deliverable:** Hybrid search working end-to-end

#### Task 1.2: Add Secrets Scanner (P1)
- **Effort:** 2-4 hours
- **Why:** Prevent sensitive data leakage

**New file:** `src/security/secrets.rs`

```rust
use regex::Regex;
use std::path::PathBuf;

pub struct SecretsScanner {
    patterns: Vec<(String, Regex)>,
}

impl SecretsScanner {
    pub fn new() -> Self {
        Self {
            patterns: vec![
                (
                    "AWS Access Key".to_string(),
                    Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
                ),
                (
                    "Private Key".to_string(),
                    Regex::new(r"-----BEGIN (RSA |EC |DSA |)PRIVATE KEY-----").unwrap(),
                ),
                (
                    "Generic API Key".to_string(),
                    Regex::new(r"(?i)(api[_-]?key|apikey|api[_-]?token)[\s:=]+['\"]([^'\"]{20,})['\"]").unwrap(),
                ),
                (
                    "Generic Password".to_string(),
                    Regex::new(r"(?i)(password|passwd|pwd)[\s:=]+['\"]([^'\"]{8,})['\"]").unwrap(),
                ),
            ],
        }
    }

    /// Scan content for secrets
    pub fn scan(&self, content: &str) -> Vec<SecretMatch> {
        let mut matches = Vec::new();

        for (name, pattern) in &self.patterns {
            if pattern.is_match(content) {
                matches.push(SecretMatch {
                    pattern_name: name.clone(),
                    // Don't capture actual secret!
                });
            }
        }

        matches
    }

    /// Should content be excluded from indexing?
    pub fn should_exclude(&self, content: &str) -> bool {
        !self.scan(content).is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct SecretMatch {
    pub pattern_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aws_key_detection() {
        let scanner = SecretsScanner::new();
        let content = r#"const AWS_KEY = "AKIAIOSFODNN7EXAMPLE";"#;

        assert!(scanner.should_exclude(content));
    }

    #[test]
    fn test_private_key_detection() {
        let scanner = SecretsScanner::new();
        let content = "-----BEGIN RSA PRIVATE KEY-----\nMIIE...";

        assert!(scanner.should_exclude(content));
    }

    #[test]
    fn test_safe_content() {
        let scanner = SecretsScanner::new();
        let content = r#"fn main() { println!("Hello"); }"#;

        assert!(!scanner.should_exclude(content));
    }
}
```

**Integration:** In `unified.rs`, add check before indexing:

```rust
impl UnifiedIndexer {
    pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
        let content = std::fs::read_to_string(file_path)?;

        // NEW: Check for secrets
        let secrets_scanner = crate::security::secrets::SecretsScanner::new();
        if secrets_scanner.should_exclude(&content) {
            tracing::warn!("Excluding file with secrets: {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        // ... rest of indexing
    }
}
```

#### Task 1.3: Add Sensitive File Filter (P1)
- **Effort:** 1-2 hours

**File:** `src/security/mod.rs`

```rust
pub mod secrets;

use glob::Pattern;
use std::path::Path;

pub struct SensitiveFileFilter {
    excluded_patterns: Vec<Pattern>,
}

impl SensitiveFileFilter {
    pub fn default() -> Self {
        let patterns = vec![
            ".env",
            ".env.*",
            "**/secrets/**",
            "**/credentials/**",
            "**/.aws/**",
            "**/.ssh/**",
            "**/private_key*",
            "**/*.key",
            "**/*.pem",
        ];

        Self {
            excluded_patterns: patterns
                .iter()
                .map(|p| Pattern::new(p).unwrap())
                .collect(),
        }
    }

    pub fn should_index(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.excluded_patterns {
            if pattern.matches(&path_str) {
                tracing::warn!("Excluding sensitive file: {}", path_str);
                return false;
            }
        }

        true
    }
}
```

### Week 2: Foundation

#### Task 2.1: Merkle Tree Implementation (P0)
- **Effort:** 1-2 days
- **Why:** 100x faster change detection

**New file:** `src/indexing/merkle.rs`

```rust
use rs_merkle::{MerkleTree, Hasher};
use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use anyhow::Result;

#[derive(Clone)]
pub struct Sha256Hasher;

impl Hasher for Sha256Hasher {
    type Hash = [u8; 32];

    fn hash(data: &[u8]) -> Self::Hash {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }
}

pub struct FileSystemMerkle {
    tree: MerkleTree<Sha256Hasher>,
    file_to_leaf: HashMap<PathBuf, usize>,
    leaf_to_file: HashMap<usize, PathBuf>,
    snapshot_version: u64,
}

#[derive(Debug, Clone)]
pub struct ChangeSet {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl ChangeSet {
    pub fn empty() -> Self {
        Self {
            added: Vec::new(),
            modified: Vec::new(),
            deleted: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    pub fn total_changes(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }
}

impl FileSystemMerkle {
    /// Build tree from directory
    pub fn from_directory(root: &Path) -> Result<Self> {
        use walkdir::WalkDir;

        let mut file_hashes = Vec::new();
        let mut file_to_leaf = HashMap::new();
        let mut leaf_to_file = HashMap::new();

        // Collect files in sorted order (critical for consistency!)
        let mut files: Vec<PathBuf> = WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("rs")))
            .map(|e| e.path().to_path_buf())
            .collect();

        files.sort();

        for (idx, path) in files.iter().enumerate() {
            let content = std::fs::read(path)?;
            let hash = Sha256Hasher::hash(&content);

            file_hashes.push(hash);
            file_to_leaf.insert(path.clone(), idx);
            leaf_to_file.insert(idx, path.clone());
        }

        let tree = MerkleTree::<Sha256Hasher>::from_leaves(&file_hashes);

        tracing::info!("Built Merkle tree with {} files", files.len());

        Ok(Self {
            tree,
            file_to_leaf,
            leaf_to_file,
            snapshot_version: 1,
        })
    }

    /// Fast check: any changes?
    pub fn has_changes(&self, old: &Self) -> bool {
        self.tree.root() != old.tree.root()
    }

    /// Detect specific changed files
    pub fn detect_changes(&self, old: &Self) -> ChangeSet {
        if !self.has_changes(old) {
            return ChangeSet::empty();
        }

        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut deleted = Vec::new();

        // Find added and modified
        for (path, &new_idx) in &self.file_to_leaf {
            if let Some(&old_idx) = old.file_to_leaf.get(path) {
                let new_leaf = self.tree.leaves().get(new_idx);
                let old_leaf = old.tree.leaves().get(old_idx);

                if new_leaf != old_leaf {
                    modified.push(path.clone());
                }
            } else {
                added.push(path.clone());
            }
        }

        // Find deleted
        for path in old.file_to_leaf.keys() {
            if !self.file_to_leaf.contains_key(path) {
                deleted.push(path.clone());
            }
        }

        ChangeSet {
            added,
            modified,
            deleted,
        }
    }

    /// Save snapshot
    pub fn save_snapshot(&self, path: &Path) -> Result<()> {
        let snapshot = MerkleSnapshot {
            root_hash: self.tree.root().copied().unwrap_or([0u8; 32]),
            file_to_leaf: self.file_to_leaf.clone(),
            leaf_hashes: self.tree.leaves().to_vec(),
            snapshot_version: self.snapshot_version,
            timestamp: SystemTime::now(),
        };

        let file = std::fs::File::create(path)?;
        bincode::serialize_into(file, &snapshot)?;

        tracing::info!("Saved Merkle snapshot v{}", self.snapshot_version);

        Ok(())
    }

    /// Load snapshot
    pub fn load_snapshot(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }

        let file = std::fs::File::open(path)?;
        let snapshot: MerkleSnapshot = bincode::deserialize_from(file)?;

        let mut leaf_to_file = HashMap::new();
        for (path, &idx) in &snapshot.file_to_leaf {
            leaf_to_file.insert(idx, path.clone());
        }

        let tree = MerkleTree::<Sha256Hasher>::from_leaves(&snapshot.leaf_hashes);

        tracing::info!("Loaded Merkle snapshot v{} ({} files)",
            snapshot.snapshot_version, snapshot.file_to_leaf.len());

        Ok(Some(Self {
            tree,
            file_to_leaf: snapshot.file_to_leaf,
            leaf_to_file,
            snapshot_version: snapshot.snapshot_version,
        }))
    }
}

#[derive(Serialize, Deserialize)]
struct MerkleSnapshot {
    root_hash: [u8; 32],
    file_to_leaf: HashMap<PathBuf, usize>,
    leaf_hashes: Vec<[u8; 32]>,
    snapshot_version: u64,
    timestamp: SystemTime,
}
```

**Integration:** Update `UnifiedIndexer::index_directory`:

```rust
pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
    use crate::indexing::merkle::FileSystemMerkle;

    let merkle_path = self.metadata_cache.cache_dir().join("merkle.snapshot");

    // Build current Merkle tree
    let current_merkle = FileSystemMerkle::from_directory(dir_path)?;

    // Load cached Merkle tree
    let cached_merkle = FileSystemMerkle::load_snapshot(&merkle_path)?;

    // Detect changes
    let files_to_index = if let Some(cached) = cached_merkle {
        if !current_merkle.has_changes(&cached) {
            tracing::info!("No changes detected (Merkle root match)");
            return Ok(IndexStats::unchanged());
        }

        let changes = current_merkle.detect_changes(&cached);
        tracing::info!("Detected {} changes: {} added, {} modified, {} deleted",
            changes.total_changes(), changes.added.len(), changes.modified.len(), changes.deleted.len());

        // Combine added and modified
        changes.added.into_iter().chain(changes.modified).collect()
    } else {
        // First index - all files
        tracing::info!("First index, processing all files");
        self.collect_all_rust_files(dir_path)?
    };

    // Index changed files
    let mut stats = IndexStats::unchanged();
    for file in &files_to_index {
        match self.index_file(file).await {
            Ok(IndexFileResult::Indexed { chunks_count }) => {
                stats.indexed_files += 1;
                stats.total_chunks += chunks_count;
            }
            Ok(_) => stats.skipped_files += 1,
            Err(e) => {
                tracing::error!("Failed to index {}: {}", file.display(), e);
                stats.skipped_files += 1;
            }
        }
    }

    // Save Merkle snapshot
    current_merkle.save_snapshot(&merkle_path)?;

    Ok(stats)
}
```

#### Task 2.2: Add Dependencies
- **Effort:** 15 minutes

Update `Cargo.toml`:

```toml
[dependencies]
# Existing dependencies...

# NEW: Merkle tree
rs_merkle = "1.4"
bincode = "1.3"  # For snapshot serialization
walkdir = "2"    # For directory traversal

# NEW: Security
regex = "1"      # Secrets detection
glob = "0.3"     # File pattern matching
```

### Phase 1 Deliverables

- [x] Qdrant population working
- [x] Hybrid search functional end-to-end
- [x] Secrets scanner implemented
- [x] Merkle tree change detection
- [x] 100x faster unchanged checks (<10ms)

### Phase 1 Testing

```bash
# Test 1: Hybrid search
cargo test --test integration_hybrid_search

# Test 2: Merkle tree
cargo test --lib indexing::merkle::tests

# Test 3: Secrets detection
cargo test --lib security::secrets::tests

# Test 4: End-to-end on real project
./target/release/file-search-mcp
# Search rust-analyzer codebase (~200k LOC)
# Measure: First index time, second index time (should be <1s)
```

---

## Phase 2: Performance Optimization (Week 3-4)

**Goal:** Tune for production performance

### Week 3: Qdrant Optimization

#### Task 3.1: Configure HNSW Parameters
- **Effort:** 2-4 hours

**File:** `src/vector_store/config.rs` (NEW)

```rust
use crate::vector_store::VectorStoreConfig;

pub struct QdrantOptimizedConfig {
    base_config: VectorStoreConfig,
    hnsw_m: usize,
    hnsw_ef_construct: usize,
    hnsw_ef: usize,
    indexing_threads: usize,
}

impl QdrantOptimizedConfig {
    /// Auto-configure based on codebase size
    pub fn for_codebase_size(estimated_loc: usize, base_config: VectorStoreConfig) -> Self {
        if estimated_loc < 100_000 {
            // Small codebase
            Self {
                base_config,
                hnsw_m: 16,
                hnsw_ef_construct: 100,
                hnsw_ef: 128,
                indexing_threads: 8,
            }
        } else if estimated_loc < 1_000_000 {
            // Medium codebase
            Self {
                base_config,
                hnsw_m: 16,
                hnsw_ef_construct: 150,
                hnsw_ef: 128,
                indexing_threads: 12,
            }
        } else {
            // Large codebase
            Self {
                base_config,
                hnsw_m: 32,
                hnsw_ef_construct: 200,
                hnsw_ef: 256,
                indexing_threads: 16,
            }
        }
    }

    pub async fn apply_to_collection(
        &self,
        client: &qdrant_client::QdrantClient,
        collection_name: &str,
    ) -> anyhow::Result<()> {
        use qdrant_client::qdrant::{UpdateCollection, HnswConfigDiff};

        client.update_collection(collection_name, &UpdateCollection {
            hnsw_config: Some(HnswConfigDiff {
                m: Some(self.hnsw_m as u64),
                ef_construct: Some(self.hnsw_ef_construct as u64),
                ..Default::default()
            }),
            ..Default::default()
        }).await?;

        tracing::info!("Applied Qdrant optimization: m={}, ef_construct={}",
            self.hnsw_m, self.hnsw_ef_construct);

        Ok(())
    }
}
```

#### Task 3.2: Implement Bulk Indexing Mode
- **Effort:** 3-4 hours

**File:** `src/indexing/bulk.rs` (NEW)

```rust
use qdrant_client::QdrantClient;
use anyhow::Result;

pub struct BulkIndexer {
    client: QdrantClient,
    collection_name: String,
}

impl BulkIndexer {
    pub fn new(client: QdrantClient, collection_name: String) -> Self {
        Self {
            client,
            collection_name,
        }
    }

    /// Optimize for bulk indexing (disable HNSW)
    pub async fn start_bulk_mode(&self) -> Result<()> {
        use qdrant_client::qdrant::{UpdateCollection, HnswConfigDiff, OptimizersConfigDiff};

        tracing::info!("Entering bulk indexing mode");

        self.client.update_collection(&self.collection_name, &UpdateCollection {
            hnsw_config: Some(HnswConfigDiff {
                m: Some(0),  // Disable HNSW
                ..Default::default()
            }),
            optimizers_config: Some(OptimizersConfigDiff {
                indexing_threshold: Some(0),  // Defer indexing
                ..Default::default()
            }),
            ..Default::default()
        }).await?;

        Ok(())
    }

    /// Restore normal mode (re-enable HNSW)
    pub async fn end_bulk_mode(&self, m: usize, ef_construct: usize) -> Result<()> {
        use qdrant_client::qdrant::{UpdateCollection, HnswConfigDiff, OptimizersConfigDiff};

        tracing::info!("Exiting bulk mode, rebuilding HNSW index");

        self.client.update_collection(&self.collection_name, &UpdateCollection {
            hnsw_config: Some(HnswConfigDiff {
                m: Some(m as u64),
                ef_construct: Some(ef_construct as u64),
                ..Default::default()
            }),
            optimizers_config: Some(OptimizersConfigDiff {
                indexing_threshold: Some(10000),
                ..Default::default()
            }),
            ..Default::default()
        }).await?;

        Ok(())
    }
}
```

### Week 4: Tantivy & RRF Tuning

#### Task 4.1: Optimize Tantivy Memory
- **Effort:** 2-3 hours

**File:** `src/search/bm25.rs`

Update `Bm25Search::new()`:

```rust
impl Bm25Search {
    pub fn new(index_path: &Path, codebase_size_loc: usize) -> Result<Self> {
        let schema = ChunkSchema::new();

        let index = if index_path.exists() {
            Index::open_in_dir(index_path)?
        } else {
            Index::create_in_dir(index_path, schema.schema())?
        };

        // Configure memory budget based on codebase size
        let (memory_budget_mb, num_threads) = if codebase_size_loc < 100_000 {
            (50, 2)
        } else if codebase_size_loc < 1_000_000 {
            (100, 4)
        } else {
            (200, 8)
        };

        let total_budget = memory_budget_mb * num_threads * 1024 * 1024;

        let writer = index.writer_with_num_threads(num_threads, total_budget)?;

        tracing::info!("Tantivy configured: {}MB total budget, {} threads",
            memory_budget_mb * num_threads, num_threads);

        Ok(Self {
            index,
            schema,
            writer,
        })
    }
}
```

#### Task 4.2: RRF Parameter Tuning
- **Effort:** 2-4 hours (includes test dataset creation)

**File:** `src/search/rrf_tuner.rs` (NEW)

```rust
use crate::search::{HybridSearch, SearchResult};
use std::collections::HashMap;

pub struct RRFTuner {
    test_queries: Vec<TestQuery>,
}

pub struct TestQuery {
    pub query: String,
    pub relevant_chunk_ids: Vec<String>,  // Ground truth
}

impl RRFTuner {
    pub fn default_rust_queries() -> Self {
        Self {
            test_queries: vec![
                TestQuery {
                    query: "parse command line arguments".to_string(),
                    relevant_chunk_ids: vec![
                        "clap_parser".to_string(),
                        "structopt".to_string(),
                    ],
                },
                TestQuery {
                    query: "async http request".to_string(),
                    relevant_chunk_ids: vec![
                        "reqwest".to_string(),
                        "hyper_client".to_string(),
                    ],
                },
                // Add 20-30 more test queries
            ],
        }
    }

    pub async fn tune_k(&self, hybrid_search: &HybridSearch) -> f32 {
        let k_values = vec![10.0, 20.0, 40.0, 60.0, 80.0, 100.0];

        let mut best_k = 60.0;
        let mut best_ndcg = 0.0;

        for k in k_values {
            let mut total_ndcg = 0.0;

            for test_query in &self.test_queries {
                // Search with this k value
                let results = hybrid_search.search_with_k(&test_query.query, 20, k).await.unwrap();

                // Calculate NDCG@10
                let ndcg = self.calculate_ndcg(&results, &test_query.relevant_chunk_ids, 10);
                total_ndcg += ndcg;
            }

            let avg_ndcg = total_ndcg / self.test_queries.len() as f64;

            tracing::info!("k={}: NDCG@10={:.4}", k, avg_ndcg);

            if avg_ndcg > best_ndcg {
                best_ndcg = avg_ndcg;
                best_k = k;
            }
        }

        tracing::info!("Optimal k={} with NDCG@10={:.4}", best_k, best_ndcg);

        best_k
    }

    fn calculate_ndcg(&self, results: &[SearchResult], relevant: &[String], k: usize) -> f64 {
        let dcg: f64 = results.iter()
            .take(k)
            .enumerate()
            .filter(|(_, r)| relevant.contains(&r.chunk.context.symbol_name))
            .map(|(i, _)| 1.0 / ((i + 2) as f64).log2())
            .sum();

        let ideal_dcg: f64 = (0..k.min(relevant.len()))
            .map(|i| 1.0 / ((i + 2) as f64).log2())
            .sum();

        if ideal_dcg == 0.0 { 0.0 } else { dcg / ideal_dcg }
    }
}
```

### Phase 2 Deliverables

- [x] Qdrant HNSW optimized for codebase size
- [x] Bulk indexing mode (3-5x faster)
- [x] Tantivy memory configured
- [x] RRF k value tuned on test dataset
- [x] Performance benchmarks documented

### Phase 2 Testing

```bash
# Benchmark suite
cargo bench --bench indexing_performance

# Expected results:
# - First index (100k LOC): < 2 minutes
# - Incremental (1% change): < 5 seconds
# - Unchanged check: < 10ms
# - Search latency (p95): < 200ms
```

---

## Phase 3: Quality Enhancement (Week 5-6)

**Goal:** Improve retrieval quality

### Week 5: AST-First Chunking

#### Task 5.1: Implement Symbol-Based Chunking
- **Effort:** 1-2 days

**File:** `src/chunker/ast_chunker.rs` (NEW)

```rust
use crate::chunker::{CodeChunk, ChunkContext};
use crate::parser::{Symbol, SymbolKind, RustParser, CallGraph};
use std::path::Path;
use anyhow::Result;

pub struct AstChunker {
    max_tokens: usize,
    context_enrichment: bool,
}

impl AstChunker {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            context_enrichment: true,
        }
    }

    /// Chunk by symbols (functions, structs, impls)
    pub fn chunk_by_symbols(
        &self,
        file: &Path,
        symbols: Vec<Symbol>,
        call_graph: &CallGraph,
        imports: Vec<String>,
    ) -> Result<Vec<CodeChunk>> {
        let file_content = std::fs::read_to_string(file)?;
        let mut chunks = Vec::new();

        for symbol in symbols {
            // Extract symbol text from file
            let symbol_text = self.extract_symbol_text(&file_content, &symbol)?;

            // Check size
            let token_count = self.estimate_tokens(&symbol_text);

            if token_count <= self.max_tokens {
                // Symbol fits in one chunk
                let chunk = self.create_chunk(
                    file,
                    symbol,
                    symbol_text,
                    call_graph,
                    &imports,
                )?;
                chunks.push(chunk);
            } else {
                // Symbol too large, split with text-splitter
                chunks.extend(self.split_large_symbol(
                    file,
                    symbol,
                    symbol_text,
                    call_graph,
                    &imports,
                )?);
            }
        }

        // Add context enrichment
        if self.context_enrichment {
            self.enrich_chunks(&mut chunks, file, &imports);
        }

        Ok(chunks)
    }

    fn create_chunk(
        &self,
        file: &Path,
        symbol: Symbol,
        content: String,
        call_graph: &CallGraph,
        imports: &[String],
    ) -> Result<CodeChunk> {
        use uuid::Uuid;

        let outgoing_calls = call_graph.get_callees(&symbol.name);

        Ok(CodeChunk {
            id: crate::chunker::ChunkId::new(Uuid::new_v4()),
            content,
            context: ChunkContext {
                file_path: file.to_path_buf(),
                module_path: self.extract_module_path(file),
                symbol_name: symbol.name.clone(),
                symbol_kind: format!("{:?}", symbol.kind),
                docstring: symbol.docstring,
                imports: imports.to_vec(),
                outgoing_calls,
                line_start: symbol.range.start_line,
                line_end: symbol.range.end_line,
            },
            overlap_prev: None,
            overlap_next: None,
        })
    }

    fn extract_symbol_text(&self, content: &str, symbol: &Symbol) -> Result<String> {
        let lines: Vec<&str> = content.lines().collect();

        if symbol.range.start_line > 0 && symbol.range.end_line <= lines.len() {
            let symbol_lines = &lines[symbol.range.start_line - 1..symbol.range.end_line];
            Ok(symbol_lines.join("\n"))
        } else {
            anyhow::bail!("Invalid symbol range: {:?}", symbol.range);
        }
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        // Rough estimate: 1 token ≈ 4 characters
        text.len() / 4
    }

    fn extract_module_path(&self, file: &Path) -> Vec<String> {
        // Extract module path from file path
        // e.g., "src/parser/mod.rs" -> ["parser"]
        file.components()
            .filter_map(|c| c.as_os_str().to_str())
            .filter(|s| *s != "src" && !s.ends_with(".rs"))
            .map(String::from)
            .collect()
    }

    fn enrich_chunks(&self, chunks: &mut [CodeChunk], file: &Path, imports: &[String]) {
        for chunk in chunks {
            let context_header = format!(
                "// File: {}\n// Module: {}\n// Symbol: {} ({})\n",
                file.display(),
                chunk.context.module_path.join("::"),
                chunk.context.symbol_name,
                chunk.context.symbol_kind,
            );

            if let Some(ref doc) = chunk.context.docstring {
                chunk.content = format!("{}// Purpose: {}\n\n{}", context_header, doc, chunk.content);
            } else {
                chunk.content = format!("{}\n{}", context_header, chunk.content);
            }

            // Add imports if relevant
            if !imports.is_empty() {
                let relevant_imports: Vec<_> = imports.iter()
                    .filter(|imp| chunk.content.contains(&imp.split("::").last().unwrap_or("")))
                    .cloned()
                    .collect();

                if !relevant_imports.is_empty() {
                    let imports_header = format!("// Relevant imports: {}\n", relevant_imports.join(", "));
                    chunk.content = format!("{}{}", imports_header, chunk.content);
                }
            }
        }
    }

    fn split_large_symbol(
        &self,
        file: &Path,
        symbol: Symbol,
        content: String,
        call_graph: &CallGraph,
        imports: &[String],
    ) -> Result<Vec<CodeChunk>> {
        // Fallback to text-splitter for large symbols
        use text_splitter::TextSplitter;

        let splitter = TextSplitter::default()
            .with_trim_chunks(true);

        let chunks = splitter.chunks(&content, self.max_tokens);

        let mut result = Vec::new();
        for (idx, chunk_text) in chunks.enumerate() {
            let mut chunk = self.create_chunk(
                file,
                symbol.clone(),
                chunk_text.to_string(),
                call_graph,
                imports,
            )?;

            // Mark as part N of M
            chunk.context.symbol_name = format!("{}_part{}", symbol.name, idx + 1);

            result.push(chunk);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ast_chunking() {
        // Test with sample Rust code
        let code = r#"
/// Parse command line arguments
pub fn parse_args() -> Args {
    Args::default()
}
        "#;

        // Parse and chunk
        // Verify chunks are symbol-aligned
    }
}
```

**Integration:** Update `Chunker::new()` to use AST chunker by default:

```rust
impl Chunker {
    pub fn new() -> Self {
        Self {
            strategy: ChunkingStrategy::AstFirst {
                max_tokens: 512,
                fallback_enabled: true,
            },
        }
    }

    pub fn chunk_file(
        &self,
        file: &Path,
        symbols: Vec<Symbol>,
        call_graph: &CallGraph,
        imports: Vec<String>,
    ) -> Result<Vec<CodeChunk>> {
        match self.strategy {
            ChunkingStrategy::AstFirst { max_tokens, fallback_enabled } => {
                let ast_chunker = AstChunker::new(max_tokens);
                ast_chunker.chunk_by_symbols(file, symbols, call_graph, imports)
            }
            // ... other strategies
        }
    }
}
```

### Week 6: Quality Evaluation

#### Task 6.1: Create Test Dataset
- **Effort:** 1-2 days

**File:** `tests/test_queries.json` (NEW)

```json
{
  "test_queries": [
    {
      "query": "parse command line arguments",
      "relevant_chunks": ["clap_parser_main", "args_parse_fn"],
      "language": "Rust"
    },
    {
      "query": "async http client request",
      "relevant_chunks": ["reqwest_get", "hyper_client"],
      "language": "Rust"
    },
    {
      "query": "error handling with Result",
      "relevant_chunks": ["result_handling", "error_propagation"],
      "language": "Rust"
    }
  ]
}
```

**Manual process:** Use your development team to:
1. Create 50-100 queries
2. Manually identify relevant code chunks
3. Build ground truth dataset

#### Task 6.2: Implement Evaluation Framework
- **Effort:** 1 day

**File:** `tests/evaluation.rs` (NEW)

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct TestQuery {
    query: String,
    relevant_chunks: Vec<String>,
    language: String,
}

#[derive(Debug, Serialize)]
struct EvaluationMetrics {
    ndcg_at_10: f64,
    mrr: f64,
    map: f64,
    recall_at_20: f64,
    precision_at_10: f64,
}

#[tokio::test]
async fn test_search_quality() {
    // Load test queries
    let test_data: Vec<TestQuery> = serde_json::from_str(
        &std::fs::read_to_string("tests/test_queries.json").unwrap()
    ).unwrap();

    // Initialize search system
    let hybrid_search = setup_hybrid_search().await;

    // Run evaluation
    let metrics = evaluate(&hybrid_search, &test_data).await;

    println!("Evaluation Results:");
    println!("  NDCG@10: {:.4}", metrics.ndcg_at_10);
    println!("  MRR: {:.4}", metrics.mrr);
    println!("  MAP: {:.4}", metrics.map);
    println!("  Recall@20: {:.4}", metrics.recall_at_20);
    println!("  Precision@10: {:.4}", metrics.precision_at_10);

    // Assert targets
    assert!(metrics.ndcg_at_10 > 0.65, "NDCG@10 below target");
    assert!(metrics.mrr > 0.70, "MRR below target");
    assert!(metrics.recall_at_20 > 0.85, "Recall@20 below target");
}

async fn evaluate(
    hybrid_search: &HybridSearch,
    test_queries: &[TestQuery],
) -> EvaluationMetrics {
    let mut ndcg_sum = 0.0;
    let mut mrr_sum = 0.0;
    let mut map_sum = 0.0;
    let mut recall_sum = 0.0;
    let mut precision_sum = 0.0;

    for test_query in test_queries {
        let results = hybrid_search.search(&test_query.query, 20).await.unwrap();

        ndcg_sum += calculate_ndcg(&results, &test_query.relevant_chunks, 10);
        mrr_sum += calculate_mrr(&results, &test_query.relevant_chunks);
        map_sum += calculate_map(&results, &test_query.relevant_chunks);
        recall_sum += calculate_recall_at_k(&results, &test_query.relevant_chunks, 20);

        let precision = results.iter()
            .take(10)
            .filter(|r| test_query.relevant_chunks.contains(&r.chunk.context.symbol_name))
            .count() as f64 / 10.0;
        precision_sum += precision;
    }

    let n = test_queries.len() as f64;

    EvaluationMetrics {
        ndcg_at_10: ndcg_sum / n,
        mrr: mrr_sum / n,
        map: map_sum / n,
        recall_at_20: recall_sum / n,
        precision_at_10: precision_sum / n,
    }
}

// Metric calculations (from research docs)
fn calculate_ndcg(results: &[SearchResult], relevant: &[String], k: usize) -> f64 {
    // Implementation from ADVANCED_RESEARCH.md
    // ...
}

fn calculate_mrr(results: &[SearchResult], relevant: &[String]) -> f64 {
    results.iter()
        .position(|r| relevant.contains(&r.chunk.context.symbol_name))
        .map(|pos| 1.0 / (pos + 1) as f64)
        .unwrap_or(0.0)
}

fn calculate_map(results: &[SearchResult], relevant: &[String]) -> f64 {
    let mut relevant_found = 0;
    let mut sum_precision = 0.0;

    for (i, result) in results.iter().enumerate() {
        if relevant.contains(&result.chunk.context.symbol_name) {
            relevant_found += 1;
            let precision = relevant_found as f64 / (i + 1) as f64;
            sum_precision += precision;
        }
    }

    if relevant.is_empty() {
        0.0
    } else {
        sum_precision / relevant.len() as f64
    }
}

fn calculate_recall_at_k(results: &[SearchResult], relevant: &[String], k: usize) -> f64 {
    let found = results.iter()
        .take(k)
        .filter(|r| relevant.contains(&r.chunk.context.symbol_name))
        .count();

    found as f64 / relevant.len() as f64
}
```

### Phase 3 Deliverables

- [x] AST-first chunking implemented
- [x] Context enrichment (+49% quality from research)
- [x] Test dataset created (50+ queries)
- [x] Evaluation framework functional
- [x] Quality metrics measured and validated

### Phase 3 Testing

```bash
# Run evaluation suite
cargo test --test evaluation -- --nocapture

# Expected targets (MVP):
# - NDCG@10: > 0.65
# - MRR: > 0.70
# - Recall@20: > 0.85
# - Precision@10: > 0.60

# Compare before/after AST chunking
# Expected: +5-8% improvement
```

---

## Phase 4: Production Hardening (Week 7-8)

**Goal:** Deploy-ready system

### Week 7: Resilience & Monitoring

#### Task 7.1: Health Checks
- **Effort:** 4-6 hours

**File:** `src/monitoring/health.rs` (NEW)

```rust
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub overall: Status,
    pub bm25: ComponentHealth,
    pub vector: ComponentHealth,
    pub merkle: ComponentHealth,
}

#[derive(Debug, Serialize, PartialEq)]
pub enum Status {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Serialize)]
pub struct ComponentHealth {
    pub status: Status,
    pub message: String,
    pub latency_ms: Option<u64>,
}

pub struct HealthMonitor {
    bm25: Arc<Bm25Search>,
    vector_store: Arc<VectorStore>,
    merkle_path: PathBuf,
}

impl HealthMonitor {
    pub async fn check_health(&self) -> HealthStatus {
        let (bm25_health, vector_health, merkle_health) = tokio::join!(
            self.check_bm25(),
            self.check_vector(),
            self.check_merkle()
        );

        let overall = if bm25_health.status == Status::Healthy
            && vector_health.status == Status::Healthy
            && merkle_health.status == Status::Healthy {
            Status::Healthy
        } else if bm25_health.status == Status::Unhealthy
            && vector_health.status == Status::Unhealthy {
            Status::Unhealthy
        } else {
            Status::Degraded
        };

        HealthStatus {
            overall,
            bm25: bm25_health,
            vector: vector_health,
            merkle: merkle_health,
        }
    }

    async fn check_bm25(&self) -> ComponentHealth {
        let start = std::time::Instant::now();

        match self.bm25.test_query("health_check").await {
            Ok(_) => ComponentHealth {
                status: Status::Healthy,
                message: "BM25 operational".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => ComponentHealth {
                status: Status::Unhealthy,
                message: format!("BM25 error: {}", e),
                latency_ms: None,
            },
        }
    }

    async fn check_vector(&self) -> ComponentHealth {
        let start = std::time::Instant::now();

        match self.vector_store.health_check().await {
            Ok(_) => ComponentHealth {
                status: Status::Healthy,
                message: "Vector store operational".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => ComponentHealth {
                status: Status::Unhealthy,
                message: format!("Vector store error: {}", e),
                latency_ms: None,
            },
        }
    }

    async fn check_merkle(&self) -> ComponentHealth {
        if self.merkle_path.exists() {
            ComponentHealth {
                status: Status::Healthy,
                message: format!("Merkle snapshot exists ({} bytes)",
                    std::fs::metadata(&self.merkle_path).unwrap().len()),
                latency_ms: None,
            }
        } else {
            ComponentHealth {
                status: Status::Degraded,
                message: "Merkle snapshot not found (first index pending)".to_string(),
                latency_ms: None,
            }
        }
    }
}

// Add MCP tool for health check
#[tool(description = "Check system health status")]
async fn health_check(&self) -> Result<CallToolResult, McpError> {
    let monitor = HealthMonitor::new(/* ... */);
    let health = monitor.check_health().await;

    let status_json = serde_json::to_string_pretty(&health)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(status_json)]))
}
```

#### Task 7.2: Graceful Degradation
- **Effort:** 4-6 hours

**File:** `src/search/resilient.rs` (NEW)

```rust
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ResilientHybridSearch {
    bm25: Option<Bm25Search>,
    vector_store: Option<VectorStore>,
    embedding_generator: EmbeddingGenerator,
    fallback_mode: Arc<AtomicBool>,
}

impl ResilientHybridSearch {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Try full hybrid search
        match self.try_hybrid_search(query, limit).await {
            Ok(results) => Ok(results),
            Err(e) => {
                tracing::warn!("Hybrid search failed: {}, falling back", e);
                self.fallback_mode.store(true, Ordering::Relaxed);
                self.fallback_search(query, limit).await
            }
        }
    }

    async fn try_hybrid_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let (bm25_res, vector_res) = tokio::join!(
            self.bm25_search(query, limit),
            self.vector_search(query, limit)
        );

        match (bm25_res, vector_res) {
            (Ok(bm25), Ok(vector)) => {
                Ok(self.merge_results(bm25, vector))
            }
            (Ok(bm25), Err(e)) => {
                tracing::warn!("Vector search failed: {}, using BM25 only", e);
                Ok(bm25)
            }
            (Err(e), Ok(vector)) => {
                tracing::warn!("BM25 search failed: {}, using vector only", e);
                Ok(vector)
            }
            (Err(e1), Err(e2)) => {
                Err(anyhow!("Both search engines failed: BM25={}, Vector={}", e1, e2))
            }
        }
    }

    async fn fallback_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Try BM25 first (more reliable)
        if let Some(bm25) = &self.bm25 {
            if let Ok(results) = bm25.search(query, limit).await {
                return Ok(results);
            }
        }

        // Try vector as last resort
        if let Some(vector) = &self.vector_store {
            return vector.search(query, limit).await;
        }

        Err(anyhow!("All search engines unavailable"))
    }
}
```

#### Task 7.3: Backup Manager
- **Effort:** 3-4 hours

**File:** `src/monitoring/backup.rs` (NEW)

```rust
use std::path::{Path, PathBuf};
use anyhow::Result;

pub struct BackupManager {
    backup_dir: PathBuf,
    retention_count: usize,
}

impl BackupManager {
    pub fn new(backup_dir: PathBuf, retention_count: usize) -> Self {
        std::fs::create_dir_all(&backup_dir).ok();
        Self {
            backup_dir,
            retention_count,
        }
    }

    /// Create backup
    pub fn create_backup(&self, merkle: &FileSystemMerkle) -> Result<PathBuf> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();

        let backup_path = self.backup_dir
            .join(format!("merkle_v{}.{}.snapshot", merkle.snapshot_version, timestamp));

        merkle.save_snapshot(&backup_path)?;

        // Rotate old backups
        self.rotate_backups()?;

        tracing::info!("Created backup: {}", backup_path.display());
        Ok(backup_path)
    }

    /// Restore from latest backup
    pub fn restore_latest(&self) -> Result<Option<FileSystemMerkle>> {
        let mut backups: Vec<_> = std::fs::read_dir(&self.backup_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("snapshot")))
            .collect();

        backups.sort_by_key(|e| e.metadata().unwrap().modified().unwrap());
        backups.reverse();

        if let Some(latest) = backups.first() {
            tracing::info!("Restoring from backup: {}", latest.path().display());
            FileSystemMerkle::load_snapshot(&latest.path())
        } else {
            Ok(None)
        }
    }

    fn rotate_backups(&self) -> Result<()> {
        let mut backups: Vec<_> = std::fs::read_dir(&self.backup_dir)?
            .filter_map(|e| e.ok())
            .collect();

        backups.sort_by_key(|e| e.metadata().unwrap().modified().unwrap());

        while backups.len() > self.retention_count {
            if let Some(oldest) = backups.first() {
                std::fs::remove_file(oldest.path())?;
                tracing::info!("Deleted old backup: {}", oldest.path().display());
                backups.remove(0);
            }
        }

        Ok(())
    }
}

// Automatic backup on every N indexes
impl UnifiedIndexer {
    pub async fn index_directory_with_backup(
        &mut self,
        dir_path: &Path,
        backup_manager: &BackupManager,
    ) -> Result<IndexStats> {
        let stats = self.index_directory(dir_path).await?;

        // Backup every 100 indexed files
        if stats.indexed_files > 0 && stats.indexed_files % 100 == 0 {
            if let Ok(merkle) = FileSystemMerkle::from_directory(dir_path) {
                backup_manager.create_backup(&merkle).ok();
            }
        }

        Ok(stats)
    }
}
```

### Week 8: Final Polish

#### Task 8.1: Documentation
- **Effort:** 1 day

Create comprehensive docs:
- `README.md` - Updated with all features
- `DEPLOYMENT.md` - Production deployment guide
- `TROUBLESHOOTING.md` - Common issues and fixes
- `API.md` - MCP tool reference

#### Task 8.2: Production Checklist Validation
- **Effort:** 1 day

**File:** `docs/PRODUCTION_CHECKLIST.md`

```markdown
# Production Deployment Checklist

## Infrastructure
- [ ] Qdrant running (Docker or binary)
- [ ] Persistent volumes configured
- [ ] Backup strategy implemented (7 days retention)
- [ ] Resource limits set (CPU, RAM)

## Configuration
- [ ] Qdrant HNSW parameters optimized
- [ ] Tantivy memory budget configured
- [ ] RRF k value tuned
- [ ] Secrets scanner enabled
- [ ] Sensitive file filter active

## Security
- [ ] Privacy-first mode validated (local-only)
- [ ] Audit logging enabled
- [ ] No cloud dependencies
- [ ] Secrets detection tested

## Monitoring
- [ ] Health checks working
- [ ] Metrics collection enabled
- [ ] Alerts configured
- [ ] Log aggregation setup

## Testing
- [ ] Functional tests passing (100%)
- [ ] Performance benchmarks met
- [ ] Quality metrics validated
- [ ] Load testing completed

## Documentation
- [ ] README updated
- [ ] Deployment guide written
- [ ] API documentation complete
- [ ] Troubleshooting guide available
```

### Phase 4 Deliverables

- [x] Health monitoring system
- [x] Graceful degradation
- [x] Automatic backups (7 days retention)
- [x] Complete documentation
- [x] Production checklist validated

---

## Testing Strategy

### Unit Tests

```bash
# Test individual components
cargo test --lib

# Specific modules
cargo test --lib indexing::merkle
cargo test --lib security::secrets
cargo test --lib search::hybrid
```

### Integration Tests

```bash
# End-to-end workflows
cargo test --test integration_hybrid_search
cargo test --test integration_incremental_indexing
cargo test --test integration_error_recovery
```

### Performance Benchmarks

```bash
# Benchmark suite
cargo bench --bench indexing_performance
cargo bench --bench search_latency
cargo bench --bench merkle_detection

# Expected results:
# - First index (100k LOC): < 2 minutes
# - Incremental (1% change): < 5 seconds
# - Unchanged check: < 10ms
# - Search latency (p95): < 200ms
```

### Quality Evaluation

```bash
# Run evaluation suite
cargo test --test evaluation -- --nocapture

# Expected metrics (MVP):
# - NDCG@10: > 0.65
# - MRR: > 0.70
# - Recall@20: > 0.85
# - Precision@10: > 0.60
```

---

## Rollout Plan

### Stage 1: Internal Testing (After Phase 1)
- Deploy to development environment
- Test on internal Rust projects (10k-100k LOC)
- Validate hybrid search functionality
- Gather initial feedback

### Stage 2: Beta Testing (After Phase 2)
- Deploy to staging environment
- Test on large codebases (100k-1M LOC)
- Measure performance metrics
- Tune based on real workloads

### Stage 3: Quality Validation (After Phase 3)
- Run evaluation suite
- Compare metrics to targets
- Iterate on chunking/retrieval if needed
- Get user feedback on quality

### Stage 4: Production Deployment (After Phase 4)
- Deploy to production
- Enable monitoring and alerting
- Document known issues
- Provide support channel

---

## Success Criteria

### Functional Requirements

| Requirement | Target | Status |
|-------------|--------|--------|
| Hybrid search working | ✅ Yes | [ ] |
| Incremental indexing | ✅ Yes | [ ] |
| Secrets detection | ✅ Yes | [ ] |
| Health monitoring | ✅ Yes | [ ] |
| Graceful degradation | ✅ Yes | [ ] |

### Performance Metrics

| Metric | Target | Current | Status |
|--------|--------|---------|--------|
| Unchanged check | < 10ms | TBD | [ ] |
| First index (100k LOC) | < 2 min | TBD | [ ] |
| Incremental (1% change) | < 5s | TBD | [ ] |
| Search latency (p95) | < 200ms | TBD | [ ] |
| Memory usage (1M LOC) | < 4GB | TBD | [ ] |

### Quality Metrics

| Metric | MVP Target | Production Target | Current | Status |
|--------|------------|-------------------|---------|--------|
| NDCG@10 | > 0.65 | > 0.75 | TBD | [ ] |
| MRR | > 0.70 | > 0.80 | TBD | [ ] |
| Recall@20 | > 0.85 | > 0.95 | TBD | [ ] |
| Precision@10 | > 0.60 | > 0.70 | TBD | [ ] |

---

## Risk Assessment & Mitigation

### High Risk

| Risk | Impact | Likelihood | Mitigation |
|------|--------|-----------|------------|
| Qdrant unavailable | Service down | Medium | Graceful degradation to BM25-only |
| Disk space full | Indexing fails | Low | Monitor disk usage, cleanup old snapshots |
| Memory exhaustion | Crashes | Low | Configure Tantivy/Qdrant memory limits |

### Medium Risk

| Risk | Impact | Likelihood | Mitigation |
|------|--------|-----------|------------|
| Quality below target | Poor UX | Medium | Iterate on chunking, expand test dataset |
| Slow indexing | User frustration | Low | Optimize Qdrant bulk mode, parallelize |
| Secrets leaked | Security issue | Low | Strict secrets scanner, code review |

---

## Timeline Summary

```
Week 1: Fix Qdrant Population + Secrets Scanner
Week 2: Merkle Tree Implementation
Week 3: Qdrant Optimization + Bulk Mode
Week 4: Tantivy Tuning + RRF Parameter Tuning
Week 5: AST-First Chunking
Week 6: Quality Evaluation + Test Dataset
Week 7: Health Checks + Graceful Degradation + Backups
Week 8: Documentation + Production Checklist
```

**Total:** 8 weeks to production-ready system

---

## References

- **Research Documents:**
  - `docs/INDEXING_STRATEGIES.md` - Strategy options
  - `docs/COMPARISON_CLAUDE_CONTEXT.md` - Production validation
  - `docs/DEEP_RESEARCH_FINDINGS.md` - SOTA techniques
  - `docs/ADVANCED_RESEARCH.md` - Production patterns

- **External Resources:**
  - rs-merkle: https://github.com/antouhou/rs-merkle
  - Qdrant docs: https://qdrant.tech/documentation/
  - Tantivy docs: https://docs.rs/tantivy/
  - CoSQA+ benchmark: https://arxiv.org/abs/2406.11589

---

## Next Steps

**Immediate Actions:**

1. **Review this plan** with your team
2. **Set up development environment** (Qdrant, dependencies)
3. **Start with Week 1, Task 1.1** (Fix Qdrant population)
4. **Track progress** using this document as checklist
5. **Adjust timeline** based on actual velocity

**Questions to Answer:**

- [ ] Do you have access to Qdrant instance (Docker/cloud)?
- [ ] Do you have test Rust codebases for validation?
- [ ] Do you have team members to create test dataset (Phase 3)?
- [ ] What's your target production date?

---

**Document Version:** 1.0
**Last Updated:** 2025-10-19
**Status:** Ready for Implementation
**Estimated Effort:** 240-320 hours (8 weeks @ 30-40 hrs/week)

Ready to start implementing? Let me know which task you'd like to begin with!
