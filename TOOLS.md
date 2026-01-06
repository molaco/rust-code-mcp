# rust-code-mcp Tools Reference

Complete reference for all MCP tools provided by rust-code-mcp.

## Overview

| Tool | Category | Description |
|------|----------|-------------|
| [`search`](#search) | Query | Hybrid keyword + semantic search |
| [`get_similar_code`](#get_similar_code) | Query | Find semantically similar code |
| [`find_definition`](#find_definition) | Analysis | Locate symbol definitions by name |
| [`find_references`](#find_references) | Analysis | Find all usages of a symbol by name |
| [`get_dependencies`](#get_dependencies) | Analysis | List imports for a file |
| [`get_call_graph`](#get_call_graph) | Analysis | Show function call relationships |
| [`analyze_complexity`](#analyze_complexity) | Analysis | Calculate code complexity metrics |
| [`read_file_content`](#read_file_content) | Query | Read file contents |
| [`index_codebase`](#index_codebase) | Index | Manually trigger indexing |
| [`health_check`](#health_check) | Index | Check system status |

---

## Query Tools

### search

Hybrid search combining BM25 keyword matching with semantic vector similarity (RRF fusion).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `keyword` | string | Yes | Search query |
| `directory` | string | Yes | Project root directory |

**Example:**
```json
{
  "keyword": "parse rust source",
  "directory": "/path/to/project"
}
```

**Returns:** Ranked list of matching code chunks with scores, file paths, symbol names, line numbers, and preview.

---

### get_similar_code

Find code snippets semantically similar to a query using vector embeddings.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | string | Yes | Code snippet or natural language query |
| `directory` | string | Yes | Directory containing the codebase |
| `limit` | integer | No | Number of results (default: 5) |

**Example:**
```json
{
  "query": "function that reads configuration from file",
  "directory": "/path/to/project",
  "limit": 3
}
```

**Returns:** Similar code snippets ranked by semantic similarity score (0-1).

---

### read_file_content

Read the content of a file.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to read |

**Example:**
```json
{
  "file_path": "/path/to/project/src/main.rs"
}
```

**Returns:** Full file contents as text.

---

## Analysis Tools

### find_definition

Find where a Rust symbol (function, struct, trait, const, etc.) is defined. Uses rust-analyzer's semantic analysis for accurate results.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `symbol_name` | string | Yes | Name of the symbol to find |
| `directory` | string | Yes | Project root directory containing Cargo.toml |

**Example:**
```json
{
  "symbol_name": "RustParser",
  "directory": "/path/to/project"
}
```

**Returns:** Definition location(s) with file path, line, column, and symbol name.

**Example output:**
```
Found 1 definition(s) for 'RustParser':
/path/to/project/src/parser/mod.rs:121:12 (RustParser)
```

**Notes:**
- First query triggers lazy loading of rust-analyzer (~120ms)
- Subsequent queries are instant (<10ms)
- Only searches local project code (not dependencies)

---

### find_references

Find all places where a symbol is used (calls, type references, etc.). Uses rust-analyzer's semantic analysis.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `symbol_name` | string | Yes | Name of the symbol to find references for |
| `directory` | string | Yes | Project root directory containing Cargo.toml |

**Example:**
```json
{
  "symbol_name": "parse_file",
  "directory": "/path/to/project"
}
```

**Returns:** All reference locations with file path, line, and column.

**Example output:**
```
Found 21 reference(s) for 'RustParser':
/path/to/project/src/indexing/indexer_core.rs:64:20 (reference)
/path/to/project/src/indexing/indexer_core.rs:88:13 (reference)
/path/to/project/src/parser/mod.rs:121:12 (RustParser)
...
```

---

### get_dependencies

Get import dependencies for a Rust source file.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to analyze |

**Example:**
```json
{
  "file_path": "/path/to/project/src/parser/mod.rs"
}
```

**Returns:** List of all imports in the file.

**Example output:**
```
Dependencies for '/path/to/project/src/parser/mod.rs':

Imports (19):
- std::fs
- std::path::Path
- ra_ap_syntax::ast::self
- ra_ap_syntax::AstNode
...
```

---

### get_call_graph

Get the call graph showing function call relationships for a specific symbol.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to analyze |
| `symbol_name` | string | No | Specific symbol to get call graph for |

**Example:**
```json
{
  "file_path": "/path/to/project/src/parser/mod.rs",
  "symbol_name": "parse_file"
}
```

**Returns:** Functions called by the specified symbol.

**Example output:**
```
Call graph for '/path/to/project/src/parser/mod.rs':

Symbol: parse_file

Calls (2):
  -> read_to_string
  -> parse_source
```

---

### analyze_complexity

Analyze code complexity metrics including LOC, cyclomatic complexity, and function counts.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to analyze |

**Example:**
```json
{
  "file_path": "/path/to/project/src/parser/mod.rs"
}
```

**Returns:** Complexity metrics for the file.

**Example output:**
```
Complexity analysis for '/path/to/project/src/parser/mod.rs':

=== Code Metrics ===
Total lines:           619
Non-empty lines:       552
Comment lines:         73
Code lines (approx):   479

=== Symbol Counts ===
Functions:             21
Structs:               4
Traits:                0

=== Complexity ===
Total cyclomatic:      45
Avg per function:      2.14
Function calls:        69
```

---

## Index Tools

### index_codebase

Manually index a codebase directory. Uses incremental indexing with Merkle tree change detection.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Absolute path to codebase directory |
| `force_reindex` | boolean | No | Force full reindex even if already indexed (default: false) |

**Example:**
```json
{
  "directory": "/path/to/project",
  "force_reindex": false
}
```

**Returns:** Indexing statistics including files indexed, chunks created, and timing.

**Example output:**
```
Indexing stats:
- Indexed files: 0 (no changes)
- Total chunks: 0
- Unchanged files: 131
- Skipped files: 0
- Time: 78.100539ms (< 10ms change detection)

Background sync: enabled (5-minute interval)
```

**Notes:**
- First indexing takes longer (parsing + embedding generation)
- Subsequent runs use Merkle tree to detect changes (~10ms)
- Background sync automatically re-indexes every 5 minutes

---

### health_check

Check the health status of the code search system (BM25, Vector store, Merkle tree).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | No | Project directory to check (checks system-wide if not provided) |

**Example:**
```json
{
  "directory": "/path/to/project"
}
```

**Returns:** Health status of all system components.

**Example output:**
```
System Status: HEALTHY

{
  "overall": "healthy",
  "bm25": {
    "status": "healthy",
    "message": "BM25 search operational",
    "latency_ms": 0
  },
  "vector": {
    "status": "healthy",
    "message": "Vector store operational (921 vectors)",
    "latency_ms": 1
  },
  "merkle": {
    "status": "healthy",
    "message": "Merkle snapshot exists (19159 bytes)"
  }
}
```

**Status levels:**
- **Healthy**: All components operational
- **Degraded**: One search engine down OR Merkle snapshot missing
- **Unhealthy**: Both BM25 and Vector search are down

---

## Architecture

```
Query Tools                    Analysis Tools              Index Tools
     |                              |                           |
     v                              v                           v
+---------+                  +-------------+            +------------+
| search  |                  | find_def    |            | index_cb   |
| similar |                  | find_ref    |            | health     |
| read    |                  | deps/graph  |            +------------+
+---------+                  | complexity  |                  |
     |                       +-------------+                  v
     v                              |                  +-------------+
+------------+                      v                  | Incremental |
| Hybrid     |              +--------------+           | Indexer     |
| Search     |              | Semantic     |           +-------------+
| (BM25+Vec) |              | Service      |                  |
+------------+              | (ra_ap_ide)  |                  v
     |                      +--------------+           +-------------+
     v                              |                  | Merkle Tree |
+------------+                      |                  | (Changes)   |
| Tantivy    |              +-------+-------+          +-------------+
| LanceDB    |              |               |
+------------+              v               v
                     +--------+      +--------+
                     | Parser |      | Files  |
                     +--------+      +--------+
```

## Performance

| Operation | Typical Latency |
|-----------|-----------------|
| search | 10-50ms |
| get_similar_code | 20-100ms |
| find_definition (first) | ~120ms (loads IDE) |
| find_definition (cached) | <10ms |
| find_references | 10-200ms |
| index_codebase (no changes) | ~10ms |
| index_codebase (full) | 5-30s depending on size |
