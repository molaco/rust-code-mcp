# Rust Code MCP - Scalable Code Search for Large Rust Codebases

A Model Context Protocol (MCP) server for hybrid semantic + lexical code search, optimized for large Rust codebases (1M+ LOC).

**Status:** ✅ Phase 7 Complete - Enhanced MCP Tools (8 tools) | Integration Verified ✔

## 🎯 Project Goals

Build a scalable, in-process MCP server that provides:
- **Hybrid Search**: BM25 (lexical) + Vector embeddings (semantic)
- **Large-scale**: Handle 1M+ LOC codebases efficiently
- **Incremental**: Only reindex changed files
- **Local**: All processing local, no API calls
- **Fast**: Sub-200ms query latency

## 🏗️ Built On

This project is a fork of [file-search-mcp](https://github.com/Kurogoma4D/file-search-mcp) by Kurogoma4D, extended with:
- Semantic search via Qdrant vector database
- Tree-sitter code parsing
- Intelligent code chunking
- Incremental file watching
- Rust-specific optimizations

**Huge thanks to Kurogoma4D for the solid MCP + Tantivy foundation!**

## 📚 Documentation

### Planning & Architecture
- **[NEW_PLAN.md](docs/NEW_PLAN.md)** - 16-week implementation plan
- **[SEARCH.md](docs/SEARCH.md)** - Architecture overview
- **[EXTRACTION_PLAN.md](docs/EXTRACTION_PLAN.md)** - Bloop pattern extraction strategy
- **[NIX_INTEGRATION.md](docs/NIX_INTEGRATION.md)** - Nix integration guide

### Phase Completion Reports
- **[PHASE0_NOTES.md](docs/PHASE0_NOTES.md)** - Phase 0 progress & codebase analysis
- **[PHASE1_COMPLETE.md](docs/PHASE1_COMPLETE.md)** - Phase 1: Persistent index + incremental updates
- **[PHASE2_COMPLETE.md](docs/PHASE2_COMPLETE.md)** - Phase 2: Tree-sitter parsing + symbol extraction
- **[PHASE3_COMPLETE.md](docs/PHASE3_COMPLETE.md)** - Phase 3: Semantic code chunking
- **[PHASE4_COMPLETE.md](docs/PHASE4_COMPLETE.md)** - Phase 4: Local embedding generation
- **[PHASE5_COMPLETE.md](docs/PHASE5_COMPLETE.md)** - Phase 5: Qdrant vector search
- **[PHASE6_COMPLETE.md](docs/PHASE6_COMPLETE.md)** - Phase 6: Hybrid search with BM25 + Vector RRF
- **[PHASE7_WEEK12_COMPLETE.md](docs/PHASE7_WEEK12_COMPLETE.md)** - Phase 7 Week 12: Enhanced MCP Tools (6 new tools)
- **[PHASE7_COMPLETE.md](docs/PHASE7_COMPLETE.md)** - Phase 7 Complete: 8 Tools Total (Resources deferred)
- **[INTEGRATION_VERIFIED.md](docs/INTEGRATION_VERIFIED.md)** - Integration verified with Nix + Claude Code ⭐ NEW

### Testing & Usage
- **[TESTING.md](TESTING.md)** - Comprehensive testing guide for all 8 MCP tools

### Research & Analysis
- **[RUST_MCP_CODE_SEARCH_RESEARCH.md](docs/RUST_MCP_CODE_SEARCH_RESEARCH.md)** - Research on reusable components
- **[STATE_OF_THE_ART_CODEBASE_ANALYSIS.md](docs/STATE_OF_THE_ART_CODEBASE_ANALYSIS.md)** - Industry analysis

### Reference
- **[PLAN.md](docs/PLAN.md)** - Original from-scratch plan (for comparison)

## 🚀 Current Status

**Phase 0: Setup (Week 1)** ✅ **COMPLETE**
- [x] Fork file-search-mcp
- [x] Set up project structure
- [x] Copy planning documents
- [x] Study existing codebase → Documented in PHASE0_NOTES.md
- [x] Clone bloop for reference patterns → `/home/molaco/Documents/bloop`
- [x] ~~Set up Qdrant instance~~ → Deferred to Phase 5 (Docker not available)
- [x] Create extraction plan → EXTRACTION_PLAN.md created

**Phase 1: Persistent Index + Incremental Updates (Week 2)** ✅ **COMPLETE**
- [x] Add Phase 1 dependencies (sled, sha2, directories, bincode)
- [x] Create schema.rs with extended Tantivy schema (5 fields)
- [x] Make Tantivy index persistent (XDG-compliant storage)
- [x] Implement metadata cache for file tracking (SHA-256 + sled)
- [x] Add incremental indexing (only reindex changed files)
- [x] Test end-to-end with 3 scenarios
- [ ] File watching with notify crate → Deferred to Phase 1.5 (optional)

**Phase 2: Tree-sitter Integration (Week 5-6)** ✅ **COMPLETE**
- [x] Add tree-sitter and tree-sitter-rust dependencies
- [x] Create RustParser for AST parsing
- [x] Extract symbols (functions, structs, impls, traits)
- [x] Build call graph for function dependencies
- [x] Extract imports and docstrings
- [x] Test with real Rust files

**See [PHASE2_COMPLETE.md](docs/PHASE2_COMPLETE.md) for full details.**

**Phase 3: Semantic Code Chunking (Week 7)** ✅ **COMPLETE**
- [x] Implement symbol-based chunking
- [x] Add context enrichment (imports, calls, docstrings)
- [x] Implement overlap between chunks for continuity
- [x] Format chunks for embedding (contextual retrieval pattern)
- [x] Test chunking with parsed files

**See [PHASE3_COMPLETE.md](docs/PHASE3_COMPLETE.md) for full details.**

**Phase 4: Local Embedding Generation (Week 8)** ✅ **COMPLETE**
- [x] Integrate fastembed for local embeddings
- [x] Use all-MiniLM-L6-v2 model (384 dimensions)
- [x] Implement batch embedding generation
- [x] Add similarity computation
- [x] Test with code chunks

**See [PHASE4_COMPLETE.md](docs/PHASE4_COMPLETE.md) for full details.**

**Phase 5: Qdrant Vector Search (Week 9)** ✅ **COMPLETE**
- [x] Integrate Qdrant client
- [x] Create vector store with optimal configuration
- [x] Implement batch indexing
- [x] Add vector search with cosine similarity
- [x] Store full chunks as payloads
- [x] Test with embeddings

**See [PHASE5_COMPLETE.md](docs/PHASE5_COMPLETE.md) for full details.**

**Phase 6: Hybrid Search (Week 10-11)** ✅ **COMPLETE**
- [x] Create ChunkSchema for Tantivy
- [x] Implement Bm25Search module for chunk-level search
- [x] Implement Reciprocal Rank Fusion (RRF) algorithm
- [x] Create VectorSearch wrapper
- [x] Build HybridSearch infrastructure with BM25 + Vector
- [x] Add unified SearchResult type
- [x] Parallel execution (async vector + sync BM25)
- [x] Comprehensive testing (45 tests passing)

**See [PHASE6_COMPLETE.md](docs/PHASE6_COMPLETE.md) for full details.**

**Phase 7: Enhanced MCP Tools** ✅ **COMPLETE**

**Week 12: Core Tools** ✅
- [x] find_definition - Locate symbol definitions
- [x] find_references - Find all symbol references
- [x] get_dependencies - Analyze file imports
- [x] get_call_graph - Show function call relationships
- [x] analyze_complexity - Calculate code metrics
- [x] get_similar_code - Semantic similarity search

**Week 13: MCP Resources** ⏸️ **DEFERRED**
- [ ] rust:///file/ast - Get AST (deferred - rmcp SDK limitation)
- [ ] rust:///file/metrics - Get metrics (deferred)
- [ ] rust:///symbol/docs - Get docs (deferred)
- [ ] rust:///symbol/references - Get references (deferred)

**Note:** Resources deferred to future work. The rmcp Rust SDK doesn't currently document resource handler APIs. All planned functionality is available via tools.

**See [PHASE7_COMPLETE.md](docs/PHASE7_COMPLETE.md) for full details.**

## ✨ Planned Features

### Current Features
- ✅ Full-text search with Tantivy (BM25)
- ✅ MCP protocol implementation
- ✅ File content reader
- ✅ Smart file detection
- ✅ **Persistent index** - Survives restarts
- ✅ **Incremental indexing** - Only reindex changed files (10x+ faster)
- ✅ **SHA-256 change detection** - Efficient file tracking
- ✅ **XDG-compliant storage** - `~/.local/share/rust-code-mcp/`
- ✅ **Tree-sitter parsing** - Extract symbols from Rust code
- ✅ **Semantic chunking** - Symbol-based code chunks
- ✅ **Local embeddings** - all-MiniLM-L6-v2 (384 dims)
- ✅ **Vector search** - Qdrant with cosine similarity
- ✅ **Hybrid search** - BM25 + Vector with RRF fusion

### Enhancements (Phases 1-8)
- [x] **Phase 1 (Week 2)**: Persistent index + incremental updates ✅
- [x] **Phase 2 (Week 5-6)**: Tree-sitter parsing + symbol extraction ✅
- [x] **Phase 3 (Week 7)**: Semantic code chunking ✅
- [x] **Phase 4 (Week 8)**: Local embedding generation ✅
- [x] **Phase 5 (Week 9)**: Qdrant vector search ✅
- [x] **Phase 6 (Week 10-11)**: Hybrid search with RRF ✅
- [x] **Phase 7 (Week 12-13)**: Enhanced MCP tools ✅
  - Week 12: 6 new tools (find_definition, find_references, etc.) ✅
  - Week 13: MCP resources (deferred - rmcp SDK limitation) ⏸️
- [ ] **Phase 8 (Week 14-16)**: Optimization & release

## 🛠️ Technology Stack

### Current
- **Rust** - Performance and safety
- **Tantivy** - Full-text search (BM25) with persistent index
- **Sled** - Embedded KV store for metadata cache
- **SHA-256** - File change detection
- **RMCP** - Model Context Protocol
- **Tokio** - Async runtime
- **Tree-sitter** - AST parsing for Rust
- **Qdrant** - Vector database for semantic search
- **Fastembed** - Local embeddings (all-MiniLM-L6-v2)
- **UUID** - Unique chunk identification

### Planned Additions
- **notify** - File watching (optional, Phase 1.5)
- **rayon** - Parallel processing (optimization, Phase 8)
- Additional language grammars (multi-language support, Phase 7)

See [NEW_PLAN.md](docs/NEW_PLAN.md) for complete dependency list.

## 📋 Installation

### Current (file-search-mcp functionality)

```bash
cd /home/molaco/Documents/rust-code-mcp
cargo build --release
```

Add to your MCP settings (Cursor, Claude, etc.):
```json
{
  "mcpServers": {
    "rust-code": {
      "command": "/home/molaco/Documents/rust-code-mcp/target/release/file-search-mcp"
    }
  }
}
```

### Future (after Phase 2+)
```bash
cargo install rust-code-mcp
rust-code-mcp init --path /path/to/project
rust-code-mcp index
rust-code-mcp serve
```

## 🧪 Testing

### Unit Tests

Run all library tests:
```bash
cargo test --lib
```

**Expected output:** `test result: ok. 45 passed; 0 failed; 11 ignored`

### Integration Tests

#### Quick Manual Test

1. Build the project:
```bash
cargo build --release
```

2. Create test files:
```bash
mkdir -p /tmp/test-rust-search
echo "fn hello() { println!(\"test\"); }" > /tmp/test-rust-search/test.rs
echo "fn world() { println!(\"demo\"); }" > /tmp/test-rust-search/demo.rs
```

3. Clear any existing cache (for fresh test):
```bash
rm -rf ~/.local/share/rust-code-mcp/
```

4. Test via MCP Inspector:
```bash
npx @modelcontextprotocol/inspector ./target/release/file-search-mcp
```

5. In the MCP Inspector, call the `search` tool:
```json
{
  "directory": "/tmp/test-rust-search",
  "keyword": "test"
}
```

#### Verify Incremental Indexing

Test that the system only reindexes changed files:

**First search** (fresh index):
- Should index both files: `test.rs` and `demo.rs`
- Check logs: `New/Changed=2, Unchanged=0`

**Second search** (no changes):
- Should skip both files
- Check logs: `New/Changed=0, Unchanged=2`
- Should be **10x+ faster** (skips indexing)

**After modifying one file**:
```bash
echo "// Modified" >> /tmp/test-rust-search/test.rs
```
- Should reindex only `test.rs`
- Check logs: `New/Changed=1, Unchanged=1`

Enable debug logging to see detailed output:
```bash
RUST_LOG=debug ./target/release/file-search-mcp
```

#### Automated Test Script

Run the comprehensive test script:
```bash
./test-incremental.sh
```

This tests:
- Fresh indexing of 3 files
- Skipping unchanged files
- Selective reindexing of modified files
- Index persistence across restarts

### Testing with Claude Code

**Integration Verified ✔** - See [INTEGRATION_VERIFIED.md](docs/INTEGRATION_VERIFIED.md) for complete setup guide.

**Quick Start:**

1. Enter the Nix development shell:
```bash
cd /home/molaco/Documents/nix-code
nix develop
```

2. Open Claude Code:
```bash
cd /home/molaco/Documents/rust-code-mcp
claude-code .
```

3. Test the MCP tools with prompts like:
```
Read the file src/lib.rs using rust-code-mcp
Find where RustParser is defined in src/ using rust-code-mcp
Analyze the complexity of src/search/mod.rs using rust-code-mcp
```

See [INTEGRATION_VERIFIED.md](docs/INTEGRATION_VERIFIED.md) for all 8 tools and example prompts.

### Testing in Other MCP Clients

Add to your MCP client config (Claude Desktop, Cursor, etc.):
```json
{
  "mcpServers": {
    "rust-code-mcp": {
      "command": "/home/molaco/Documents/rust-code-mcp/target/release/file-search-mcp"
    }
  }
}
```

### Verifying Index Persistence

Check that the index and cache were created:
```bash
# On Linux:
ls -lh ~/.local/share/rust-code-mcp/search/
# Expected: index/ and cache/ directories

# On macOS:
ls -lh ~/Library/Application\ Support/rust-code-mcp/search/

# On Windows:
dir %APPDATA%\rust-code-mcp\search\
```

### Performance Benchmarks

Expected performance (Phase 1):
- **First index** (2 files): ~50-100ms
- **Second search** (no changes): **<10ms** (10x+ speedup)
- **After 1 file change**: ~15-20ms (only reindexes changed file)

Check detailed stats in logs:
```bash
RUST_LOG=info ./target/release/file-search-mcp
```

Look for: `Processing complete: Found=X, New/Changed=Y, Reindexed=Z, Unchanged=W`

## 🎓 Development Setup

### Prerequisites
- Rust (latest stable or nightly)
- **Optional**: Nix (for reproducible development environment)

### Setup Steps

#### Option 1: Using Existing Nix Shell (If you have one)
```bash
# If you already have a Nix dev shell with Rust:
cd /home/molaco/Documents/rust-code-mcp
cargo build --release
cargo test
```

See [NIX_INTEGRATION.md](docs/NIX_INTEGRATION.md) for details on integrating with existing Nix flakes.

#### Option 2: Standard Rust Setup
```bash
# 1. Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Clone and build
cd /home/molaco/Documents/rust-code-mcp
cargo build --release

# 3. Run tests
cargo test
```

### Qdrant Setup (Phase 5+)

**Embedded Mode (Default)**: No setup needed! Qdrant runs in-process.

**Remote Mode (Optional)**:
```bash
# Option 1: Using Nix (if you have it)
nix run nixpkgs#qdrant

# Option 2: Download binary
# See https://qdrant.tech/documentation/guides/installation/
```

Set mode via environment variable:
```bash
export QDRANT_MODE=embedded  # Default
# or
export QDRANT_MODE=remote
export QDRANT_URL=http://localhost:6333
```

## 🔄 Current Architecture

```
rust-code-mcp (Phase 7 Week 12)
├── MCP Server ✅
│   └── stdio transport
├── Parsing & Chunking ✅
│   ├── Tree-sitter AST parsing
│   ├── Symbol extraction (functions, structs, etc.)
│   ├── Call graph building
│   └── Context-enriched chunking
├── Embedding & Vector Search ✅
│   ├── Local embeddings (fastembed)
│   ├── Qdrant vector store
│   └── Semantic similarity search
├── Hybrid Search ✅
│   ├── RRF algorithm ✅
│   ├── VectorSearch wrapper ✅
│   ├── Bm25Search module ✅
│   └── Parallel BM25 + Vector fusion ✅
├── Persistent Storage ✅
│   ├── Tantivy Index (file-level)
│   ├── Qdrant (chunk-level)
│   └── Metadata Cache
└── Tools ✅
    ├── search (keyword with persistent index)
    ├── read_file_content (read file contents)
    ├── find_definition (locate symbols)
    ├── find_references (find symbol usage)
    ├── get_dependencies (analyze imports)
    ├── get_call_graph (call relationships)
    ├── analyze_complexity (code metrics)
    └── get_similar_code (semantic search)
```

## 🎯 Target Architecture (Week 16)

```
MCP Client (Claude)
    ↓
MCP Server (enhanced)
    ↓
Hybrid Search Coordinator
    ├─> Tantivy (BM25)
    └─> Qdrant (Vector)
        ↓
    RRF Fusion
        ↓
Indexing Pipeline
    ├─> Tree-sitter Parser
    ├─> Code Chunker
    ├─> Embedding Generator
    └─> Storage (Tantivy + Qdrant)
```

See [docs/SEARCH.md](docs/SEARCH.md) for complete architecture.

## 📊 Performance Targets

### MVP (Week 10)
- Index 100k LOC in <2 min
- Query latency <500ms (p95)
- Memory usage <2GB

### Production (Week 16)
- Index 1M LOC in <5 min
- Query latency <200ms (p95)
- Memory usage <4GB
- Retrieval accuracy >80%

## 🤝 Contributing

This is currently in early development. Contributions welcome after v0.1.0 release!

See [docs/NEW_PLAN.md](docs/NEW_PLAN.md) for development roadmap.

## 📄 License

MIT License (same as original file-search-mcp)

## 🙏 Acknowledgements

- **[file-search-mcp](https://github.com/Kurogoma4D/file-search-mcp)** by Kurogoma4D - Foundation for this project
- **[BloopAI/bloop](https://github.com/BloopAI/bloop)** - Reference implementation for patterns
- **[Tantivy](https://github.com/quickwit-oss/tantivy)** - Full-text search engine
- **[Qdrant](https://qdrant.tech/)** - Vector database
- **[RMCP](https://github.com/modelcontextprotocol/rust-sdk)** - MCP Rust SDK

## 🗺️ Roadmap

- **v0.1.0** (Week 16) - MVP with hybrid search
- **v0.2.0** (Month 5-6) - Multi-language support, web UI
- **v0.3.0** (Month 7-9) - Distributed indexing, custom embeddings
- **v1.0.0** (Month 10-12) - Production-ready, enterprise features

---

**Phase 0 (Week 1):** ✅ Complete - Setup & Planning
**Phase 1 (Week 2):** ✅ Complete - Persistent Index + Incremental Updates
**Phase 2 (Week 5-6):** ✅ Complete - Tree-sitter Parsing
**Phase 3 (Week 7):** ✅ Complete - Semantic Chunking
**Phase 4 (Week 8):** ✅ Complete - Local Embeddings
**Phase 5 (Week 9):** ✅ Complete - Vector Search
**Phase 6 (Week 10-11):** ✅ Complete - Hybrid Search with BM25 + Vector
**Phase 7 (Week 12-13):** ✅ Complete - Enhanced MCP Tools (8 total, resources deferred)
**Current Phase:** Testing Phase - Validate tools with MCP clients
**Next Milestone:** Phase 8 - Optimization & Release
**Target MVP:** Week 16 - Full hybrid search operational

**Key Documents:**
- [PHASE0_NOTES.md](docs/PHASE0_NOTES.md) - Codebase analysis & integration points
- [PHASE1_COMPLETE.md](docs/PHASE1_COMPLETE.md) - Persistent index + incremental updates
- [PHASE2_COMPLETE.md](docs/PHASE2_COMPLETE.md) - Tree-sitter parsing
- [PHASE3_COMPLETE.md](docs/PHASE3_COMPLETE.md) - Semantic chunking
- [PHASE4_COMPLETE.md](docs/PHASE4_COMPLETE.md) - Local embeddings
- [PHASE5_COMPLETE.md](docs/PHASE5_COMPLETE.md) - Qdrant vector search
- [PHASE6_COMPLETE.md](docs/PHASE6_COMPLETE.md) - Hybrid search (BM25 + Vector)
- [PHASE7_WEEK12_COMPLETE.md](docs/PHASE7_WEEK12_COMPLETE.md) - Enhanced MCP tools (6 new tools)
- [PHASE7_COMPLETE.md](docs/PHASE7_COMPLETE.md) - Phase 7 complete (8 tools, resources deferred) ⭐ NEW
- [EXTRACTION_PLAN.md](docs/EXTRACTION_PLAN.md) - Bloop patterns to extract
