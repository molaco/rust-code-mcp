# MCP Server - Architecture Diagrams & Flow

## 1. System Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                                                                          │
│                    RUST CODE MCP SYSTEM ARCHITECTURE                    │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                           MCP CLIENT LAYER                              │
│  (Claude AI, VS Code, IDE, or any MCP-compatible client)                │
│                                                                          │
│  Sends:  JSON-RPC 2.0 requests (tools/call)                            │
│  Receives: JSON-RPC 2.0 responses                                       │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
                    Newline-delimited JSON
                               │
┌──────────────────────────────┴──────────────────────────────────────────┐
│                      STDIO TRANSPORT LAYER                              │
│                     (JSON-RPC 2.0 Protocol)                             │
│                                                                          │
│  stdin  → Receives tool call requests                                   │
│  stdout → Sends tool results                                            │
│  stderr → Logs via tracing                                              │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
┌──────────────────────────────┴──────────────────────────────────────────┐
│                    RMCP FRAMEWORK LAYER                                 │
│           (Rust SDK for Model Context Protocol)                         │
│                                                                          │
│  ┌──────────────────────────────────────────────────────────┐           │
│  │ JSON-RPC Parser                                          │           │
│  │ - Deserialize request                                    │           │
│  │ - Route to correct tool handler                          │           │
│  │ - Type-check parameters                                  │           │
│  └────────┬─────────────────────────────────────────────────┘           │
│           │                                                              │
│  ┌────────▼─────────────────────────────────────────────────┐           │
│  │ Tool Router (Macro-Generated)                            │           │
│  │ - Route by tool name                                     │           │
│  │ - Extract Parameters<T>                                  │           │
│  │ - Invoke correct method                                  │           │
│  └────────┬─────────────────────────────────────────────────┘           │
│           │                                                              │
│  ┌────────▼─────────────────────────────────────────────────┐           │
│  │ JSON Serializer                                          │           │
│  │ - Serialize results to JSON-RPC response                 │           │
│  │ - Handle errors                                          │           │
│  └────────┬─────────────────────────────────────────────────┘           │
└───────────┼──────────────────────────────────────────────────────────────┘
            │
┌───────────▼──────────────────────────────────────────────────────────────┐
│                    SEARCH TOOL IMPLEMENTATION                            │
│                    (Tools via #[tool_router] macro)                      │
│                                                                          │
│  ┌────────────────────────────────────────────────────────┐             │
│  │ pub struct SearchTool {                                │             │
│  │   tool_router: ToolRouter<Self>,                       │             │
│  │   sync_manager: Option<Arc<SyncManager>>,  ◄───┐      │             │
│  │ }                                              │      │             │
│  └────────────────────────────────────────────────┼──────┘             │
│                                                   │                     │
│  Tool Methods (decorated with #[tool]):          │                     │
│  ┌─────────────────────────────────────┐         │                     │
│  │ read_file_content()                 │         │                     │
│  │ search()                            │         │                     │
│  │ find_definition()                   │         │                     │
│  │ find_references()                   │         │                     │
│  │ get_dependencies()                  │         │                     │
│  │ get_call_graph()                    │         │                     │
│  │ analyze_complexity()                │         │                     │
│  │ get_similar_code()                  │         │                     │
│  │ index_codebase()                    │         │                     │
│  │ health_check()                      │         │                     │
│  └─────────────────────────────────────┘         │                     │
│                                                   │                     │
│  ServerHandler Implementation:                   │                     │
│  ┌─────────────────────────────────────┐         │                     │
│  │ get_info() -> ServerInfo            │         │                     │
│  │   - Protocol version                │         │                     │
│  │   - Server capabilities             │         │                     │
│  │   - Tool descriptions               │         │                     │
│  └─────────────────────────────────────┘         │                     │
└───────────────────────────────────────────────────┼─────────────────────┘
                                                    │
                ┌───────────────────────────────────┘
                │
┌───────────────▼──────────────────────────────────────────────────────────┐
│             CORE PROCESSING & INDEXING LAYER                             │
│                                                                          │
│  ┌──────────────────────────┐   ┌──────────────────────────┐            │
│  │  UNIFIED INDEXER         │   │  INCREMENTAL INDEXER     │            │
│  │  (Full Index Workflow)   │   │  (Change Detection)      │            │
│  │                          │   │                          │            │
│  │ • Indexes directory      │   │ • Detects file changes   │            │
│  │ • Creates BM25 index     │   │ • Updates indices only   │            │
│  │ • Creates Vector store   │   │ • Uses Merkle tree       │            │
│  │ • Generates embeddings   │   │ • Reuses unchanged data  │            │
│  │ • Chunks code            │   │ • Fast (< 10ms if clean) │            │
│  │                          │   │                          │            │
│  │ Used by:                 │   │ Used by:                 │            │
│  │ - search()               │   │ - index_codebase()       │            │
│  │ - get_similar_code()     │   │ - Background sync        │            │
│  └──────────┬───────────────┘   └────────┬─────────────────┘            │
│             │                            │                              │
│             │            ┌───────────────┘                              │
│             │            │                                              │
│  ┌──────────▼────────────▼──────┐   ┌──────────────────┐               │
│  │   TANTIVY BM25 INDEX          │   │   QDRANT         │               │
│  │   (Full-Text Search)          │   │   VECTOR STORE   │               │
│  │                               │   │                  │               │
│  │ • Keyword indexing            │   │ • Vector search  │               │
│  │ • TF-IDF scoring              │   │ • Semantic match │               │
│  │ • Position tracking           │   │ • HNSW index     │               │
│  │ • Persistent storage          │   │ • 384-dim embed  │               │
│  │   ({index}/{hash}/)           │   │   (all-MiniLM)   │               │
│  └──────────────────────────────┘   └──────────────────┘               │
│                                                                          │
│  ┌────────────────────────────────────────────────────────┐             │
│  │   MERKLE TREE CHANGE DETECTION                         │             │
│  │   (Snapshots/{hash}/)                                  │             │
│  │                                                         │             │
│  │ • File hash tracking                                   │             │
│  │ • Fast change detection (< 10ms)                       │             │
│  │ • Incremental indexing basis                           │             │
│  │ • Force reindex capability                             │             │
│  └────────────────────────────────────────────────────────┘             │
│                                                                          │
│  ┌────────────────────────────────────────────────────────┐             │
│  │   HYBRID SEARCH ENGINE                                 │             │
│  │                                                         │             │
│  │ 1. Generate query embedding (fastembed)                │             │
│  │ 2. BM25 search on keywords (Tantivy)                   │             │
│  │ 3. Vector search on semantics (Qdrant)                 │             │
│  │ 4. Reciprocal Rank Fusion (RRF) combine                │             │
│  │ 5. Return top-10 results                               │             │
│  └────────────────────────────────────────────────────────┘             │
└──────────────────────────────────────────────────────────────────────────┘
                               │
┌──────────────────────────────┴──────────────────────────────────────────┐
│            BACKEND INFRASTRUCTURE LAYER                                 │
│                                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                  │
│  │ EMBEDDINGS   │  │ RUST PARSER  │  │ CODE CHUNKER │                  │
│  │              │  │              │  │              │                  │
│  │ • fastembed  │  │ • tree-sitter│  │ • Semantic   │                  │
│  │ • all-MiniLM │  │ • Rust gram  │  │   splitting  │                  │
│  │ • 384-dim    │  │ • AST parse  │  │ • Symbol     │                  │
│  │   vectors    │  │ • Symbol def │  │   extraction │                  │
│  │              │  │ • Imports    │  │ • Docstrings │                  │
│  └──────────────┘  └──────────────┘  └──────────────┘                  │
│                                                                          │
│  ┌──────────────────────────────────────────────────────┐               │
│  │ METADATA CACHE (sled embedded DB)                     │               │
│  │                                                       │               │
│  │ • File hashes (SHA-256)                               │               │
│  │ • Modification times                                  │               │
│  │ • File sizes                                          │               │
│  │ • Path tracking                                       │               │
│  │ Location: {cache}/{hash}/                            │               │
│  └──────────────────────────────────────────────────────┘               │
│                                                                          │
│  ┌──────────────────────────────────────────────────────┐               │
│  │ BACKGROUND SYNC MANAGER (Arc<SyncManager>)           │               │
│  │                                                       │               │
│  │ • Tracks directories in RwLock<HashSet>              │               │
│  │ • 5-minute periodic sync                             │               │
│  │ • Spawns tokio background task                       │               │
│  │ • Uses IncrementalIndexer                            │               │
│  │ • Auto-reindexes detected changes                    │               │
│  └──────────────────────────────────────────────────────┘               │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Tool Execution Flow

```
┌─────────────────────────────────────────────────────────────┐
│                                                              │
│              TOOL EXECUTION FLOW (Detailed)                 │
│                                                              │
└─────────────────────────────────────────────────────────────┘

USER INPUT
   │
   └─→ Tool Call Request (JSON-RPC)
       {
         "method": "tools/call",
         "params": {
           "name": "search",
           "arguments": {
             "directory": "/home/user/project",
             "keyword": "error"
           }
         }
       }
       │
       ▼
┌──────────────────────────────────────────┐
│ RMCP Transport::Stdio::recv_json()       │
│ Read from stdin, parse JSON-RPC          │
└────────────┬─────────────────────────────┘
             │
             ▼
┌──────────────────────────────────────────┐
│ RMCP Framework                           │
│ - Deserialize request                    │
│ - Extract tool name & arguments          │
│ - Route to tool handler                  │
└────────────┬─────────────────────────────┘
             │
             ▼
┌──────────────────────────────────────────┐
│ ToolRouter::route() [macro-generated]    │
│ Match tool name: "search"                │
│ Deserialize SearchParams from JSON       │
└────────────┬─────────────────────────────┘
             │
             ▼
┌──────────────────────────────────────────┐
│ SearchTool::search(                      │
│   &self,                                 │
│   Parameters(SearchParams {              │
│     directory,                           │
│     keyword                              │
│   })                                     │
│ ) -> Result<CallToolResult, McpError>   │
└────────────┬─────────────────────────────┘
             │
    ┌────────┴──────────┐
    │ EXECUTION STEPS    │
    │                    │
    ▼                    ▼
1. INPUT VALIDATION
   ├─ Check directory exists
   ├─ Check is_dir()
   └─ Validate keyword not empty
          │
          ├──► Error: return McpError::invalid_params()
          │
          └──► Success: continue
                    │
                    ▼
2. COLLECTION NAME GENERATION
   ├─ Hash directory path (SHA-256)
   ├─ Take first 8 chars of hash
   └─ Format: "code_chunks_{hash}"
                    │
                    ▼
3. INITIALIZE INDEXER
   ├─ UnifiedIndexer::new()
   │  ├─ Create EmbeddingGenerator (fastembed)
   │  ├─ Create VectorStore (Qdrant client)
   │  └─ Open/create Tantivy indices
   │
   └─ Connect to Qdrant at QDRANT_URL
                    │
                    ▼
4. INDEX DIRECTORY
   ├─ UnifiedIndexer::index_directory()
   │  ├─ Traverse .rs files recursively
   │  ├─ Parse each file (tree-sitter)
   │  ├─ Extract symbols (functions, structs, etc.)
   │  ├─ Generate code chunks
   │  ├─ Create embeddings (batch)
   │  ├─ Push to Tantivy index
   │  ├─ Push to Qdrant
   │  └─ Return IndexStats
   │
   └─ Stats: {indexed_files, total_chunks, unchanged_files, skipped_files}
                    │
                    ▼
5. TRACK FOR BACKGROUND SYNC
   ├─ Check self.sync_manager
   │  │
   │  ├─ if Some(sync_mgr):
   │  │   sync_mgr.track_directory(dir).await
   │  │   (next sync cycle will auto-reindex)
   │  │
   │  └─ if None: (background sync disabled)
                    │
                    ▼
6. CREATE HYBRID SEARCH ENGINE
   ├─ Generate embedding for keyword
   │  "error" → [0.12, -0.45, ..., 0.89] (384 dims)
   │
   ├─ Create HybridSearch instance
   │  ├─ EmbeddingGenerator (from indexer)
   │  ├─ VectorStore (from indexer)
   │  └─ BM25Search (Tantivy-based)
   │
   └─ Ready to search
                    │
                    ▼
7. EXECUTE SEARCH
   ├─ BM25 search:
   │  ├─ QueryParser tokenize "error"
   │  ├─ Search Tantivy index
   │  ├─ Get top-20 BM25 results with scores
   │  └─ Results: Vec<(DocID, BM25Score)>
   │
   ├─ Vector search:
   │  ├─ Query Qdrant nearest neighbors
   │  ├─ k=20
   │  ├─ Get top-20 vector results with scores
   │  └─ Results: Vec<(ChunkID, VectorScore)>
   │
   ├─ Reciprocal Rank Fusion:
   │  ├─ Combine rankings from both searches
   │  ├─ RRF formula: 1/(k + rank)
   │  ├─ Sum scores for each chunk
   │  └─ Sort by combined score (descending)
   │
   └─ Return top-10 final results
                    │
                    ▼
8. FORMAT RESPONSE
   ├─ For each result:
   │  ├─ File path
   │  ├─ Symbol name & kind
   │  ├─ Line numbers
   │  ├─ Score (0-1)
   │  └─ Code preview (first 3 lines)
   │
   ├─ Add indexing stats:
   │  ├─ Files indexed
   │  ├─ Total chunks
   │  ├─ Unchanged files
   │  └─ Skipped files
   │
   └─ Format as text string
                    │
                    ▼
9. RETURN SUCCESS RESULT
   Ok(CallToolResult::success(vec![
     Content::text(formatted_response)
   ]))
                    │
                    ▼
10. RMCP FRAMEWORK WRAPPING
    ├─ Wrap result in JSON-RPC response
    ├─ Serialize to JSON
    └─ Format: {"jsonrpc":"2.0","id":1,"result":{...}}
                    │
                    ▼
11. TRANSPORT OUTPUT
    ├─ Write to stdout (newline-delimited)
    └─ MCP Client receives response
                    │
                    ▼
    USER SEES RESULTS
    ├─ Found X results
    ├─ Score, file, symbol for each
    └─ Code preview
```

---

## 3. Background Sync Workflow

```
┌────────────────────────────────────────┐
│  APPLICATION START (main.rs)           │
│                                        │
│  1. Initialize logging                 │
│  2. Create SyncManager                 │
│     - interval: 300s (5 min)          │
│     - tracked_dirs: empty set         │
│  3. Spawn background task             │
│     tokio::spawn(sync_manager.run())  │
│  4. Create SearchTool                 │
│     - inject sync_manager             │
│  5. Bind to stdio transport           │
│  6. Start serving requests            │
└────────────┬───────────────────────────┘
             │
             ├─────────────────────┬──────────────────────────┐
             │                     │                          │
    [MAIN TASK]            [BACKGROUND SYNC TASK]    [SERVES REQUESTS]
             │                     │                          │
             │              Wait 5 seconds                    │
             │                     │                          │
             │                     ▼                          │
             │           handle_sync_all()                    │
             │           (first sync)                         │
             │           ├─ Get tracked_dirs                 │
             │           ├─ Check each dir                   │
             │           └─ Log results                       │
             │                     │                          │
             │                     ▼                          │
             │              Create interval                   │
             │              timer (300s)                      │
             │                     │                          │
             │                     │ Wait 300s                │
             │                     ▼                          │
             │            interval.tick()                     │
             │                     │                          │
             │                     ▼                          │
             │           handle_sync_all()                    │
             │           (periodic sync)                      │
             │           ├─ Get tracked_dirs                 │
             │           │                                    │
             │           │ FOR EACH DIR:                      │
             │           │ ├─ Hash directory path            │
             │           │ ├─ Create IncrementalIndexer      │
             │           │ ├─ Check Merkle snapshot          │
             │           │ │  ├─ No file changes?            │
             │           │ │  │  └─ Skip (< 10ms)            │
             │           │ │  └─ File changed?               │
             │           │ │     └─ Reindex that file        │
             │           │ ├─ Update embeddings if needed    │
             │           │ ├─ Push to Qdrant                 │
             │           │ └─ Log stats                       │
             │           │                                    │
             │           └─ Log sync complete                │
             │                     │                          │
             │                     └──── Wait 300s ────┐      │
             │                                         │      │
             │                    ┌────────────────────┘      │
             │                    │                           │
             │                    ▼                           │
             │           interval.tick()                      │
             │                  (repeat forever)              │
             │                                                │
             │                     TOOLS MAY:                 │
             │                     ├─ Call search()           │
             │                     ├─ Call index_codebase()   │
             │                     ├─ Call get_similar_code() │
             │                     │                          │
             │                     └──► IF SUCCESSFUL:        │
             │                           sync_mgr.            │
             │                           track_directory(dir) │
             │                           (adds to tracked set)│
             │                                                │
             └──────────────────────────────────────────────┘

EXAMPLE TIMELINE:

Time  Event
────────────────────────────────────────────
0s    Main: Create SyncManager
      Background: Spawn task
      Main: Create SearchTool
      Main: Start serving

5s    Background: First sync (no dirs tracked)

10s   Client: Call search("/home/user/project")
      Main: SearchTool::search()
      Main: Indexing...
      Main: Track directory for sync
      Main: Return results

...

305s  Background: Periodic sync
      Background: Check /home/user/project
      Background: Merkle tree says "no changes"
      Background: Skip indexing (< 10ms)
      Background: Log: "No changes in /home/user/project"

...

610s  Background: Periodic sync
      Background: Check /home/user/project
      Background: User modified src/main.rs
      Background: Merkle tree detects change
      Background: Reindex changed file only
      Background: Push new embeddings to Qdrant
      Background: Update Merkle snapshot
      Background: Log: "Synced /home/user/project: 1 file"

...     [Repeats every 300s]
```

---

## 4. Tool Dependency Graph

```
┌─────────────────────────────────────────────────────────────┐
│                    TOOL DEPENDENCIES                        │
└─────────────────────────────────────────────────────────────┘

INDEPENDENT TOOLS (No dependencies on other tools):
  │
  ├─► read_file_content
  │   └─ Input: file_path
  │   └─ Output: file text
  │   └─ Dependencies: std::fs
  │
  ├─► find_definition
  │   └─ Input: symbol_name, directory
  │   └─ Output: definition locations
  │   └─ Dependencies: RustParser
  │
  ├─► find_references
  │   └─ Input: symbol_name, directory
  │   └─ Output: reference locations
  │   └─ Dependencies: RustParser
  │
  ├─► get_dependencies
  │   └─ Input: file_path
  │   └─ Output: imports list
  │   └─ Dependencies: RustParser
  │
  ├─► get_call_graph
  │   └─ Input: file_path, symbol_name?
  │   └─ Output: function call graph
  │   └─ Dependencies: RustParser
  │
  └─► analyze_complexity
      └─ Input: file_path
      └─ Output: code metrics
      └─ Dependencies: RustParser

DEPENDENT TOOLS (Use indexing infrastructure):
  │
  ├─► search
  │   ├─ Input: directory, keyword
  │   ├─ Output: search results
  │   ├─ Dependencies:
  │   │  ├─ UnifiedIndexer
  │   │  ├─ EmbeddingGenerator
  │   │  ├─ VectorStore (Qdrant)
  │   │  ├─ BM25Search (Tantivy)
  │   │  └─ HybridSearch
  │   └─ Side effect: tracks directory in SyncManager
  │
  ├─► get_similar_code
  │   ├─ Input: query, directory, limit
  │   ├─ Output: similar code snippets
  │   ├─ Dependencies:
  │   │  ├─ UnifiedIndexer
  │   │  ├─ EmbeddingGenerator
  │   │  └─ VectorStore (Qdrant)
  │   └─ Side effect: tracks directory in SyncManager
  │
  ├─► index_codebase
  │   ├─ Input: directory, force_reindex?
  │   ├─ Output: indexing results
  │   ├─ Dependencies:
  │   │  ├─ IncrementalIndexer
  │   │  ├─ Merkle tree operations
  │   │  └─ VectorStore (Qdrant)
  │   └─ Side effect: tracks directory in SyncManager
  │
  └─► health_check
      ├─ Input: directory?
      ├─ Output: health status
      ├─ Dependencies:
      │  ├─ BM25Search
      │  ├─ VectorStore
      │  ├─ HealthMonitor
      │  └─ Merkle snapshots
      └─ Side effect: none

SyncManager (Background):
  │
  ├─► Tracks directories from tools
  ├─► Runs IncrementalIndexer periodically
  ├─► Uses Merkle tree for change detection
  └─► Updates indices in Qdrant & Tantivy
```

---

## 5. Directory Hash-Based Collection Strategy

```
┌────────────────────────────────────────────────────────────┐
│     DIRECTORY HASH & COLLECTION NAMING STRATEGY            │
└────────────────────────────────────────────────────────────┘

User provides: /home/user/my-project

Step 1: Hash Directory Path
  Input: "/home/user/my-project"
  Algorithm: SHA-256
  Process:
    - Convert path to string
    - Apply SHA256
    - Get hex digest
  Output: "a3f5c7e8d1b4f6a9c2e5d7b4f1a8c3e6d9f2a5b8c1e4f7d0a3b6c9e2f5a8b1"
          (64-character hex string)

Step 2: Truncate to First 8 Characters
  Input: "a3f5c7e8d1b4f6a9c2e5d7b4f1a8c3e6d9f2a5b8c1e4f7d0a3b6c9e2f5a8b1"
  Output: "a3f5c7e8"

Step 3: Generate Collection Name
  Template: "code_chunks_{hash}"
  Input: "a3f5c7e8"
  Output: "code_chunks_a3f5c7e8"

Step 4: Generate Path Names
  
  Cache directory:
    Base: ~/.local/share/rust-code-mcp/search/
    Path: ~/.local/share/rust-code-mcp/search/cache/a3f5c7e8/
    
  Index directory:
    Base: ~/.local/share/rust-code-mcp/search/
    Path: ~/.local/share/rust-code-mcp/search/index/a3f5c7e8/
    
  Merkle snapshot:
    Path: ~/.local/share/rust-code-mcp/search/snapshots/a3f5c7e8/merkle.snapshot

CONSISTENCY ACROSS TOOLS:

  Tool 1 (search):
    ├─ Input: "/home/user/my-project"
    ├─ Hash: "a3f5c7e8"
    └─ Collection: "code_chunks_a3f5c7e8"
    
  Tool 2 (index_codebase):
    ├─ Input: "/home/user/my-project"
    ├─ Hash: "a3f5c7e8" ◄── SAME
    └─ Collection: "code_chunks_a3f5c7e8" ◄── SAME
    
  Tool 3 (get_similar_code):
    ├─ Input: "/home/user/my-project"
    ├─ Hash: "a3f5c7e8" ◄── SAME
    └─ Collection: "code_chunks_a3f5c7e8" ◄── SAME
    
  Tool 4 (health_check):
    ├─ Input: "/home/user/my-project"
    ├─ Hash: "a3f5c7e8" ◄── SAME
    └─ Collection: "code_chunks_a3f5c7e8" ◄── SAME

  Background Sync:
    ├─ Directory: "/home/user/my-project"
    ├─ Hash: "a3f5c7e8" ◄── SAME
    └─ Collection: "code_chunks_a3f5c7e8" ◄── SAME

BENEFITS:

1. No Conflicts
   - Different projects get different collections
   - /home/user/project1 → code_chunks_xyz123ab
   - /home/user/project2 → code_chunks_def456cd
   - /work/project3 → code_chunks_ghi789ef

2. Deterministic
   - Same directory always produces same hash
   - Repeated calls use same collection
   - Consistent across tool calls

3. Multi-Project Support
   - Can index multiple projects simultaneously
   - Each project has isolated indices
   - Qdrant maintains separate collections

4. Merkle Tree Alignment
   - Snapshot path includes hash
   - IncrementalIndexer finds correct snapshot
   - Change detection works correctly

5. Background Sync Alignment
   - SyncManager tracks by directory
   - IncrementalIndexer uses same hash
   - Indices stay in sync
```

---

## 6. Error Handling Flow

```
┌────────────────────────────────────────────────┐
│         ERROR HANDLING & RECOVERY               │
└────────────────────────────────────────────────┘

Tool Execution
       │
       ▼
┌─────────────────────┐
│ INPUT VALIDATION    │
└──────┬──────────────┘
       │
       ├─ Path doesn't exist?
       │  └─► Err(McpError::invalid_params(...))
       │      └─► JSON-RPC error response
       │          └─► Client sees error message
       │
       ├─ Empty keyword?
       │  └─► Err(McpError::invalid_params(...))
       │      └─► Same as above
       │
       └─ All valid?
          └─► Continue to processing

                      │
                      ▼
┌─────────────────────────────────────┐
│ PROCESSING (e.g., Indexing)        │
└──────┬────────────────────────────────┘
       │
       ├─ UnifiedIndexer::new() fails?
       │  └─► McpError::invalid_params("Failed to init indexer: ...")
       │
       ├─ Qdrant connection fails?
       │  └─► McpError::invalid_params("Failed to connect to Qdrant: ...")
       │
       ├─ Tantivy index corrupt?
       │  └─► McpError::invalid_params("Failed to open index: ...")
       │
       ├─ File read error?
       │  └─► McpError::invalid_params("Failed to parse: ...")
       │
       └─ All success?
          └─► Continue to response formatting

                      │
                      ▼
┌─────────────────────────────────────┐
│ RESPONSE FORMATTING                 │
└──────┬────────────────────────────────┘
       │
       ├─ No results?
       │  └─► Ok(CallToolResult::success(
       │       vec![Content::text("No results found")]))
       │
       └─ Results generated?
          └─► Ok(CallToolResult::success(
               vec![Content::text(formatted_results)]))

                      │
                      ▼
┌─────────────────────────────────────┐
│ RMCP FRAMEWORK WRAPPING             │
└──────┬────────────────────────────────┘
       │
       ├─ Success case:
       │  └─► {"jsonrpc":"2.0","result":{...}}
       │
       └─ Error case:
          └─► {"jsonrpc":"2.0","error":{
                "code":-32602,
                "message":"Invalid params",
                "data":{"details":"..."}
              }}

                      │
                      ▼
┌─────────────────────────────────────┐
│ TRANSPORT OUTPUT                    │
└──────────────────────────────────────┘
       │
       ├─ Write to stdout
       └─ Client receives and displays to user

BACKGROUND SYNC ERROR HANDLING:

sync_directory() fails?
  ├─► Log: tracing::error!("Failed to sync {}: {}", dir, e)
  ├─► Continue to next directory
  ├─► Don't crash or stop background task
  └─► User can investigate logs and retry manually

SyncManager unrecoverable error?
  ├─► Log error
  ├─► Background task continues running
  └─► Tools can still be called directly

RECOVERY STRATEGIES:

1. User sees error message → Adjust input
2. Tools can be retried → No permanent damage
3. Force reindex available → "reset" via index_codebase(force=true)
4. Health check available → Diagnose issues
5. Logging → Debug issues offline
```

---

## Summary

This architecture provides:
- Clean separation of concerns
- Macro-based tool registration for maintainability  
- Async/concurrent execution via Tokio
- Deterministic multi-project support via hash-based collections
- Efficient change detection via Merkle trees
- Robust error handling with clear error messages
- Background sync for automatic updates
- Extensible design for adding new tools

