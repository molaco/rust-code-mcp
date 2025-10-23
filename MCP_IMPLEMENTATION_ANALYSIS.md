# MCP (Model Context Protocol) Server Implementation Analysis
## Rust Code Search MCP - Complete Technical Architecture

**Project**: rust-code-mcp (file-search-mcp)  
**Framework**: RMCP (Rust SDK for Model Context Protocol)  
**Language**: Rust  
**Async Runtime**: Tokio  
**Current Version**: 0.1.0

---

## Table of Contents
1. [Architecture Overview](#architecture-overview)
2. [MCP Server Setup & Initialization](#mcp-server-setup--initialization)
3. [Tool Definitions & Registration](#tool-definitions--registration)
4. [Request/Response Handling](#requestresponse-handling)
5. [Transport Layer (Stdio)](#transport-layer-stdio)
6. [Tool Structure & Implementation](#tool-structure--implementation)
7. [Background Sync Integration](#background-sync-integration)
8. [Data Flow Diagram](#data-flow-diagram)
9. [Key Design Patterns](#key-design-patterns)

---

## Architecture Overview

This MCP server implements a comprehensive code search and analysis platform using a **modular, layered architecture**:

```
┌─────────────────────────────────────────────────────────────┐
│                    MCP Protocol Layer                        │
│                  (rmcp crate - Rust SDK)                     │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────┴──────────────────────────────────┐
│                   Transport Layer                            │
│              stdio() - Standard I/O communication             │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────┴──────────────────────────────────┐
│            SearchTool Implementation Layer                   │
│  (ServerHandler + Tool Router with tool_router! macro)      │
│                                                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Tool Handler Methods (decorated with #[tool])       │   │
│  │ - read_file_content                                 │   │
│  │ - search (hybrid BM25 + vector)                      │   │
│  │ - find_definition                                   │   │
│  │ - find_references                                   │   │
│  │ - get_dependencies                                  │   │
│  │ - get_call_graph                                    │   │
│  │ - analyze_complexity                                │   │
│  │ - get_similar_code (semantic search)                │   │
│  │ - index_codebase (manual indexing)                  │   │
│  │ - health_check                                      │   │
│  └─────────────────────────────────────────────────────┘   │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────┴──────────────────────────────────┐
│          Core Processing & Indexing Layer                   │
│                                                              │
│  ┌──────────────────┐  ┌──────────────────┐                │
│  │ Unified Indexer  │  │ Incremental      │                │
│  │ (Full Index)     │  │ Indexer          │                │
│  │                  │  │ (Change Detection)               │
│  └────────┬─────────┘  └────────┬─────────┘                │
│           │                     │                           │
│  ┌────────┴─────────┬───────────┴───────┐                  │
│  │   Tantivy BM25   │   Qdrant Vector   │  Merkle Tree    │
│  │   Full-Text Index│   Semantic Search │  Change Detection│
│  └────────┬─────────┴────────┬──────────┘                  │
│           │                  │                              │
└───────────┴──────────────────┴──────────────────────────────┘
                           │
┌──────────────────────────┴──────────────────────────────────┐
│           Backend Infrastructure Layer                      │
│                                                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐    │
│  │ Embeddings  │  │  Parser     │  │ Chunker &       │    │
│  │ (fastembed) │  │ (tree-sitter)  │ Metadata        │    │
│  │ all-MiniLM  │  │ Rust grammar│  │ (sled cache)    │    │
│  └─────────────┘  └─────────────┘  └─────────────────┘    │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ Background Sync Manager (Arc<SyncManager>)           │   │
│  │ - Tracks directories for automatic reindexing        │   │
│  │ - 5-minute interval sync (configurable)              │   │
│  │ - Uses IncrementalIndexer for fast change detection  │   │
│  └──────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────┘
```

---

## MCP Server Setup & Initialization

### Main Entry Point (`main.rs`)

**File**: `/home/molaco/Documents/rust-code-mcp/src/main.rs`

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize tracing/logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    // 2. Create background sync manager
    // Syncs every 5 minutes (300 seconds)
    let sync_manager = Arc::new(SyncManager::with_defaults(300));
    
    // 3. Start background sync task
    let sync_manager_clone = Arc::clone(&sync_manager);
    tokio::spawn(async move {
        sync_manager_clone.run().await;
    });
    
    // 4. Create SearchTool with sync manager
    let service = SearchTool::with_sync_manager(Arc::clone(&sync_manager))
        .serve(stdio())  // Connect to stdio transport
        .await?;
    
    // 5. Wait for service completion
    service.waiting().await?;
    Ok(())
}
```

### Key Initialization Steps:

1. **Logging Setup**: Uses `tracing-subscriber` for structured logging to stderr
2. **Sync Manager Creation**: Initializes with default configuration (300s interval, XDG-compliant paths)
3. **Background Task**: Spawns tokio task for periodic sync operations
4. **Service Creation**: Creates SearchTool instance with sync manager
5. **Transport Binding**: Connects to stdio for MCP protocol communication
6. **Blocking Wait**: Service waits for incoming requests

### Dependency Injection Pattern

The `Arc<SyncManager>` is passed through the system:
- Main creates and spawns it as background task
- Passed to `SearchTool` constructor
- Accessible in tool handlers via `self.sync_manager`
- Allows automatic tracking of indexed directories

---

## Tool Definitions & Registration

### Tool Registration Mechanism

**RMCP Framework**: Uses `#[tool_router]` macro to generate tool routing

```rust
#[tool_router]
impl SearchTool {
    #[tool(description = "Tool description")]
    async fn tool_name(
        &self,
        Parameters(params): Parameters<ToolParams>,
    ) -> Result<CallToolResult, McpError> {
        // Implementation
    }
}

#[tool_handler]
impl ServerHandler for SearchTool {
    fn get_info(&self) -> ServerInfo { /* ... */ }
}
```

### Complete Tool List (10 Tools)

**File**: `/home/molaco/Documents/rust-code-mcp/src/tools/search_tool.rs`

| # | Tool Name | Purpose | Parameters |
|---|-----------|---------|------------|
| 1 | `read_file_content` | Read file contents | `file_path: String` |
| 2 | `search` | Hybrid search (BM25 + vector) | `directory: String, keyword: String` |
| 3 | `find_definition` | Locate symbol definition | `symbol_name: String, directory: String` |
| 4 | `find_references` | Find all symbol references | `symbol_name: String, directory: String` |
| 5 | `get_dependencies` | Analyze import dependencies | `file_path: String` |
| 6 | `get_call_graph` | Show function call relationships | `file_path: String, symbol_name: Option<String>` |
| 7 | `analyze_complexity` | Calculate code metrics | `file_path: String` |
| 8 | `get_similar_code` | Semantic similarity search | `query: String, directory: String, limit: Option<usize>` |
| 9 | `index_codebase` | Manual incremental indexing | `directory: String, force_reindex: Option<bool>` |
| 10 | `health_check` | System health status | `directory: Option<String>` |

### Parameter Structures (schemars + serde)

Example: SearchParams
```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Path to the directory to search")]
    pub directory: String,
    #[schemars(description = "Keyword to search for")]
    pub keyword: String,
}
```

All parameters:
- Use `serde::Deserialize` for JSON deserialization
- Use `schemars::JsonSchema` for automatic schema generation
- Wrapped in `Parameters<T>` for extraction

---

## Request/Response Handling

### Request Flow

```
MCP Client (e.g., Claude)
        │
        ├─→ JSON-RPC Request (Tool Call)
        │   {
        │     "jsonrpc": "2.0",
        │     "id": 1,
        │     "method": "tools/call",
        │     "params": {
        │       "name": "search",
        │       "arguments": {
        │         "directory": "/path/to/code",
        │         "keyword": "error"
        │       }
        │     }
        │   }
        │
        ├─→ Stdio Transport
        │
        ├─→ RMCP Framework
        │   - Deserialize JSON-RPC
        │   - Route to correct tool handler
        │   - Extract Parameters<SearchParams>
        │
        ├─→ SearchTool.search()
        │   - Validate inputs
        │   - Initialize indexer
        │   - Run indexing
        │   - Execute hybrid search
        │   - Format results
        │
        ├─→ CallToolResult::success(vec![Content::text(...)])
        │
        └─→ JSON-RPC Response
            {
              "jsonrpc": "2.0",
              "id": 1,
              "result": {
                "content": [
                  {
                    "type": "text",
                    "text": "Found 5 results for 'error':..."
                  }
                ]
              }
            }
```

### Response Types

**Success Response**:
```rust
Ok(CallToolResult::success(vec![
    Content::text("Result text")
]))
```

**Error Response**:
```rust
Err(McpError::invalid_params(
    "Error description".to_string(),
    None
))
```

### Error Handling

All tools use `McpError` enum:
- `invalid_params`: Input validation failures
- `internal_error`: Server-side errors
- Includes error message + optional error data

### Response Content Types

- `Content::text(String)`: Plain text results (primary)
- Can include multiple Content items for rich responses

---

## Transport Layer (Stdio)

### Stdio Transport Implementation

**Framework**: RMCP provides `transport::stdio()`

**Connection Flow**:
```rust
SearchTool::with_sync_manager(sync_manager)
    .serve(stdio())      // Bind to stdin/stdout
    .await?
```

### Protocol Details

- **Format**: JSON-RPC 2.0 over stdio
- **Direction**: Bidirectional (request/response)
- **Buffering**: Standard I/O buffering
- **Encoding**: UTF-8 JSON

### Message Structure

**Frame Format**: Messages are newline-delimited JSON

```
Request:
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}\n

Response:
{"jsonrpc":"2.0","result":{"serverInfo":{...}},"id":1}\n
```

### Logging Considerations

- **Logging Output**: Goes to stderr
- **stdout Reserved**: Used exclusively for MCP protocol
- **Tracing Level**: DEBUG by default (configurable via `RUST_LOG`)

---

## Tool Structure & Implementation

### Common Pattern: File Operations

All file/code analysis tools follow this pattern:

```rust
async fn some_tool(
    &self,
    Parameters(params): Parameters<SomeParams>,
) -> Result<CallToolResult, McpError> {
    // 1. Input validation
    let path = Path::new(&params.path);
    if !path.exists() {
        return Err(McpError::invalid_params(
            format!("Path '{}' does not exist", params.path),
            None,
        ));
    }
    
    // 2. Processing
    let result = process_code(path)
        .map_err(|e| McpError::invalid_params(
            format!("Processing error: {}", e),
            None
        ))?;
    
    // 3. Response formatting
    let response = format!("Results:\n{}", result);
    
    // 4. Return
    Ok(CallToolResult::success(vec![
        Content::text(response)
    ]))
}
```

### Search Tool Implementation Details

**`search()` - Hybrid Search**:

```rust
async fn search(
    &self,
    Parameters(SearchParams { directory, keyword }): Parameters<SearchParams>,
) -> Result<CallToolResult, McpError> {
    // 1. Validate directory
    let dir_path = Path::new(&directory);
    
    // 2. Initialize unified indexer
    let qdrant_url = env::var("QDRANT_URL").unwrap_or_default();
    let collection_name = format!("code_chunks_{}", sanitized_name);
    let mut indexer = UnifiedIndexer::new(
        &cache_dir,
        &index_dir,
        &qdrant_url,
        &collection_name,
        384, // all-MiniLM-L6-v2 dimensions
    ).await?;
    
    // 3. Index directory (incremental - only changed files)
    let stats = indexer.index_directory(dir_path).await?;
    
    // 4. Track for background sync
    if let Some(ref sync_mgr) = self.sync_manager {
        sync_mgr.track_directory(dir_path.to_path_buf()).await;
    }
    
    // 5. Create hybrid search
    let hybrid_search = HybridSearch::with_defaults(
        indexer.embedding_generator_cloned(),
        indexer.vector_store_cloned(),
        Some(bm25_search),
    );
    
    // 6. Execute search
    let results = hybrid_search.search(&keyword, 10).await?;
    
    // 7. Format and return
    let response = format_results(&results);
    Ok(CallToolResult::success(vec![Content::text(response)]))
}
```

### Index Tool Implementation

**File**: `/home/molaco/Documents/rust-code-mcp/src/tools/index_tool.rs`

```rust
pub async fn index_codebase(
    params: IndexCodebaseParams,
    sync_manager: Option<&Arc<SyncManager>>,
) -> Result<CallToolResult, McpError> {
    let dir = PathBuf::from(&params.directory);
    let force = params.force_reindex.unwrap_or(false);
    
    // 1. Validate directory
    if !dir.is_dir() {
        return Err(McpError::invalid_params(...));
    }
    
    // 2. Create collection name from directory hash
    let dir_hash = hash_directory(&dir);
    let collection_name = format!("code_chunks_{}", &dir_hash[..8]);
    
    // 3. Create indexer
    let mut indexer = IncrementalIndexer::new(
        &cache_path,
        &tantivy_path,
        &qdrant_url,
        &collection_name,
        384,
        codebase_loc,
    ).await?;
    
    // 4. Handle force reindex
    if force {
        let snapshot_path = get_snapshot_path(&dir);
        std::fs::remove_file(&snapshot_path)?; // Clear Merkle snapshot
        indexer.clear_all_data().await?;       // Clear all indices
    }
    
    // 5. Run incremental indexing
    let stats = indexer.index_with_change_detection(&dir).await?;
    
    // 6. Track for background sync
    if let Some(sync_mgr) = sync_manager {
        sync_mgr.track_directory(dir).await;
    }
    
    // 7. Format response
    let response = format_index_results(&stats);
    Ok(CallToolResult::success(vec![Content::text(response)]))
}
```

### Health Check Tool

**File**: `/home/molaco/Documents/rust-code-mcp/src/tools/health_tool.rs`

```rust
pub async fn health_check(
    Parameters(HealthCheckParams { directory }): Parameters<HealthCheckParams>,
) -> Result<CallToolResult, McpError> {
    // 1. Determine paths
    let (bm25_path, merkle_path, collection_name) = if let Some(ref dir) = directory {
        let dir_hash = hash_directory(dir);
        (bm25_path, merkle_path, format!("code_chunks_{}", &dir_hash[..8]))
    } else {
        (default_bm25, default_merkle, "code_chunks_default".to_string())
    };
    
    // 2. Initialize components
    let bm25 = Bm25Search::new(&bm25_path).ok();
    let vector_store = VectorStore::new(config).await.ok();
    
    // 3. Create health monitor
    let monitor = HealthMonitor::new(bm25, vector_store, merkle_path);
    
    // 4. Run health check
    let health = monitor.check_health().await;
    
    // 5. Format response with status interpretation
    let mut response = format!("✓ System Status: {}\n\n", health.overall);
    response.push_str(&serde_json::to_string_pretty(&health)?);
    
    Ok(CallToolResult::success(vec![Content::text(response)]))
}
```

---

## Background Sync Integration

### SyncManager Architecture

**File**: `/home/molaco/Documents/rust-code-mcp/src/mcp/sync.rs`

```rust
pub struct SyncManager {
    tracked_dirs: Arc<RwLock<HashSet<PathBuf>>>,
    interval: Duration,                    // 5 minutes default
    qdrant_url: String,
    cache_base: PathBuf,
    tantivy_base: PathBuf,
}

impl SyncManager {
    pub async fn run(self: Arc<Self>) {
        // Initial sync after 5 seconds
        tokio::time::sleep(Duration::from_secs(5)).await;
        self.handle_sync_all().await;
        
        // Periodic sync
        let mut interval = tokio::time::interval(self.interval);
        loop {
            interval.tick().await;
            self.handle_sync_all().await;
        }
    }
    
    async fn handle_sync_all(&self) {
        let dirs = self.get_tracked_directories().await;
        for dir in dirs {
            if let Err(e) = self.sync_directory(&dir).await {
                tracing::error!("Failed to sync {}: {}", dir.display(), e);
            }
        }
    }
    
    async fn sync_directory(&self, dir: &PathBuf) -> Result<()> {
        // Create paths using directory hash
        let dir_hash = hash_directory(dir);
        
        // Create incremental indexer
        let mut indexer = IncrementalIndexer::new(
            &self.cache_base.join(&dir_hash),
            &self.tantivy_base.join(&dir_hash),
            &self.qdrant_url,
            &format!("code_chunks_{}", &dir_hash[..8]),
            384,
            None,
        ).await?;
        
        // Run incremental indexing (< 10ms if no changes)
        let stats = indexer.index_with_change_detection(dir).await?;
        
        if stats.indexed_files > 0 {
            tracing::info!("✓ Synced {}: {} files", dir.display(), stats.indexed_files);
        } else {
            tracing::debug!("No changes in {}", dir.display());
        }
        
        Ok(())
    }
}
```

### Lifecycle

1. **Main starts SyncManager**: `Arc::new(SyncManager::with_defaults(300))`
2. **Background task spawned**: `tokio::spawn(async move { sync_manager.run() })`
3. **Tool tracks directory**: When search/index succeeds, `sync_manager.track_directory(dir)`
4. **Periodic sync**: Every 5 minutes, checks all tracked directories for changes
5. **Incremental detection**: Uses Merkle tree for < 10ms change detection

---

## Data Flow Diagram

### Complete Request/Response Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          MCP Client (Claude)                             │
│                                                                          │
│  User: "Search for error handling in /home/user/project"                │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
                    Generates Tool Call JSON-RPC
                               │
                      ┌────────▼─────────┐
                      │  JSON-RPC 2.0    │
                      │ {"method": ...}  │
                      └────────┬─────────┘
                               │
┌──────────────────────────────┴──────────────────────────────────────────┐
│                        Stdio Transport                                   │
│             (newline-delimited JSON over stdin/stdout)                   │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
                    ┌──────────▼────────────┐
                    │  RMCP Framework       │
                    │ - Deserialize JSON    │
                    │ - Route to handler    │
                    │ - Type-check params   │
                    └──────────┬────────────┘
                               │
                    ┌──────────▼────────────┐
                    │  Tool Router          │
                    │  (macro-generated)    │
                    └──────────┬────────────┘
                               │
┌──────────────────────────────┴──────────────────────────────────────────┐
│                     SearchTool::search()                                │
│                                                                          │
│  Input: directory="/home/user/project", keyword="error"                 │
│                                                                          │
│  1. Validation                                                          │
│     ├─ Check directory exists                                          │
│     └─ Check keyword not empty                                         │
│                                                                          │
│  2. Initialize Indexing Components                                      │
│     ├─ UnifiedIndexer                                                  │
│     ├─ EmbeddingGenerator (fastembed)                                  │
│     └─ VectorStore (Qdrant client)                                     │
│                                                                          │
│  3. Incremental Indexing                                                │
│     ├─ Check Merkle snapshot for changes                               │
│     ├─ Index only changed files (< 10ms if no changes)                │
│     ├─ Generate embeddings for new chunks                              │
│     ├─ Push to Qdrant                                                  │
│     └─ Update Merkle tree                                              │
│                                                                          │
│  4. Track Directory for Background Sync                                  │
│     └─ SyncManager::track_directory(dir)                               │
│        (will be auto-synced every 5 minutes)                           │
│                                                                          │
│  5. Hybrid Search Execution                                              │
│     ├─ Generate embedding for "error"                                  │
│     ├─ BM25 search on Tantivy                                          │
│     ├─ Vector search on Qdrant                                         │
│     └─ Reciprocal Rank Fusion (RRF) to combine results                 │
│                                                                          │
│  6. Format Results                                                       │
│     ├─ For each result:                                                │
│     │  ├─ File path                                                    │
│     │  ├─ Symbol name and kind                                         │
│     │  ├─ Line numbers                                                 │
│     │  ├─ Score (0-1)                                                  │
│     │  └─ Code preview (3 lines)                                       │
│     └─ Indexing statistics                                             │
│                                                                          │
│  Output: CallToolResult::success(vec![Content::text(result_string)])   │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
                    ┌──────────▼────────────┐
                    │  RMCP Framework       │
                    │ - Wrap in JSON-RPC    │
                    │ - Serialize           │
                    └──────────┬────────────┘
                               │
                    ┌──────────▼────────────┐
                    │  Stdio Transport      │
                    │ - Write to stdout     │
                    └──────────┬────────────┘
                               │
┌──────────────────────────────┴──────────────────────────────────────────┐
│                           MCP Client                                    │
│                                                                          │
│  Response: "Found 5 results for 'error':                               │
│            1. Score: 0.95 | File: src/parser/mod.rs | Symbol: parse   │
│               Lines: 42-68                                              │
│               Doc: Parses Rust source code                             │
│               Code preview:                                             │
│                 pub fn parse_file(...) -> Result<Vec<Symbol>> {         │
│                   // error handling...                                  │
│            ..."                                                         │
│                                                                          │
│  User can now read files, find definitions, etc.                       │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Key Design Patterns

### 1. **Macro-Based Tool Registration**

```rust
#[tool_router]          // Generates routing logic
impl SearchTool {
    #[tool(description = "...")]  // Registers tool with description
    async fn tool_name(...) { }
}

#[tool_handler]         // Implements ServerHandler trait
impl ServerHandler for SearchTool { }
```

**Benefits**:
- Automatic JSON schema generation
- Type-safe parameter extraction
- Declarative tool registration
- Reduces boilerplate

### 2. **Arc<SyncManager> Dependency Injection**

```rust
pub struct SearchTool {
    tool_router: ToolRouter<Self>,
    sync_manager: Option<Arc<SyncManager>>,  // Optional dependency
}

impl SearchTool {
    pub fn with_sync_manager(sync_manager: Arc<SyncManager>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            sync_manager: Some(sync_manager),
        }
    }
}
```

**Benefits**:
- Decouples tool execution from background sync
- Optional feature (tools work without it)
- Shared mutable state via Arc + RwLock
- Safe concurrent access

### 3. **Directory Hash-Based Collection Naming**

```rust
let dir_hash = {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(dir.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
};

let collection_name = format!("code_chunks_{}", &dir_hash[..8]);
```

**Benefits**:
- Consistent collection naming across tools
- Supports multiple project indexing
- No conflicts between projects
- Deterministic (same directory = same collection)

### 4. **Incremental Indexing with Merkle Trees**

```rust
// First run: full index
let stats = indexer.index_with_change_detection(&dir).await?;

// Subsequent runs: change detection (< 10ms if no changes)
// - Compares file hashes against Merkle snapshot
// - Only reindexes modified files
// - Reuses existing embeddings for unchanged chunks
```

**Benefits**:
- Fast change detection (< 10ms)
- Reduced redundant computation
- Efficient for large codebases
- Background sync feasible

### 5. **Unified vs Incremental Indexer**

**UnifiedIndexer** (used in `search()` tool):
- Creates indices on demand
- Full indexing workflow
- Creates Tantivy + Qdrant indices simultaneously

**IncrementalIndexer** (used in `index_codebase()` and background sync):
- Reuses existing indices
- Detects changes via Merkle tree
- Incremental updates only
- Used by background sync for efficiency

### 6. **Error Handling Pattern**

```rust
// Validation errors
Err(McpError::invalid_params(
    format!("Path '{}' does not exist", path),
    None,
))

// Processing errors (from internal crates)
.map_err(|e| McpError::invalid_params(
    format!("Processing error: {}", e),
    None
))?

// Always return McpError for consistency
```

### 7. **Result Formatting**

All tools use consistent response format:
```rust
let response = format!(
    "Tool Results:\n\
    - Field1: {}\n\
    - Field2: {}\n\
    {additional_details}",
    value1, value2
);

Ok(CallToolResult::success(vec![
    Content::text(response)
]))
```

### 8. **Configuration via Environment Variables**

```rust
// Qdrant URL
let qdrant_url = std::env::var("QDRANT_URL")
    .unwrap_or_else(|_| "http://localhost:6334".to_string());

// Logging level
RUST_LOG=debug cargo run
```

### 9. **XDG-Compliant Data Directories**

```rust
fn data_dir() -> PathBuf {
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
}
// ~/.local/share/rust-code-mcp/search on Linux
// ~/Library/Application Support/rust-code-mcp/search on macOS
```

---

## Component Interaction Summary

| Component | Role | Integration |
|-----------|------|-------------|
| **SearchTool** | Main handler | Implements ServerHandler, routes tools |
| **SyncManager** | Background service | Injected into SearchTool, runs separately |
| **UnifiedIndexer** | Full indexing | Used by search/get_similar_code tools |
| **IncrementalIndexer** | Incremental indexing | Used by index_codebase and sync manager |
| **RustParser** | Code analysis | Provides AST for find_definition, find_references, etc. |
| **EmbeddingGenerator** | Semantic representation | Generates 384-dim vectors for code chunks |
| **VectorStore** | Vector database | Qdrant client for semantic search |
| **Bm25Search** | Full-text search | Tantivy-based keyword search |
| **HybridSearch** | Combined search | Combines BM25 + vector results via RRF |
| **HealthMonitor** | System status | Checks health of BM25, vector store, Merkle tree |
| **MetadataCache** | File metadata | sled-based cache for file hashing |

---

## Configuration Summary

### Environment Variables

- `QDRANT_URL`: Qdrant server URL (default: `http://localhost:6334`)
- `RUST_LOG`: Logging level (default: `debug`)

### Paths (XDG-Compliant)

- **Data dir**: `~/.local/share/rust-code-mcp/search` (Linux)
- **Index dir**: `{data_dir}/index/{project_hash}/`
- **Cache dir**: `{data_dir}/cache/{project_hash}/`
- **Merkle snapshots**: `{data_dir}/snapshots/{project_hash}/`

### Service Parameters

- **Sync interval**: 300 seconds (5 minutes)
- **Vector size**: 384 (all-MiniLM-L6-v2)
- **Search limit**: 10 results per search
- **Change detection**: < 10ms (with Merkle tree)

---

## Library Dependencies

```toml
[dependencies]
# MCP Framework
rmcp = { git = "https://...", branch = "main", features = ["server", "transport-io"] }

# Async Runtime
tokio = { version = "1", features = ["macros", "rt", "rt-multi-thread", "io-std", "signal"] }

# Search & Indexing
tantivy = "0.22.0"           # Full-text search
qdrant-client = "1"           # Vector database
fastembed = "4"              # Local embeddings
text-splitter = "0.13"       # Semantic chunking
tree-sitter = "0.20"         # AST parsing
tree-sitter-rust = "0.20"    # Rust grammar

# Storage
sled = "0.34"                # Embedded KV store (metadata cache)
rs_merkle = "1.4"            # Merkle trees (change detection)

# Utilities
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sha2 = "0.10"                # File hashing
directories = "5"            # XDG paths
tracing = "0.1"              # Structured logging
```

---

## Summary

This MCP server implements a **sophisticated code search platform** with:

1. **10 specialized tools** for code analysis and search
2. **Hybrid search** combining full-text (BM25) and semantic (vector) search
3. **Incremental indexing** with Merkle tree change detection (< 10ms)
4. **Background sync** that automatically reindexes tracked directories
5. **Multiple project support** via directory hash-based collection naming
6. **Production-ready health monitoring** of all components
7. **Macro-based tool registration** for clean, maintainable code
8. **Standard MCP protocol** via RMCP framework (Rust SDK)
9. **Structured logging** with configurable levels
10. **XDG-compliant** data storage

The architecture enables rapid code exploration at scale while maintaining consistency across multiple indexed codebases.
