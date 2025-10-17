# Phase 0: Setup - Progress Notes

**Phase:** Week 1 - Setup & Forking
**Started:** 2025-10-17
**Status:** In Progress

## ✅ Completed Tasks

### Project Setup
- [x] Cloned file-search-mcp to `/home/molaco/Documents/rust-code-mcp`
- [x] Created docs directory for planning materials
- [x] Copied all planning documents:
  - NEW_PLAN.md (16-week implementation plan)
  - RUST_MCP_CODE_SEARCH_RESEARCH.md (component research)
  - STATE_OF_THE_ART_CODEBASE_ANALYSIS.md (industry analysis)
  - SEARCH.md (architecture overview)
  - PLAN.md (original from-scratch plan)
- [x] Created test/benchmark/example directories
- [x] Updated README with project vision and roadmap
- [x] Verified project builds successfully

### Directory Structure Created
```
rust-code-mcp/
├── Cargo.toml
├── README.md
├── docs/                    # ✅ Planning documents
│   ├── NEW_PLAN.md
│   ├── RUST_MCP_CODE_SEARCH_RESEARCH.md
│   ├── STATE_OF_THE_ART_CODEBASE_ANALYSIS.md
│   ├── SEARCH.md
│   ├── PLAN.md
│   └── PHASE0_NOTES.md
├── src/                     # ✅ From file-search-mcp
│   ├── main.rs
│   └── tools/
│       ├── mod.rs
│       └── search_tool.rs
├── tests/                   # ✅ Created (empty)
│   ├── integration/
│   └── fixtures/
├── benches/                 # ✅ Created (empty)
└── examples/                # ✅ Created (empty)
```

## 📋 ~~Remaining~~ Completed Phase 0 Tasks ✅

### 1. Study Existing Codebase ✅
- [x] Read and understand `src/main.rs` - MCP server setup
- [x] Read and understand `src/tools/search_tool.rs` - Tantivy integration
- [x] Document current architecture
- [x] Identify integration points for enhancements
- **Result**: Comprehensive analysis in "Detailed Codebase Analysis" section below

### 2. Clone bloop for Reference ✅
```bash
cd /home/molaco/Documents
git clone https://github.com/BloopAI/bloop.git
```
- [x] Clone bloop repository → `/home/molaco/Documents/bloop`
- [x] Study `server/bleep/src/` directory structure
- [x] Document relevant files to extract patterns from:
  - `indexes/schema.rs` - Schema design (306 lines)
  - `indexes/reader.rs` - Search patterns (11KB)
  - `semantic/embedder.rs` - Qdrant integration (330 lines)
  - `semantic/chunk.rs` - Code chunking (25KB)
  - `intelligence/code_navigation.rs` - Tree-sitter usage (540 lines)
  - `background/sync.rs` - Change detection (571 lines)

### 3. Set up Qdrant Instance ✅
- [x] ~~Docker approach~~ **Docker not available - strategy changed**
- [x] Decision: Use **embedded Qdrant** (in-process library)
- [x] Alternative: Provide Nix flake for production deployment

**Strategy**:
- **MVP (Phase 5)**: Embedded Qdrant (`qdrant = "0.4"` crate)
  - No Docker required
  - In-process vector database
  - Simpler deployment

- **Production (Phase 8+)**: Nix flake option
  - Declarative Qdrant service setup
  - Optional remote mode for scalability

- **Development**: Embedded mode is sufficient for 1M LOC target

### 4. Create Extraction Plan ✅
- [x] Document which code patterns to extract from bloop
- [x] List files to create in Phase 1
- [x] Define integration strategy between file-search-mcp and new components
- **Result**: Created comprehensive `docs/EXTRACTION_PLAN.md` (500+ lines)

## 📝 Detailed Codebase Analysis

### Architecture Overview

```
MCP Client (Claude/Cursor)
       ↓ (stdio)
   src/main.rs (24 lines)
       ↓
   SearchTool (368 lines)
       ├── read_file_content() → File reading with binary detection
       └── search() → In-memory Tantivy BM25 search
```

### File-by-File Analysis

#### **src/main.rs** (24 lines) - Entry Point
```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>>
```
- **Line 9-12**: Sets up tracing with DEBUG level to stderr (not stdout)
- **Line 17**: Creates SearchTool and serves over stdio transport
- **Line 21**: Waits for service completion
- **Key Insight**: Very clean, minimal entry point. All logic in SearchTool.

#### **src/tools/search_tool.rs** (368 lines) - Core Logic

**Structure:**
```rust
pub struct SearchTool;  // Simple unit struct, no state
```

**Tool 1: read_file_content** (Lines 37-104)
- **Input**: FileContentParams { file_path: String }
- **Output**: Result<String, String>
- **Logic**:
  1. Validates path exists and is file (lines 44-60)
  2. Tries fs::read_to_string() (line 63)
  3. On error, reads as binary and detects if binary file (lines 76-101)
     - NULL byte check (line 79)
     - Control character ratio >10% (lines 80-84)
  4. Returns error for binary files
- **Integration Point**: Could be enhanced to use cached file metadata

**Tool 2: search** (Lines 106-347) - Main Search Logic
- **Input**: SearchParams { directory: String, keyword: String }
- **Output**: Result<String, String>

**Search Flow:**
1. **Schema Definition** (Lines 109-120)
   ```rust
   path_field = "path" (STORED)
   content_field = "content" (STORED + indexed with default tokenizer)
   ```
   - ⚠️ **Integration Point**: Can add fields for:
     - `symbols` (function/struct names)
     - `doc_type` (rust/toml/md)
     - `embedding_id` (link to Qdrant)

2. **Index Creation** (Line 123)
   ```rust
   Index::create_in_ram(schema)
   ```
   - ⚠️ **CRITICAL**: Must change to `Index::open_or_create(path)` in Phase 1

3. **Index Writer** (Lines 126-128)
   ```rust
   index_writer = index.writer(50_000_000)  // 50MB buffer
   ```

4. **File Processing** (Lines 136-280)
   - **Binary Extension Blacklist** (Lines 147-152): 40+ extensions
   - **Text Detection** (Lines 155-202):
     - Extension check first
     - Reads up to 8KB sample
     - NULL byte check → binary
     - Control char ratio >30% → binary
     - UTF-8 validation
     - ASCII ratio >80% → likely text
   - **Recursive Directory Walk** (Lines 205-267)
     - ⚠️ **Integration Point**: Add file watching here (notify crate)
     - ⚠️ **Integration Point**: Check file hash before reindexing
     - Skips empty files (line 240)
     - Logs debug info for each file

5. **Search Execution** (Lines 302-321)
   ```rust
   QueryParser::for_index(&index, vec![content_field])
   searcher.search(&query, &TopDocs::with_limit(10))
   ```
   - ⚠️ **Integration Point**: Add Qdrant semantic search + RRF fusion here

6. **Result Formatting** (Lines 324-346)
   - Returns file paths with BM25 scores
   - Limit: 10 results

#### **Cargo.toml** - Dependencies
```toml
rmcp = { git = "...", features = ["server", "transport-io"] }
tantivy = "0.22.0"
tokio = { version = "1", features = ["macros", "rt", "rt-multi-thread", ...] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "std", "fmt"] }
serde = "1.0.219"
```
- ⚠️ **Phase 1 Additions Needed**:
  - `notify = "6"` (file watching)
  - `sled` or `rocksdb` (metadata cache)
  - `sha2` (file hashing)

### Key Integration Points for Enhancement

#### 1. **Persistent Index** (Phase 1 - Critical)
**Location**: search_tool.rs:123
```rust
// Current:
let index = Index::create_in_ram(schema);

// Target:
let index_path = PathBuf::from("~/.rust-code-mcp/index");
let index = Index::open_or_create(&index_path)?;
```

#### 2. **File Metadata Caching** (Phase 1)
**Location**: search_tool.rs:238-247 (inside file processing loop)
```rust
// Add before indexing:
let file_hash = sha256(&content);
let cached_hash = metadata_cache.get(path)?;
if cached_hash == Some(file_hash) {
    continue; // Skip, already indexed
}
```

#### 3. **File Watching** (Phase 1)
**Location**: New module `src/watcher.rs`
- Use `notify` crate
- Watch for Create/Modify/Delete events
- Trigger selective reindexing
- Integration: Call from search_tool.rs before search

#### 4. **Schema Extension** (Phase 2-3)
**Location**: search_tool.rs:109-120
```rust
// Add fields:
let symbol_field = schema_builder.add_text_field("symbols", ...);
let doc_type_field = schema_builder.add_text_field("doc_type", ...);
let chunk_id_field = schema_builder.add_u64_field("chunk_id", ...);
```

#### 5. **Code Parsing** (Phase 2)
**Location**: New module `src/parser.rs`
- Use tree-sitter to parse Rust files
- Extract: functions, structs, traits, impls, modules
- Store in `symbols` field

#### 6. **Semantic Chunking** (Phase 3)
**Location**: New module `src/chunker.rs`
- Use `text-splitter` with recursive chunking
- Create chunks at function/struct boundaries
- Index chunks separately with parent link

#### 7. **Hybrid Search** (Phase 5-6)
**Location**: search_tool.rs:307-321
```rust
// Add after BM25 search:
let tantivy_results = searcher.search(&query, &TopDocs::with_limit(100))?;
let qdrant_results = qdrant_client.search(embedding, limit=100)?;
let fused_results = reciprocal_rank_fusion(tantivy_results, qdrant_results)?;
```

### Current Strengths to Preserve

✅ **Clean MCP Integration**: RMCP macros work well
✅ **Smart Binary Detection**: Robust text/binary classification
✅ **Recursive Directory Walk**: Handles nested structures
✅ **Good Error Handling**: Proper Result types throughout
✅ **Logging**: Comprehensive tracing for debugging

### Current Limitations to Address

⚠️ **No State Persistence**: Index recreated on every search
⚠️ **No Incremental Updates**: Full reindex every time
⚠️ **Generic Text Search**: No code structure awareness
⚠️ **No Semantic Search**: Only lexical BM25
⚠️ **No Caching**: File content read every time
⚠️ **No Configuration**: Hardcoded paths and limits

### What Needs Enhancement

1. **Persistent Storage** (Phase 1)
   - Current: In-memory Tantivy index
   - Target: On-disk persistent index
   - Add: Metadata caching for files

2. **Incremental Updates** (Phase 1)
   - Current: Re-index everything on each search
   - Target: Watch files, only reindex changes
   - Add: notify crate for file watching

3. **Code Awareness** (Phase 2-3)
   - Current: Generic text search
   - Target: Parse Rust code, extract symbols
   - Add: tree-sitter + text-splitter

4. **Semantic Search** (Phase 4-6)
   - Current: Lexical BM25 only
   - Target: Hybrid (BM25 + vector embeddings)
   - Add: fastembed-rs + Qdrant

## 🎯 Next Steps (Week 2)

Once Phase 0 complete, begin Phase 1:

1. **Make Tantivy Index Persistent**
   - Move from in-memory to on-disk
   - Add configuration for index location
   - Implement index versioning

2. **Add File Metadata Tracking**
   - Track file hashes (SHA-256)
   - Track modified timestamps
   - Cache in RocksDB or SQLite

3. **Implement Basic Change Detection**
   - Integrate notify crate
   - Detect file create/modify/delete
   - Trigger selective reindexing

## 📚 Reference Materials

### Key Documentation to Read
- [Tantivy Book](https://docs.rs/tantivy/) - Understand indexing/search
- [RMCP SDK Docs](https://github.com/modelcontextprotocol/rust-sdk) - MCP protocol
- [Qdrant Docs](https://qdrant.tech/documentation/) - Vector database (for later)

### Example Code to Study
- file-search-mcp source (current project)
- bloop `server/bleep/src/` (patterns to extract)
- [Qdrant demo-code-search](https://github.com/qdrant/demo-code-search) (reference impl)

## 💡 Design Decisions

### Why Keep file-search-mcp Structure?
- ✅ MCP server already working
- ✅ Tantivy integration proven
- ✅ Clean, simple codebase
- ✅ Easy to extend vs rewrite

### Why Single Crate (Not Workspace)?
- ✅ Simpler for MVP
- ✅ Faster iteration
- ✅ file-search-mcp is single crate
- ✅ Can split into workspace in v0.2 if needed

### Why Extract from bloop vs Fork?
- ✅ bloop has desktop app we don't need
- ✅ bloop is large and archived
- ✅ We just need specific patterns (Qdrant, tree-sitter, RRF)
- ✅ Cleaner to integrate patterns into file-search-mcp

## ⏱️ Time Tracking

**Week 1 Goal:** Complete Phase 0 setup ✅
**Time Spent:** ~4 hours total
  - Initial setup: ~2 hours (project setup, documentation)
  - Codebase study: ~1 hour (main.rs, search_tool.rs, Cargo.toml analysis)
  - Bloop analysis: ~30 min (clone, directory exploration, key file review)
  - Extraction planning: ~30 min (comprehensive extraction plan document)

**Phase 0 Status:** ✅ **COMPLETE**
**Phase 0 Completion:** 2025-10-17 (Week 1, Day 1)

## 🎯 Phase 0 Deliverables

✅ **Completed:**
1. Forked and set up rust-code-mcp project
2. Comprehensive codebase analysis documented
3. Cloned bloop for pattern reference
4. Created detailed extraction plan
5. Identified all integration points
6. Updated README with project vision

✅ **Strategy Updated:**
- Qdrant: Will use embedded mode (no Docker needed) + optional Nix flake for production

## 🚀 Ready for Phase 1

**Phase 1 starts:** Week 2
**Phase 1 goals:** Persistent index + incremental updates
**Phase 1 duration:** 3 weeks (Weeks 2-4)

**First steps:**
1. Update Cargo.toml with Phase 1 dependencies (notify, sled, sha2)
2. Create src/schema.rs based on bloop patterns
3. Make Tantivy index persistent
4. Implement metadata cache
5. Add file watching

---

**Last Updated:** 2025-10-17
**Phase 0:** ✅ Complete
**Next Phase:** Phase 1 (Weeks 2-4) - Persistent Index
