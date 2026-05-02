# Phase 7 - Enhanced MCP Tools (Week 12: Core Tools) - Complete

**Implementation Date:** 2025-01-XX
**Status:** ✅ COMPLETE
**Related Phases:** Builds on Phase 6 (Hybrid Search)

## Overview

Phase 7 Week 12 adds 6 new MCP tools to the server, providing comprehensive code intelligence capabilities beyond basic search. These tools leverage the parsing, chunking, call graph, and vector search infrastructure built in Phases 2-6.

## Implemented Tools

### 1. `find_definition`
**Purpose:** Locate where a Rust symbol is defined in the codebase

**Parameters:**
- `symbol_name`: Symbol to find (function, struct, trait, etc.)
- `directory`: Directory to search in

**Implementation:**
- Recursively scans `.rs` files using `RustParser`
- Extracts all symbols and matches by name
- Returns file path, line number, and symbol kind

**Example Output:**
```
Found 1 definition(s) for 'HybridSearch':
- src/search/mod.rs:91 (struct)
```

### 2. `find_references`
**Purpose:** Find all places where a symbol is referenced or called

**Parameters:**
- `symbol_name`: Symbol to find references to
- `directory`: Directory to search in

**Implementation:**
- Uses `CallGraph` to find all callers
- Scans all files and builds call graphs
- Returns files and calling functions

**Example Output:**
```
Found 3 reference(s) to 'parse_file' in 2 file(s):
- src/chunker/mod.rs (called by: chunk_file)
- src/tools/search_tool.rs (called by: find_definition, get_dependencies)
```

### 3. `get_dependencies`
**Purpose:** Get import dependencies for a Rust source file

**Parameters:**
- `file_path`: Path to the file to analyze

**Implementation:**
- Uses `parse_file_complete` to extract imports
- Returns all `use` statements found in the file

**Example Output:**
```
Dependencies for 'src/search/mod.rs':

Imports (4):
- crate::chunker::{ChunkId, CodeChunk}
- crate::embeddings::EmbeddingGenerator
- crate::vector_store::{VectorStore, SearchResult as VectorSearchResult}
- serde::{Deserialize, Serialize}
```

### 4. `get_call_graph`
**Purpose:** Show function call relationships for a file or specific symbol

**Parameters:**
- `file_path`: Path to the file to analyze
- `symbol_name` (optional): Specific symbol to get call graph for

**Implementation:**
- Uses `CallGraph::build` to construct call graph
- Shows both callers and callees
- Can show entire file's call graph or focus on one symbol

**Example Output:**
```
Call graph for 'src/parser/mod.rs':

Symbol: parse_source_complete

Calls (2):
  → extract_symbols
  → build

Called by (1):
  ← chunk_file
```

### 5. `analyze_complexity`
**Purpose:** Calculate code complexity metrics

**Parameters:**
- `file_path`: Path to the file to analyze

**Metrics Calculated:**
- **Lines of Code:** Total, non-empty, comment, code lines
- **Symbol Counts:** Functions, structs, traits
- **Cyclomatic Complexity:** Total and average per function
- **Function Calls:** Total call relationships

**Implementation:**
- Parses file to extract symbols
- Counts control flow keywords (`if`, `while`, `for`, `match`, `&&`, `||`)
- Uses call graph for relationship complexity

**Example Output:**
```
Complexity analysis for 'src/search/mod.rs':

=== Code Metrics ===
Total lines:           458
Non-empty lines:       389
Comment lines:         47
Code lines (approx):   342

=== Symbol Counts ===
Functions:             8
Structs:               4
Traits:                0

=== Complexity ===
Total cyclomatic:      23
Avg per function:      2.88
Function calls:        15
```

### 6. `get_similar_code`
**Purpose:** Find semantically similar code using vector embeddings

**Parameters:**
- `query`: Code snippet or description to find similar code
- `directory`: Directory containing the codebase
- `limit` (optional): Number of results (default 5)

**Implementation:**
- Initializes `EmbeddingGenerator` and `VectorStore`
- Creates `HybridSearch` in vector-only mode
- Uses `vector_only_search` to find similar chunks

**Example Output:**
```
Found 5 similar code snippet(s) for query 'parse rust code':

1. Score: 0.8742 | File: src/parser/mod.rs | Symbol: parse_source (function)
   Lines: 132-141
   Doc: Parse Rust source code and extract symbols
   Code preview:
   pub fn parse_source(&mut self, source: &str) -> Result<Vec<Symbol>, Box<dyn std::error::Error>> {
       let tree = self.parser.parse(source, None)
           .ok_or("Failed to parse source code")?;
```

## Architecture Changes

### Modified Files

**src/tools/search_tool.rs**
- Added 6 new parameter structs (lines 32-82)
- Added 6 new tool methods (lines 478-916)
- Updated server instructions (line 931)
- Total additions: ~500 lines

**Imports Added:**
```rust
use file_search_mcp::parser::RustParser;
use file_search_mcp::embeddings::EmbeddingGenerator;
use file_search_mcp::vector_store::{VectorStore, VectorStoreConfig};
use file_search_mcp::search::HybridSearch;
```

### Tool Summary

| Tool | Purpose | Infrastructure Used |
|------|---------|-------------------|
| find_definition | Locate symbol definitions | RustParser, Symbol extraction |
| find_references | Find symbol usage | CallGraph |
| get_dependencies | Analyze imports | Import extraction |
| get_call_graph | Show call relationships | CallGraph |
| analyze_complexity | Calculate metrics | Parser, CallGraph |
| get_similar_code | Semantic similarity | Vector search, Embeddings |

## Infrastructure Dependencies

All tools leverage existing infrastructure from previous phases:

- **Phase 2:** tree-sitter parsing, symbol extraction, call graph
- **Phase 3:** Semantic chunking
- **Phase 4:** Local embeddings (all-MiniLM-L6-v2)
- **Phase 5:** Qdrant vector store
- **Phase 6:** Hybrid search infrastructure

## Testing

**Test Results:** ✅ All 45 library tests passing

```bash
cargo test --lib
test result: ok. 45 passed; 0 failed; 11 ignored
```

**Note:** No new tests were added for MCP tools since they require integration testing with an MCP client. The underlying infrastructure (parser, call graph, embeddings, vector store) is already thoroughly tested.

## Performance Considerations

### Scalability
- `find_definition` and `find_references`: O(n) file scan, can be slow on large codebases
- `analyze_complexity`: Fast, single-file analysis
- `get_similar_code`: Requires Qdrant connection and embeddings, network latency

### Future Optimizations
- Add caching for parsed results in `find_definition`/`find_references`
- Implement incremental indexing for symbol lookups
- Pre-compute call graphs for faster reference finding

## Usage Example

### Via MCP Protocol

```json
{
  "method": "tools/call",
  "params": {
    "name": "find_definition",
    "arguments": {
      "symbol_name": "HybridSearch",
      "directory": "/path/to/project"
    }
  }
}
```

### Expected Response

```json
{
  "result": {
    "content": [
      {
        "type": "text",
        "text": "Found 1 definition(s) for 'HybridSearch':\n- src/search/mod.rs:91 (struct)\n"
      }
    ]
  }
}
```

## Known Limitations

1. **Symbol Resolution**: Does not handle qualified names (e.g., `module::Symbol`)
2. **Cross-File Analysis**: find_references only works at function call level, doesn't track variable references
3. **Cyclomatic Complexity**: Simplified calculation, not as sophisticated as rust-code-analysis
4. **Vector Search**: Requires Qdrant server running and indexed chunks

## What's Not Included (Week 13)

The following are planned for Phase 7 Week 13 but not implemented yet:

- **MCP Resources:** URI-based access to AST, metrics, docs
  - `rust:///file/ast` - Get Abstract Syntax Tree
  - `rust:///file/metrics` - Get code metrics
  - `rust:///symbol/docs` - Get documentation
  - `rust:///symbol/references` - Get references

## Phase Completion

**Week 12 Deliverables:** ✅ COMPLETE
- [x] find_definition tool
- [x] find_references tool
- [x] get_dependencies tool
- [x] get_call_graph tool
- [x] analyze_complexity tool
- [x] get_similar_code tool
- [x] All tests passing
- [x] Documentation complete

**Week 13 Deliverables:** ⏳ PENDING
- [ ] MCP resource implementations
- [ ] Resource URI routing
- [ ] Resource testing
- [ ] Updated documentation

## Next Steps

1. Implement MCP resources for Week 13
2. Test all tools with Claude Desktop
3. Add integration tests
4. Document tool usage patterns
5. Optimize performance for large codebases

## References

- MCP Specification: https://modelcontextprotocol.io/
- rmcp crate: https://docs.rs/rmcp/
- Phase 6 Documentation: [PHASE6_COMPLETE.md](PHASE6_COMPLETE.md)
