# MCP Server Implementation - Quick Reference Guide

## Project Overview
- **Name**: rust-code-mcp (file-search-mcp)
- **Framework**: RMCP (Rust SDK for Model Context Protocol)
- **Language**: Rust 2024 Edition
- **Runtime**: Tokio (async/multi-threaded)
- **Transport**: Stdio (JSON-RPC 2.0 over stdin/stdout)

---

## 1. Quick Start

### Entry Point: `src/main.rs`
```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup logging
    tracing_subscriber::fmt().init();
    
    // 2. Create & spawn background sync manager
    let sync_manager = Arc::new(SyncManager::with_defaults(300));
    tokio::spawn(sync_manager.clone().run());
    
    // 3. Create SearchTool with sync manager
    SearchTool::with_sync_manager(sync_manager)
        .serve(stdio())      // Bind to stdio transport
        .await?
        .waiting()           // Wait for requests
        .await?;
    
    Ok(())
}
```

---

## 2. Tool Registration System

### Macro-Based Tool Definition
```rust
#[tool_router]
impl SearchTool {
    #[tool(description = "Read file contents")]
    async fn read_file_content(
        &self,
        Parameters(params): Parameters<FileContentParams>,
    ) -> Result<CallToolResult, McpError> {
        // Implementation
    }
}

#[tool_handler]
impl ServerHandler for SearchTool {
    fn get_info(&self) -> ServerInfo { /* ... */ }
}
```

### Key Macros
- `#[tool_router]`: Generates routing logic for tools
- `#[tool(description)]`: Registers tool with JSON schema
- `#[tool_handler]`: Implements MCP ServerHandler trait
- `Parameters<T>`: Extracts deserialized parameters

---

## 3. Available Tools (10 Total)

| Tool | Parameters | Purpose |
|------|-----------|---------|
| `read_file_content` | `file_path: String` | Read file text |
| `search` | `directory: String, keyword: String` | Hybrid BM25+vector search |
| `find_definition` | `symbol_name: String, directory: String` | Locate symbol definition |
| `find_references` | `symbol_name: String, directory: String` | Find all references |
| `get_dependencies` | `file_path: String` | List imports |
| `get_call_graph` | `file_path: String, symbol_name: Option<String>` | Show function calls |
| `analyze_complexity` | `file_path: String` | Code metrics (LOC, CC) |
| `get_similar_code` | `query: String, directory: String, limit: Option<usize>` | Semantic search |
| `index_codebase` | `directory: String, force_reindex: Option<bool>` | Manual indexing |
| `health_check` | `directory: Option<String>` | System health status |

---

## 4. Request/Response Flow

### JSON-RPC Message Format
```json
Request:
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "search",
    "arguments": {
      "directory": "/path/to/code",
      "keyword": "error"
    }
  }
}

Response:
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "Found 5 results..."
      }
    ]
  }
}
```

### Tool Implementation Pattern
```rust
async fn tool_name(
    &self,
    Parameters(params): Parameters<ParamsStruct>,
) -> Result<CallToolResult, McpError> {
    // 1. Validate inputs
    validate_params(&params)?;
    
    // 2. Process
    let result = do_work(&params)
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
    
    // 3. Format response
    let response = format!("Results:\n{}", result);
    
    // 4. Return
    Ok(CallToolResult::success(vec![
        Content::text(response)
    ]))
}
```

---

## 5. Transport Layer

### Stdio Implementation
- **Protocol**: JSON-RPC 2.0
- **Encoding**: UTF-8 JSON
- **Format**: Newline-delimited messages
- **Direction**: Bidirectional
- **Usage**: `SearchTool.serve(stdio()).await?`

### I/O Handling
- **Input**: stdin (MCP messages)
- **Output**: stdout (protocol messages)
- **Logging**: stderr (via tracing)

---

## 6. Tool Files Location

```
src/tools/
├── mod.rs              # Module exports
├── search_tool.rs      # Main SearchTool (10 tools + ServerHandler)
├── index_tool.rs       # index_codebase implementation
└── health_tool.rs      # health_check implementation
```

### SearchTool Structure
```rust
pub struct SearchTool {
    tool_router: ToolRouter<Self>,
    sync_manager: Option<Arc<SyncManager>>,  // Optional background sync
}
```

---

## 7. Background Sync Integration

### SyncManager (`src/mcp/sync.rs`)
- **Purpose**: Automatic periodic reindexing of tracked directories
- **Interval**: 5 minutes (configurable)
- **Change Detection**: Merkle tree (< 10ms)
- **Lifecycle**: 
  1. Created with `Arc::new()` in main
  2. Spawned as background task
  3. Injected into SearchTool
  4. Directories tracked when tools succeed

### Usage in Tools
```rust
// When indexing succeeds, track directory for auto-sync
if let Some(ref sync_mgr) = self.sync_manager {
    sync_mgr.track_directory(dir_path.to_path_buf()).await;
}
```

---

## 8. Key Components

### Indexing
- **UnifiedIndexer**: Full indexing (used by search tool)
- **IncrementalIndexer**: Change detection (used by background sync)
- **Change Detection**: Merkle tree snapshots

### Search
- **BM25Search**: Full-text search (Tantivy)
- **VectorStore**: Semantic search (Qdrant)
- **HybridSearch**: Combined results (RRF)

### Code Analysis
- **RustParser**: AST parsing (tree-sitter)
- **ChunkSchema**: Code chunk indexing
- **EmbeddingGenerator**: Vector generation (fastembed)

### Monitoring
- **HealthMonitor**: System health checks
- **MetadataCache**: File metadata storage (sled)

---

## 9. Configuration

### Environment Variables
```bash
# Qdrant server (default: http://localhost:6334)
export QDRANT_URL=http://localhost:6333

# Logging level (default: debug)
export RUST_LOG=debug
```

### Data Directories (XDG-Compliant)
- **Linux**: `~/.local/share/rust-code-mcp/search/`
- **macOS**: `~/Library/Application Support/rust-code-mcp/search/`
- **Structure**:
  ```
  cache/{project_hash}/          # Metadata cache
  index/{project_hash}/          # Tantivy indices
  snapshots/{project_hash}/      # Merkle tree snapshots
  ```

---

## 10. Directory Hash Strategy

All tools use consistent collection naming:
```rust
let dir_hash = hash_directory(&dir);  // SHA-256 of directory path
let collection_name = format!("code_chunks_{}", &dir_hash[..8]);
```

**Benefits**:
- Supports multiple projects
- No name conflicts
- Deterministic naming
- Same dir = same collection across tools

---

## 11. Error Handling

### McpError Variants
```rust
// Invalid input parameters
Err(McpError::invalid_params("message".to_string(), None))

// Server-side errors
Err(McpError::internal_error("message".to_string(), None))
```

### Pattern
```rust
result
    .map_err(|e| McpError::invalid_params(
        format!("Operation failed: {}", e),
        None
    ))?
```

---

## 12. Module Structure

```
src/
├── main.rs                    # Entry point
├── lib.rs                     # Library exports
├── mcp/
│   ├── mod.rs                # MCP integration
│   └── sync.rs               # Background sync manager
├── tools/
│   ├── mod.rs
│   ├── search_tool.rs        # Main SearchTool + 10 tools
│   ├── index_tool.rs         # index_codebase
│   └── health_tool.rs        # health_check
├── indexing/
│   ├── mod.rs
│   ├── unified.rs            # Full indexing
│   ├── incremental.rs        # Incremental with Merkle tree
│   └── merkle.rs             # Merkle tree implementation
├── search/
│   ├── mod.rs
│   ├── bm25.rs              # Full-text search
│   └── hybrid.rs            # Combined search
├── vector_store/
│   ├── mod.rs               # Qdrant integration
│   └── config.rs            # HNSW optimization
├── embeddings/mod.rs        # fastembed wrapper
├── parser/mod.rs            # tree-sitter wrapper
├── chunker/mod.rs           # Code chunking
├── schema.rs                # Tantivy schemas
├── metadata_cache.rs        # sled-based cache
├── monitoring/              # Health checks
└── security/                # Secret scanning
```

---

## 13. Parameter Structures

All use `serde` + `schemars`:
```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "...")]
    pub directory: String,
    #[schemars(description = "...")]
    pub keyword: String,
}
```

**Benefits**:
- Automatic JSON deserialization
- Type-safe parameter extraction
- JSON schema generation for clients

---

## 14. Important Dependencies

```toml
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", features = ["server", "transport-io"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "io-std"] }
tantivy = "0.22.0"
qdrant-client = "1"
fastembed = "4"
tree-sitter = "0.20"
tree-sitter-rust = "0.20"
sled = "0.34"
rs_merkle = "1.4"
```

---

## 15. Debugging Tips

### Check Logs
```bash
# Debug level logging
RUST_LOG=debug cargo run

# Specific module
RUST_LOG=file_search_mcp::tools=debug cargo run
```

### Verify Configuration
```bash
echo $QDRANT_URL
echo $RUST_LOG
```

### Check Data Directories
```bash
ls ~/.local/share/rust-code-mcp/search/
```

### Monitor Background Sync
Watch logs for "Starting background sync" and "Syncing X tracked directories"

---

## 16. Performance Characteristics

| Operation | Time | Notes |
|-----------|------|-------|
| Change detection | < 10ms | With Merkle tree, no changes |
| Small codebase indexing | 1-5s | < 10K LOC |
| Large codebase indexing | 30-60s | 100K+ LOC |
| Hybrid search | 100-500ms | BM25 + vector + RRF |
| Background sync | 1-5s | Depends on changes |

---

## 17. Testing

### Run Tests
```bash
cargo test
```

### Ignore Tests Requiring Qdrant
```bash
cargo test -- --skip "requires_qdrant"
```

### Integration Tests
Located in `tests/` directory

---

## 18. Building & Deployment

### Debug Build
```bash
cargo build
./target/debug/file-search-mcp
```

### Release Build
```bash
cargo build --release
./target/release/file-search-mcp
```

### Environment Setup
1. Ensure Qdrant is running: `http://localhost:6333`
2. Set `QDRANT_URL` if different
3. Run MCP server

---

## Summary

This MCP server provides:
- **10 specialized tools** for code analysis
- **Hybrid search** (BM25 + semantic)
- **Incremental indexing** with Merkle trees
- **Background sync** for auto-updates
- **Multi-project support** via hash-based collections
- **RMCP framework** for MCP protocol compliance
- **Stdio transport** for CLI integration
- **Structured logging** for debugging

See `MCP_IMPLEMENTATION_ANALYSIS.md` for comprehensive architecture details.
