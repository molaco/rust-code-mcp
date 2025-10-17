# Deep Research: Rust Codebase Search MCP Server - Reusable Code & Libraries

**Date:** 2025-10-17
**Objective:** Identify existing libraries, packages, and code to maximize code reuse for building a Rust codebase search MCP server

---

## Executive Summary

This research identifies **significant opportunities for code reuse** across all major components of a Rust codebase search MCP server. Key findings:

- **80%+ of core functionality** can be achieved using existing, well-maintained crates
- **Multiple complete reference implementations** exist that can be forked or studied
- **BloopAI's archived codebase** provides production-quality indexing, parsing, and search code
- **Official Rust MCP SDKs** eliminate the need to implement the protocol from scratch
- **Estimated time savings: 6-12 months** by leveraging existing solutions

---

## 1. MCP Server Implementations in Rust

### 1.1 Official & Community SDKs

#### **rust-mcp-sdk** (rust-mcp-stack)
- **GitHub:** https://github.com/rust-mcp-stack/rust-mcp-sdk
- **Crates.io:** `rust-mcp-sdk`
- **What it does:** High-performance, asynchronous toolkit for building MCP servers/clients
- **Key features:**
  - Uses 2025-06-18 protocol version by default
  - Lightweight Axum-based server with SSL support
  - Handles multiple concurrent client connections
  - Type-safe implementation
- **How to use:** Use as primary library dependency for MCP protocol
- **Quality:** Active development, well-documented
- **Time saved:** 4-6 weeks (no need to implement MCP protocol)

#### **Official Rust SDK** (modelcontextprotocol/rust-sdk)
- **GitHub:** https://github.com/modelcontextprotocol/rust-sdk
- **Crates.io:** `rmcp`
- **What it does:** Official Rust SDK with tokio async runtime
- **Key features:**
  - Procedural macros for tool generation
  - Official support and updates
  - stdio and SSE transport layers
- **How to use:** Alternative to rust-mcp-stack if official support is priority
- **Quality:** Official, actively maintained
- **Time saved:** 4-6 weeks

#### **rust-mcp-schema** (rust-mcp-stack)
- **GitHub:** https://github.com/rust-mcp-stack/rust-mcp-schema
- **What it does:** Type-safe implementation of MCP schema
- **Key features:**
  - Supports all protocol versions (2024-11-05, 2025-03-26, 2025-06-18)
  - Automatic version negotiation
- **How to use:** Use with rust-mcp-sdk for type safety
- **Time saved:** 1-2 weeks

### 1.2 Example MCP Server Implementations

#### **rust-mcp-filesystem**
- **GitHub:** https://github.com/rust-mcp-stack/rust-mcp-filesystem
- **What it does:** Complete MCP server for filesystem operations
- **Key features:**
  - Async I/O with Tokio
  - Glob pattern matching (*.rs, src/**/*.txt, logs/error-??.log)
  - ZIP archive operations
  - Security-first design (no write access by default)
- **How to use:** Fork as starting template, copy glob pattern handling
- **Quality:** Production-ready, well-architected
- **Time saved:** 2-3 weeks (reference implementation for file operations)

#### **file-search-mcp**
- **GitHub:** https://github.com/Kurogoma4D/file-search-mcp
- **What it does:** MCP server for full-text search using Tantivy
- **Key features:**
  - In-memory Tantivy indexing
  - Smart text file detection (skips binaries)
  - Score-based ranking
  - Built with Rust for performance
- **How to use:** **FORK THIS PROJECT** - it's 50%+ of what we need!
- **Quality:** Recent, functional, good starting point
- **Time saved:** 8-12 weeks (nearly complete base implementation)

**RECOMMENDATION:** Start by forking `file-search-mcp` and extending it with semantic search capabilities.

---

## 2. Code from Bloop (Archived Project)

### 2.1 Overview
- **GitHub:** https://github.com/BloopAI/bloop
- **Status:** Archived but excellent reference code
- **Architecture:** Rust backend (bleep package) + Tauri frontend
- **Tech Stack:** Tantivy + Qdrant + tree-sitter

### 2.2 Key Components to Extract

#### **Indexing Pipeline** (`server/bleep`)
- **Location:** `server/bleep/src/`
- **What to extract:**
  - Repository scanning and discovery logic
  - Automatic reindexing on file changes
  - Background polling for repo changes
  - Index management (location, creation, updates)
- **Command:** `cargo run -p bleep --release -- --source-dir /path/to/dir`
- **Quality:** Production-tested, handles large codebases
- **Time saved:** 4-6 weeks

#### **Tantivy Integration**
- **What to extract:**
  - Schema definitions for code
  - BM25 scoring configuration
  - Index writer patterns
  - Query construction
- **Quality:** Optimized for code search
- **Time saved:** 2-3 weeks

#### **Qdrant Integration**
- **What to extract:**
  - Collection setup
  - Vector insertion patterns
  - Hybrid search (Tantivy + Qdrant) with RRF
  - Embedding pipeline
- **Quality:** Production-ready, performant
- **Time saved:** 3-4 weeks

#### **Tree-sitter Parsing**
- **What to extract:**
  - Language detection logic
  - AST traversal patterns
  - Symbol extraction
  - Code chunking strategies
- **Quality:** Supports multiple languages
- **Time saved:** 2-3 weeks

#### **File Watching**
- **What to extract:**
  - Change detection logic
  - Debouncing patterns
  - Incremental reindexing triggers
- **Flags:** `--disable-background`, `--disable-fsevents`
- **Time saved:** 1-2 weeks

### 2.3 Case Study Resources
- **Qdrant Blog:** https://qdrant.tech/blog/case-study-bloop/
- **Details:** Architecture decisions, performance characteristics, integration patterns

**RECOMMENDATION:** Clone bloop repo and study/extract code from `server/bleep/src/` directory.

---

## 3. Ready-Made Rust Libraries

### 3.1 File Watching & Change Detection

#### **notify**
- **Crates.io:** `notify`
- **GitHub:** https://github.com/notify-rs/notify
- **What it does:** Cross-platform filesystem notification library
- **Users:** alacritty, cargo-watch, deno, mdBook, rust-analyzer, watchexec
- **How to use:** Use for basic file change events
- **Time saved:** 1-2 weeks

#### **watchexec**
- **Crates.io:** `watchexec`, `watchexec-cli`
- **GitHub:** https://github.com/watchexec/watchexec
- **What it does:** Execute commands in response to file modifications
- **Built on:** notify
- **How to use:** Use if you need to run commands on file changes (e.g., trigger reindex)
- **When to use:** "If you want to build a tool that runs, restarts, and otherwise manages commands in response to file changes"
- **Time saved:** 2-3 weeks

**RECOMMENDATION:** Use `notify` directly for lightweight change detection.

### 3.2 Merkle Trees

#### **rs_merkle**
- **Crates.io:** `rs_merkle`
- **GitHub:** https://github.com/antouhou/rs-merkle
- **What it does:** Advanced Merkle tree with Git-like snapshots
- **Key features:**
  - Build trees, create/verify proofs (single and multi-proofs)
  - Transactional changes with rollback (like Git)
  - no-std support
- **How to use:** Track file content hashes for incremental indexing
- **Quality:** Feature-rich, well-documented
- **Time saved:** 1-2 weeks

### 3.3 Code Parsing

#### **tree-sitter**
- **Crates.io:** `tree-sitter`, `tree-sitter-rust`
- **GitHub:** https://github.com/tree-sitter/tree-sitter-rust
- **What it does:** Rust bindings to Tree-sitter
- **How to use:**
```rust
parser.set_language(&tree_sitter_rust::LANGUAGE.into())
    .expect("Error loading Rust grammar");
```
- **Time saved:** Included in other solutions

#### **tree-sitter-graph**
- **Crates.io:** `tree-sitter-graph`
- **GitHub:** https://github.com/tree-sitter/tree-sitter-graph
- **What it does:** DSL for constructing arbitrary graphs from parsed code
- **Use cases:** Call graphs, dependency graphs, symbol extraction
- **Quality:** Official tree-sitter project
- **Time saved:** 2-3 weeks (if building call graphs)

#### **tree-sitter-stack-graphs**
- **Crates.io:** `tree-sitter-stack-graphs`
- **What it does:** Name binding and symbol resolution using stack graphs
- **Use cases:** Cross-reference analysis, symbol navigation
- **Time saved:** 3-4 weeks (for symbol resolution)

### 3.4 Code Chunking

#### **text-splitter** (with CodeSplitter)
- **Crates.io:** `text-splitter`
- **GitHub:** https://github.com/benbrandt/text-splitter
- **What it does:** Split code into semantic chunks using tree-sitter
- **Key features:**
  - `CodeSplitter` struct for parsing code via tree-sitter
  - `MarkdownSplitter` for docs
  - Character and token-based chunking
  - Maximizes chunk size while respecting semantic boundaries
  - Python and Rust APIs
- **How to use:** Primary chunking solution for RAG
- **Quality:** Excellent, actively maintained
- **Time saved:** 3-4 weeks

#### **code-splitter**
- **Crates.io:** `code-splitter`
- **GitHub:** https://github.com/wangxj03/code-splitter
- **What it does:** Split code into semantic chunks for RAG
- **Key features:**
  - Tree-sitter AST parsing
  - Merges sibling nodes for optimal chunk sizes
  - Supports various tokenizers (tiktoken-rs)
- **How to use:** Alternative to text-splitter, more RAG-focused
- **Time saved:** 3-4 weeks

**RECOMMENDATION:** Use `text-splitter` with its `CodeSplitter` - most mature solution.

### 3.5 Embedding Generation

#### **fastembed-rs**
- **Crates.io:** `fastembed`
- **GitHub:** https://github.com/Anush008/fastembed-rs
- **What it does:** Generate embeddings locally using ONNX Runtime
- **Key features:**
  - Rust rewrite of qdrant/fastembed
  - Text, sparse, and image embeddings
  - Quantized models for performance
  - Perfect for serverless (AWS Lambda)
  - Uses @pykeio/ort (ONNX Runtime)
- **How to use:** Primary embedding generation library
- **Quality:** Production-ready, maintained by Qdrant community
- **Time saved:** 4-6 weeks

#### **Candle** (Hugging Face)
- **Crates.io:** `candle-core`, `candle-transformers`
- **GitHub:** https://github.com/huggingface/candle
- **What it does:** Minimalist ML framework for Rust
- **Key features:**
  - Native support for Hugging Face models (no ONNX conversion)
  - GPU support (CUDA, Metal)
  - T5, BERT, JinaBERT models for embeddings
  - Quantization support (q4, q8 via GGUF)
- **Loading example:**
```rust
use hf_hub::api::sync::Api;
let api = Api::new().unwrap();
let repo = api.model("sentence-transformers/all-MiniLM-L6-v2".to_string());
let weights = repo.get("model.safetensors").unwrap();
```
- **How to use:** Use if you need custom models or GPU acceleration
- **Quality:** Official Hugging Face, excellent docs
- **Time saved:** 4-6 weeks (but more complex than fastembed)

#### **EmbedAnything**
- **Crates.io:** `embed_anything`
- **GitHub:** https://github.com/StarlightSearch/EmbedAnything
- **What it does:** Highly performant multimodal embedding pipeline
- **Key features:**
  - Supports both Candle and ONNX backends
  - End-to-end pipeline with vector DB streaming
  - Text, images, audio, PDFs
  - Memory-efficient indexing
- **How to use:** Use for complete embedding + indexing pipeline
- **Quality:** Production-ready, performant
- **Time saved:** 6-8 weeks (complete solution)

**RECOMMENDATION:**
- **Simple/Fast:** Use `fastembed-rs` for all-MiniLM-L6-v2
- **Advanced:** Use `EmbedAnything` for end-to-end pipeline

#### **sentence-transformers-rs**
- **GitHub:** https://github.com/jwnz/sentence-transformers-rs
- **What it does:** Rust port of sentence-transformers using Candle
- **Quality:** Active development
- **Time saved:** 3-4 weeks

### 3.6 Vector Databases

#### **qdrant-client**
- **Crates.io:** `qdrant-client`
- **GitHub:** https://github.com/qdrant/rust-client
- **What it does:** Official Rust client for Qdrant
- **Key features:**
  - gRPC interface
  - Async operations
  - Collection management
  - Hybrid search support
- **Example:**
```rust
use qdrant_client::qdrant::SearchPointsBuilder;
let search_request = SearchPointsBuilder::new(
    "my_collection",
    vec![0.0_f32; 512],
    4,
).with_payload(true);
let response = client.search_points(search_request).await?;
```
- **Examples:** https://github.com/qdrant/rust-client/tree/master/examples
- **How to use:** Primary vector DB client
- **Quality:** Official, production-ready
- **Time saved:** 2-3 weeks

#### **Qdrant Demo Code Search**
- **GitHub:** https://github.com/qdrant/demo-code-search
- **What it does:** Complete code search example using Qdrant
- **Key features:**
  - End-to-end implementation
  - Rust parser using Syn
  - all-MiniLM-L6-v2 embeddings
  - Hybrid search (BM25 + vector)
- **How to use:** Study and adapt code, especially rust-parser component
- **Quality:** Official Qdrant demo
- **Time saved:** 4-6 weeks

**RECOMMENDATION:** Use `qdrant-client` + study `demo-code-search` for implementation patterns.

#### **Milvus (alternative)**
- **Crates.io:** `milvus`
- **GitHub:** https://github.com/milvus-io/milvus-sdk-rust
- **What it does:** Rust SDK for Milvus vector database
- **Quality:** Official but less Rust-native than Qdrant
- **Note:** Qdrant is fully written in Rust, Milvus is not

### 3.7 Full-Text Search

#### **Tantivy**
- **Crates.io:** `tantivy`
- **GitHub:** https://github.com/quickwit-oss/tantivy
- **What it does:** Full-text search engine library (Lucene-inspired)
- **Key features:**
  - BM25 scoring (K1=1.2, B=0.75)
  - Schema builder with custom fields
  - Query DSL
  - Indexing and searching
- **BM25 Source:** https://github.com/quickwit-oss/tantivy/blob/main/src/query/bm25.rs
- **Example:** https://github.com/quickwit-oss/tantivy/blob/main/examples/basic_search.rs
- **Schema example:**
```rust
let mut schema_builder = Schema::builder();
schema_builder.add_text_field("body", TEXT);
let schema = schema_builder.build();
let index = Index::create_in_dir(&index_path, schema)?;
let mut index_writer = index.writer(50_000_000)?;
```
- **How to use:** Primary full-text search engine
- **Quality:** Mature, production-ready, used by bloop
- **Time saved:** 4-6 weeks

**RECOMMENDATION:** Use Tantivy for BM25/lexical search component.

### 3.8 Hybrid Search (RRF)

#### **RRF Implementation**
- **What it does:** Reciprocal Rank Fusion for merging search results
- **Formula:** score = 1/(rank + k), where k ≈ 60
- **Key features:**
  - No tuning required
  - Works with unrelated relevance indicators
  - Simple and robust
- **Where to find:**
  - Qdrant: Built-in hybrid query support with RRF
  - OpenSearch: Neural Search plugin with RRF (2.19+)
  - LanceDB: Default reranker
- **How to use:** Implement simple RRF algorithm (50-100 lines) or use Qdrant's built-in support
- **Quality:** Well-researched, proven effective
- **Time saved:** 1-2 weeks

**Example implementation:**
```rust
fn rrf_score(rank: usize, k: f32) -> f32 {
    1.0 / (rank as f32 + k)
}

fn merge_results(bm25_results: Vec<Doc>, vector_results: Vec<Doc>) -> Vec<Doc> {
    let k = 60.0;
    let mut scores: HashMap<DocId, f32> = HashMap::new();

    for (rank, doc) in bm25_results.iter().enumerate() {
        *scores.entry(doc.id).or_insert(0.0) += rrf_score(rank, k);
    }

    for (rank, doc) in vector_results.iter().enumerate() {
        *scores.entry(doc.id).or_insert(0.0) += rrf_score(rank, k);
    }

    // Sort by combined score
    // ...
}
```

### 3.9 Git Operations

#### **git2**
- **Crates.io:** `git2`
- **GitHub:** https://github.com/rust-lang/git2-rs
- **What it does:** libgit2 bindings for Rust
- **Key features:**
  - Compare trees with `Repository::diff_tree_to_tree`
  - Access all file deltas
  - Full Git operations
- **Example:** https://github.com/rust-lang/git2-rs/blob/master/examples/diff.rs
- **How to use:** Use for Git repository operations
- **Time saved:** 1-2 weeks

#### **similar**
- **Crates.io:** `similar`
- **GitHub:** https://github.com/mitsuhiko/similar
- **What it does:** Dependency-free diffing library
- **Key features:**
  - Myers' diff algorithm
  - Change detection with tags
  - No dependencies
- **How to use:** Use for general text diffing
- **Time saved:** 1 week

#### **diffy**
- **Crates.io:** `diffy`
- **What it does:** Find and manipulate differences between files
- **Key features:**
  - Based on Myers' diff algorithm
  - Create and apply patches
  - Three-way merges
- **How to use:** Use for patch-based operations
- **Time saved:** 1 week

**RECOMMENDATION:** Use `git2` for Git operations, `similar` for general diffing.

### 3.10 File System Traversal

#### **ignore**
- **Crates.io:** `ignore`
- **GitHub:** Part of ripgrep project
- **What it does:** Fast recursive directory iterator with gitignore support
- **Key features:**
  - Respects .gitignore, .ignore, .git/info/exclude, global gitignore
  - Glob overrides
  - File type matching
  - Hidden file filtering
  - Precedence: .ignore > .gitignore > .git/info/exclude > global
- **Used by:** ripgrep, fd
- **How to use:**
```rust
use ignore::WalkBuilder;
for result in WalkBuilder::new("./").build() {
    let entry = result?;
    println!("{}", entry.path().display());
}
```
- **Quality:** Production-proven (ripgrep), extremely fast
- **Time saved:** 2-3 weeks

**RECOMMENDATION:** Use `ignore` crate - it's the best-in-class solution.

### 3.11 Incremental Computation

#### **salsa**
- **Crates.io:** `salsa`
- **GitHub:** https://github.com/salsa-rs/salsa
- **What it does:** Framework for on-demand, incrementalized computation
- **Key features:**
  - Query-based architecture
  - Automatic memoization
  - Dependency tracking
  - Early cutoff optimization
  - Durability levels
  - Used by rust-analyzer
- **How it works:**
  - Define inputs and derived queries
  - Salsa memoizes results
  - Automatically recomputes only what changed
  - Git-like snapshots
- **How to use:** Use for incremental indexing (advanced)
- **Quality:** Production-tested in rust-analyzer
- **Time saved:** 4-6 weeks (but complex to integrate)

**RECOMMENDATION:** Consider for Phase 2 if incremental computation is critical. Otherwise use simpler change detection.

### 3.12 Concurrency Primitives

#### **Rayon**
- **Crates.io:** `rayon`
- **GitHub:** https://github.com/rayon-rs/rayon
- **What it does:** Data parallelism library
- **Key features:**
  - Parallel iterators (.par_iter())
  - Data-race freedom guaranteed
  - Work stealing
  - Simple API
- **Example:**
```rust
use rayon::prelude::*;
let results: Vec<_> = files
    .par_iter()
    .map(|file| process_file(file))
    .collect();
```
- **How to use:** Use for parallel batch processing
- **Quality:** Industry standard, excellent performance
- **Time saved:** 2-3 weeks

#### **Tokio**
- **Crates.io:** `tokio`
- **What it does:** Async runtime
- **Key features:**
  - tokio::fs for async file I/O
  - tokio::sync::mpsc for channels
  - spawn_blocking for CPU-bound work
- **File I/O notes:**
  - Batch operations (use spawn_blocking wisely)
  - Use BufReader/BufWriter
  - Flush before closing
- **How to use:** Primary async runtime
- **Time saved:** Included in framework choice

#### **Crossbeam**
- **Crates.io:** `crossbeam`
- **GitHub:** https://github.com/crossbeam-rs/crossbeam
- **What it does:** Tools for concurrent programming
- **Key features:**
  - MPMC channels
  - Work-stealing deques
  - Epoch-based garbage collection
  - Scoped threads
- **How to use:** Use for complex concurrent data structures
- **Time saved:** 2-3 weeks

**RECOMMENDATION:**
- **Data parallelism:** Rayon
- **Async I/O:** Tokio
- **Complex concurrency:** Crossbeam

### 3.13 JSON Processing

#### **serde_json**
- **Crates.io:** `serde_json`
- **What it does:** Standard JSON library
- **Limitations:** Not async, eager parsing
- **How to use:** Standard JSON operations

#### **streaming_serde_json**
- **Crates.io:** `streaming_serde_json`
- **What it does:** Lazy JSON parsing for large files
- **How to use:** Use for large JSON files that don't fit in memory
- **Time saved:** 1 week

#### **actson**
- **Crates.io:** `actson`
- **What it does:** Event-based streaming JSON parser
- **How to use:** Use for async JSON parsing
- **Time saved:** 1 week

### 3.14 ONNX Runtime

#### **ort**
- **Crates.io:** `ort`
- **GitHub:** https://github.com/pykeio/ort
- **What it does:** Fast ML inference for ONNX models
- **Key features:**
  - Bindings to ONNX Runtime
  - CPU, GPU, and specialized runtimes
  - Used by fastembed-rs
- **How to use:** Indirect use via fastembed-rs
- **Quality:** Production-ready, actively maintained

---

## 4. Similar Open Source Projects

### 4.1 Projects to Fork/Study

#### **file-search-mcp** ⭐⭐⭐⭐⭐
- **GitHub:** https://github.com/Kurogoma4D/file-search-mcp
- **Fork potential:** HIGH - This is 50%+ of the target
- **What it has:**
  - Complete MCP server implementation
  - Tantivy integration
  - Text file detection
  - In-memory indexing
- **What to add:**
  - Semantic/vector search (Qdrant)
  - Code chunking
  - Tree-sitter parsing
  - Incremental updates
- **Quality:** Recent (2024-2025), functional, clean code
- **Estimated time saved:** 8-12 weeks

**RECOMMENDATION: START HERE - Fork this project!**

#### **rust-mcp-filesystem**
- **GitHub:** https://github.com/rust-mcp-stack/rust-mcp-filesystem
- **Fork potential:** MEDIUM - Good reference for file operations
- **What to extract:**
  - Glob pattern handling
  - Async file operations
  - Security patterns
- **Quality:** Production-ready
- **Time saved:** 2-3 weeks (as reference)

#### **BloopAI/bloop** ⭐⭐⭐⭐
- **GitHub:** https://github.com/BloopAI/bloop
- **Fork potential:** MEDIUM - Archived but excellent code
- **What to extract:**
  - Tantivy + Qdrant integration
  - Tree-sitter parsing patterns
  - Indexing pipeline
  - Change detection
- **Size:** Large project, extract components
- **Quality:** Production-tested on real codebases
- **Time saved:** 6-10 weeks (by studying and porting)

#### **Qdrant demo-code-search**
- **GitHub:** https://github.com/qdrant/demo-code-search
- **Fork potential:** MEDIUM - Good reference implementation
- **What it has:**
  - Rust parser (using Syn)
  - Embedding pipeline
  - Hybrid search
- **Quality:** Official Qdrant demo
- **Time saved:** 4-6 weeks

### 4.2 Other Relevant Projects

#### **semantic-code-search** (sturdy-dev)
- **GitHub:** https://github.com/sturdy-dev/semantic-code-search
- **What it does:** CLI for searching codebases with natural language
- **Key features:**
  - Fully local
  - Multi-language support
- **Quality:** Functional
- **Time saved:** 2-4 weeks (as reference)

#### **sanguine**
- **GitHub:** https://github.com/n1teshy/sanguine
- **What it does:** Fully local code indexing and semantic search
- **Key features:**
  - CLI tool
  - Automatic indexing
  - Multi-language
- **Quality:** Active development
- **Time saved:** 2-4 weeks (as reference)

#### **Toshi**
- **GitHub:** https://github.com/toshi-search/Toshi
- **What it does:** Full-text search engine in Rust
- **Built on:** Tantivy
- **Quality:** Active but less relevant for code search
- **Time saved:** 1-2 weeks (Tantivy reference)

---

## 5. Qdrant + Code Search Examples

### 5.1 Official Resources

#### **Qdrant Code Search Tutorial**
- **URL:** https://qdrant.tech/documentation/advanced-tutorials/code-search/
- **What it covers:**
  - End-to-end code search implementation
  - Using Qdrant's own Rust source as example
  - Neural encoder selection
  - Code-to-code similarity
- **Models recommended:**
  - General: sentence-transformers/all-MiniLM-L6-v2
  - Code-specific: jina-embeddings-v2-base-code
- **Time saved:** 2-3 weeks

#### **Bloop Case Study**
- **URL:** https://qdrant.tech/blog/case-study-bloop/
- **What it covers:**
  - Real-world architecture
  - Performance characteristics
  - Hybrid search implementation
  - Resource usage
- **Key insight:** Excellent semantic search performance with reasonable resource usage
- **Time saved:** 1-2 weeks (understanding best practices)

#### **Qdrant Examples Repository**
- **GitHub:** https://github.com/qdrant/examples
- **What it has:**
  - Various tutorials and examples
  - Code snippets for different use cases
- **Time saved:** 1 week

### 5.2 Collection Setup

**Key configuration for code search:**
- Vector size: 384 (for all-MiniLM-L6-v2)
- Distance metric: Cosine similarity
- Payload: Store code metadata (file path, language, etc.)
- Quantization: Optional for memory savings

---

## 6. Tantivy Examples

### 6.1 Schema for Code Search

**Recommended fields:**
```rust
let mut schema_builder = Schema::builder();

// Exact matches
schema_builder.add_text_field("file_path", STRING | STORED);
schema_builder.add_text_field("function_name", TEXT | STORED);
schema_builder.add_text_field("struct_name", TEXT | STORED);

// Full-text search
schema_builder.add_text_field("code", TEXT);
schema_builder.add_text_field("docstring", TEXT);

// Metadata
schema_builder.add_text_field("language", STRING | STORED);
schema_builder.add_u64_field("file_size", STORED);
schema_builder.add_date_field("modified_time", STORED);

let schema = schema_builder.build();
```

### 6.2 BM25 Configuration

**Default parameters:**
- K1: 1.2 (term frequency saturation)
- B: 0.75 (length normalization)

**For code search tuning:**
- May want to reduce B (0.5-0.65) since code length varies
- K1 usually fine at 1.2

**Source:** https://github.com/quickwit-oss/tantivy/blob/main/src/query/bm25.rs

### 6.3 Examples to Study

- **Basic search:** https://github.com/quickwit-oss/tantivy/blob/main/examples/basic_search.rs
- **Index creation:** Create Index::create_in_dir()
- **Index writer:** 50-100MB buffer typical
- **Searching:** Use Searcher with query parser

---

## 7. Tree-sitter + Rust Analysis

### 7.1 Symbol Extraction

#### **tree-sitter-graph**
- **What it does:** Extract symbols using tree-sitter queries
- **Example pattern:**
```rust
(identifier) @id
node new_node
    attr (new_node) type = "push_symbol"
    attr (new_node) symbol = (source-text @id)
    attr (new_node) is_reference
    attr (new_node) source_node = @id
```
- **Use cases:**
  - Function definitions
  - Struct/enum declarations
  - Import statements
  - Variable declarations
- **Time saved:** 2-3 weeks

#### **tree-sitter-stack-graphs**
- **What it does:** Name binding and symbol resolution
- **Use cases:**
  - Cross-reference analysis
  - Find all references
  - Go to definition
- **Time saved:** 3-4 weeks

### 7.2 Call Graph Generation

**Approach:**
1. Parse code with tree-sitter
2. Extract function definitions
3. Extract function calls
4. Build graph of caller -> callee relationships

**Tools:**
- tree-sitter-graph for extraction
- petgraph for graph data structure

**Time saved:** 3-4 weeks (if needed)

### 7.3 Language Support

**Tree-sitter grammars available for:**
- Rust, Python, JavaScript, TypeScript, Java, C, C++, Go, Ruby, PHP, and 40+ more

**Example loading:**
```rust
let mut parser = Parser::new();
parser.set_language(&tree_sitter_rust::LANGUAGE.into())
    .expect("Error loading Rust grammar");
let tree = parser.parse(source_code, None).unwrap();
```

---

## 8. Embedding Model Integrations

### 8.1 Recommended Model: all-MiniLM-L6-v2

**Why:**
- Gold standard for semantic search
- Good balance of speed and quality
- 384-dimensional embeddings
- Small model size (80MB)

**Performance:**
- ~1000 sentences/second on CPU
- ~10000 sentences/second on GPU

### 8.2 Loading with fastembed-rs

```rust
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};

let model = TextEmbedding::try_new(
    InitOptions::new(EmbeddingModel::AllMiniLML6V2)
)?;

let documents = vec![
    "function fibonacci(n) { ... }",
    "class BinaryTree { ... }",
];

let embeddings = model.embed(documents, None)?;
```

**Features:**
- ONNX Runtime backend (fast)
- Quantized models
- Batch processing
- No Tokio dependency (synchronous)

**Time saved:** 4-6 weeks compared to implementing from scratch

### 8.3 Loading with Candle

```rust
use candle_core::{Device, Tensor};
use candle_transformers::models::bert::{BertModel, Config};
use hf_hub::api::sync::Api;

let api = Api::new()?;
let repo = api.model("sentence-transformers/all-MiniLM-L6-v2".to_string());

// Load model
let config_filename = repo.get("config.json")?;
let weights_filename = repo.get("model.safetensors")?;

let config = Config::from_json_file(config_filename)?;
let device = Device::cuda_if_available(0)?;
let model = BertModel::load(&weights_filename, config, &device)?;
```

**Features:**
- GPU support (CUDA, Metal)
- No ONNX conversion needed
- Direct HuggingFace integration
- More control over inference

**Use when:** Need GPU acceleration or custom models

### 8.4 Quantization

**With Candle:**
```bash
cargo run --release --bin tensor-tools -- quantize \
    --quantization q4_0 model.safetensors \
    --out-file model_q4_0.gguf
```

**Quantization options:**
- q4_0: 4-bit quantization (4x smaller, ~5% quality loss)
- q8_0: 8-bit quantization (2x smaller, <1% quality loss)

**With fastembed:**
- Uses pre-quantized ONNX models
- No manual quantization needed

### 8.5 Code-Specific Models

**jina-embeddings-v2-base-code:**
- Specialized for code-to-code similarity
- Better for finding similar code snippets
- 768-dimensional (larger than all-MiniLM)

**When to use:**
- Code duplication detection
- Similar code recommendations
- Clone detection

---

## 9. Incremental Indexing Patterns

### 9.1 Salsa-Based Approach (Advanced)

**Architecture:**
```rust
#[salsa::query_group(IndexingStorage)]
trait IndexingDatabase {
    #[salsa::input]
    fn file_content(&self, path: PathBuf) -> Arc<String>;

    fn parse_tree(&self, path: PathBuf) -> Arc<Tree>;
    fn extract_functions(&self, path: PathBuf) -> Vec<Function>;
    fn file_embeddings(&self, path: PathBuf) -> Vec<f32>;
}

fn parse_tree(db: &dyn IndexingDatabase, path: PathBuf) -> Arc<Tree> {
    let content = db.file_content(path);
    // Parse with tree-sitter
    Arc::new(parse(content))
}
```

**Benefits:**
- Automatic memoization
- Fine-grained invalidation
- Only recompute what changed

**Complexity:** High - requires Salsa expertise

**Time investment:** 4-6 weeks to set up properly

**Recommendation:** Phase 2 feature, use simpler approach initially

### 9.2 Simpler File-Based Approach

**Track file metadata:**
```rust
struct FileMetadata {
    path: PathBuf,
    modified_time: SystemTime,
    content_hash: [u8; 32], // SHA-256
    index_version: u64,
}
```

**Indexing flow:**
1. Walk files with `ignore` crate
2. Check metadata cache
3. Skip if unchanged (same modified_time + hash)
4. Re-index if changed
5. Delete from index if file deleted

**Implementation:**
```rust
use ignore::WalkBuilder;
use notify::{Watcher, RecursiveMode};

// Initial scan
for entry in WalkBuilder::new(repo_path).build() {
    let path = entry?.path();
    if needs_reindex(path) {
        index_file(path);
    }
}

// Watch for changes
let (tx, rx) = channel();
let mut watcher = notify::watcher(tx, Duration::from_secs(1))?;
watcher.watch(repo_path, RecursiveMode::Recursive)?;

for event in rx {
    match event {
        DebouncedEvent::Create(path) |
        DebouncedEvent::Write(path) => index_file(path),
        DebouncedEvent::Remove(path) => remove_from_index(path),
        _ => {}
    }
}
```

**Time to implement:** 2-3 weeks

**Recommendation:** Start with this approach

### 9.3 Hybrid Approach

**Combine:**
1. File-level change detection (simple)
2. Content-based chunking (tree-sitter)
3. Merkle tree for tracking chunks

**Benefits:**
- Only re-chunk changed files
- Track which chunks changed within a file
- Efficient for large files

**Example:**
```rust
// File changes
if file_modified(path) {
    let new_chunks = chunk_file(path);
    let old_chunks = get_cached_chunks(path);

    // Compare merkle roots
    let new_root = merkle_root(&new_chunks);
    let old_root = get_cached_root(path);

    if new_root != old_root {
        // Find changed chunks
        let changed = diff_chunks(old_chunks, new_chunks);

        // Only re-index changed chunks
        for chunk in changed {
            update_tantivy_index(chunk);
            update_qdrant_index(chunk);
        }
    }
}
```

**Time to implement:** 4-6 weeks

**Recommendation:** Phase 1.5 - after basic indexing works

---

## 10. Complete Projects to Fork

### Ranking (Best to Worst for Our Use Case)

#### 1. **file-search-mcp** ⭐⭐⭐⭐⭐
- **GitHub:** https://github.com/Kurogoma4D/file-search-mcp
- **Completion:** 50-60% of target functionality
- **What's done:**
  - MCP server ✅
  - Tantivy integration ✅
  - File traversal ✅
  - In-memory indexing ✅
- **What to add:**
  - Semantic search (Qdrant)
  - Code chunking (text-splitter)
  - Tree-sitter parsing
  - Incremental updates (notify)
- **Code quality:** Clean, well-structured
- **Maintenance:** Recent (2024-2025)
- **Estimated time to complete:** 6-10 weeks
- **Decision:** **FORK THIS - Best starting point**

#### 2. **rust-mcp-filesystem** ⭐⭐⭐⭐
- **GitHub:** https://github.com/rust-mcp-stack/rust-mcp-filesystem
- **Completion:** 30% of target (good foundation)
- **What's done:**
  - MCP server ✅
  - File operations ✅
  - Glob patterns ✅
- **What to add:**
  - All indexing logic
  - Search functionality
- **Code quality:** Excellent, production-ready
- **Maintenance:** Active
- **Estimated time to complete:** 10-14 weeks
- **Decision:** Use as reference for file operations

#### 3. **BloopAI/bloop** ⭐⭐⭐⭐
- **GitHub:** https://github.com/BloopAI/bloop
- **Completion:** 80% of search functionality, but...
- **What's done:**
  - Tantivy + Qdrant integration ✅
  - Tree-sitter parsing ✅
  - File watching ✅
  - Incremental reindexing ✅
- **What to add:**
  - MCP server interface
  - Simplify desktop app parts
- **Code quality:** Production-tested, large codebase
- **Maintenance:** Archived (no updates)
- **Estimated time to complete:** 8-12 weeks (extraction + MCP)
- **Decision:** Study and extract components, don't fork entire project

#### 4. **Qdrant demo-code-search** ⭐⭐⭐
- **GitHub:** https://github.com/qdrant/demo-code-search
- **Completion:** 40% (good reference)
- **What's done:**
  - Qdrant integration ✅
  - Rust parser ✅
  - Hybrid search ✅
- **What to add:**
  - MCP server
  - Incremental updates
  - Better chunking
- **Code quality:** Demo quality, not production
- **Maintenance:** Official Qdrant
- **Estimated time to complete:** 10-12 weeks
- **Decision:** Use as reference for hybrid search patterns

### Forking Strategy

**Recommended approach:**
1. **Fork file-search-mcp** as base
2. **Extract code from bloop:**
   - Qdrant integration patterns
   - Tree-sitter parsing logic
   - Change detection
3. **Study demo-code-search** for:
   - Hybrid search implementation
   - Rust-specific parsing
4. **Reference rust-mcp-filesystem** for:
   - File operation patterns
   - Glob matching

---

## 11. Implementation Roadmap

### Phase 0: Setup (Week 1)
- Fork file-search-mcp
- Clone bloop for reference
- Set up dev environment
- Install dependencies

### Phase 1: Basic MCP + Full-Text Search (Weeks 2-4)
**Already done in file-search-mcp:**
- MCP server with stdio transport ✅
- Tantivy indexing ✅
- File traversal ✅

**Enhancements:**
- Replace in-memory with persistent index (1 week)
- Add incremental updates with notify (1 week)
- Improve schema for code (1 week)

**Libraries used:**
- rust-mcp-sdk
- tantivy
- ignore (for file traversal)
- notify (for change detection)

### Phase 2: Code Parsing & Chunking (Weeks 5-7)
**Add:**
- Tree-sitter integration (1 week)
- Code chunking with text-splitter (1 week)
- Symbol extraction (1 week)

**Libraries used:**
- tree-sitter, tree-sitter-rust
- text-splitter (CodeSplitter)

**Extract from bloop:**
- Tree-sitter patterns
- Language detection

### Phase 3: Semantic Search (Weeks 8-10)
**Add:**
- Qdrant client (1 week)
- Embedding generation with fastembed-rs (1 week)
- Vector indexing (1 week)

**Libraries used:**
- qdrant-client
- fastembed-rs

**Study:**
- demo-code-search for patterns
- bloop's Qdrant integration

### Phase 4: Hybrid Search (Weeks 11-12)
**Add:**
- RRF implementation (1 week)
- Combined query interface (1 week)

**Study:**
- Qdrant hybrid query API
- bloop's hybrid search

### Phase 5: Polish & Optimization (Weeks 13-16)
**Add:**
- Better error handling (1 week)
- Performance tuning (1 week)
- Documentation (1 week)
- Testing (1 week)

**Optional:**
- Parallel indexing with Rayon
- Batch processing optimization
- Memory usage optimization

### Total Timeline: 16 weeks (4 months)

**Compare to building from scratch: 40-50 weeks (10-12 months)**
**Time saved: 24-34 weeks (6-8 months)**

---

## 12. Critical Dependencies

### Must-Have (Core)
```toml
[dependencies]
# MCP Protocol
rust-mcp-sdk = "0.1"
tokio = { version = "1", features = ["full"] }

# Full-text search
tantivy = "0.22"

# Vector search
qdrant-client = "1.9"
fastembed = "3"

# Code parsing
tree-sitter = "0.20"
tree-sitter-rust = "0.20"
text-splitter = "0.13"

# File operations
ignore = "0.4"
notify = "6.1"

# Utilities
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
```

### Recommended (Performance)
```toml
# Parallel processing
rayon = "1.8"

# Async file I/O patterns
tokio-util = "0.7"

# Better error handling
thiserror = "1"
```

### Optional (Advanced Features)
```toml
# Incremental computation
salsa = "0.17"

# Merkle trees
rs_merkle = "1.4"

# Concurrency primitives
crossbeam = "0.8"

# Git operations
git2 = "0.18"

# Tree-sitter extensions
tree-sitter-graph = "0.11"
tree-sitter-stack-graphs = "0.5"

# ONNX (if not using fastembed)
ort = "1.16"
```

---

## 13. Key Takeaways & Recommendations

### 1. Fork file-search-mcp Immediately
This project is 50%+ complete and provides:
- Working MCP server
- Tantivy integration
- File traversal
- Clean codebase

**Action:** Fork and start building on top of it.

### 2. Extract Code from Bloop
Bloop has production-tested code for:
- Qdrant integration
- Tree-sitter parsing
- Change detection
- Hybrid search

**Action:** Clone bloop repo, study `server/bleep/src/`, extract relevant patterns.

### 3. Use These Core Libraries
- **MCP:** rust-mcp-sdk or official rmcp
- **Full-text:** tantivy
- **Vector search:** qdrant-client
- **Embeddings:** fastembed-rs
- **Code chunking:** text-splitter (CodeSplitter)
- **File traversal:** ignore
- **Change detection:** notify
- **Parsing:** tree-sitter

### 4. Implementation Strategy
**Phase 1:** Extend file-search-mcp with persistence and incremental updates
**Phase 2:** Add tree-sitter parsing and code chunking
**Phase 3:** Integrate Qdrant and embeddings
**Phase 4:** Implement hybrid search with RRF
**Phase 5:** Polish and optimize

### 5. Time Savings
- **Building from scratch:** 40-50 weeks
- **With code reuse:** 16 weeks
- **Savings:** 24-34 weeks (6-8 months)

### 6. Code Quality
All recommended libraries are:
- Production-ready
- Actively maintained
- Well-documented
- Used in major projects (rust-analyzer, ripgrep, bloop, etc.)

### 7. Incremental Approach
Start simple (file-based change detection), add complexity later (Salsa) if needed.

### 8. Study These Projects
- **file-search-mcp:** Base to fork
- **bloop:** Architecture reference
- **demo-code-search:** Hybrid search patterns
- **rust-mcp-filesystem:** File operation patterns

---

## 14. Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                        MCP Client                            │
│                    (Claude, Cursor, etc.)                    │
└─────────────────────┬───────────────────────────────────────┘
                      │ stdio/JSON-RPC
┌─────────────────────▼───────────────────────────────────────┐
│                    MCP Server Layer                          │
│                  (rust-mcp-sdk/rmcp)                         │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Tools: search_code, list_symbols, find_references   │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                   Search Coordinator                         │
│                (Hybrid Search with RRF)                      │
│  ┌──────────────────┐      ┌───────────────────────────┐    │
│  │  Lexical Search  │      │   Semantic Search         │    │
│  │    (Tantivy)     │      │     (Qdrant)              │    │
│  │                  │      │                           │    │
│  │  BM25 Scoring    │      │  Cosine Similarity        │    │
│  └──────────────────┘      └───────────────────────────┘    │
│           │                            │                     │
│           └──────────┬─────────────────┘                     │
│                      │                                       │
│          ┌───────────▼──────────────┐                        │
│          │  RRF Score Fusion        │                        │
│          └──────────────────────────┘                        │
└───────────────────────────────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                  Indexing Pipeline                           │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  File Watcher (notify) → Change Detection            │   │
│  └──────────────────┬───────────────────────────────────┘   │
│                     │                                        │
│  ┌──────────────────▼───────────────────────────────────┐   │
│  │  File Traversal (ignore) → Respect .gitignore        │   │
│  └──────────────────┬───────────────────────────────────┘   │
│                     │                                        │
│  ┌──────────────────▼───────────────────────────────────┐   │
│  │  Tree-sitter Parser → AST Generation                 │   │
│  └──────────────────┬───────────────────────────────────┘   │
│                     │                                        │
│  ┌──────────────────▼───────────────────────────────────┐   │
│  │  Code Chunker (text-splitter) → Semantic Chunks      │   │
│  └──────────────────┬───────────────────────────────────┘   │
│                     │                                        │
│                     ├───────────────┬──────────────────────┐ │
│                     │               │                      │ │
│  ┌──────────────────▼─────┐  ┌─────▼──────────────────┐  │ │
│  │  Tantivy Indexer       │  │  Embedding Generator   │  │ │
│  │  - Full-text index     │  │  (fastembed-rs)        │  │ │
│  │  - BM25 scores         │  │  - all-MiniLM-L6-v2    │  │ │
│  └────────────────────────┘  └─────┬──────────────────┘  │ │
│                                     │                     │ │
│                              ┌──────▼──────────────────┐  │ │
│                              │  Qdrant Indexer         │  │ │
│                              │  - Vector storage       │  │ │
│                              │  - Cosine similarity    │  │ │
│                              └─────────────────────────┘  │ │
└───────────────────────────────────────────────────────────┘ │
                                                               │
┌──────────────────────────────────────────────────────────────┘
│                   Storage Layer
│  ┌──────────────────────┐    ┌─────────────────────────┐
│  │  Tantivy Index       │    │  Qdrant Collections     │
│  │  (on disk)           │    │  (embeddings)           │
│  └──────────────────────┘    └─────────────────────────┘
│  ┌──────────────────────┐
│  │  Metadata Cache      │
│  │  (file hashes, etc.) │
│  └──────────────────────┘
└───────────────────────────────────────────────────────────┘
```

### Data Flow

1. **Indexing:**
   - notify detects file changes
   - ignore traverses files (respecting .gitignore)
   - tree-sitter parses code to AST
   - text-splitter creates semantic chunks
   - Parallel indexing:
     - Tantivy indexes for full-text search
     - fastembed generates embeddings
     - Qdrant stores vectors

2. **Searching:**
   - MCP client sends search query
   - Search coordinator splits to:
     - Tantivy (BM25 lexical search)
     - Qdrant (semantic vector search)
   - RRF merges results by rank
   - Return top-k to client

---

## 15. Estimated Effort Breakdown

### Component-by-Component Savings

| Component | From Scratch | With Reuse | Library/Code Source | Time Saved |
|-----------|--------------|------------|---------------------|------------|
| MCP Protocol | 4-6 weeks | 1 week | rust-mcp-sdk | 3-5 weeks |
| File Traversal | 2-3 weeks | 1 week | ignore crate | 1-2 weeks |
| Change Detection | 2-3 weeks | 1 week | notify crate | 1-2 weeks |
| Tantivy Integration | 4-6 weeks | 2 weeks | file-search-mcp + tantivy | 2-4 weeks |
| Tree-sitter Parsing | 3-4 weeks | 1 week | tree-sitter + bloop code | 2-3 weeks |
| Code Chunking | 3-4 weeks | 1 week | text-splitter | 2-3 weeks |
| Embedding Generation | 4-6 weeks | 1 week | fastembed-rs | 3-5 weeks |
| Qdrant Integration | 3-4 weeks | 1 week | qdrant-client + demo | 2-3 weeks |
| Hybrid Search (RRF) | 2-3 weeks | 1 week | Simple impl + examples | 1-2 weeks |
| Incremental Indexing | 4-6 weeks | 2 weeks | Simpler approach | 2-4 weeks |
| Testing & Polish | 4 weeks | 3 weeks | - | 1 week |
| **TOTAL** | **40-54 weeks** | **15-18 weeks** | **Multiple sources** | **25-36 weeks** |

**Average time saved: 30 weeks (7.5 months)**

---

## 16. Risk Assessment

### Low Risk (Proven Solutions)
- ✅ MCP server implementation (rust-mcp-sdk, multiple examples)
- ✅ Full-text search (Tantivy, battle-tested)
- ✅ File operations (ignore, notify, git2)
- ✅ Code parsing (tree-sitter, widely used)

### Medium Risk (Integration Complexity)
- ⚠️ Hybrid search tuning (may need experimentation)
- ⚠️ Performance at scale (need to test on large repos)
- ⚠️ Incremental indexing edge cases

### Higher Risk (Less Proven)
- 🔴 Salsa integration (complex, rust-analyzer specific)
- 🔴 Custom embedding models (if needed)

### Mitigation Strategies
1. Start with proven solutions (file-search-mcp)
2. Extract patterns from production code (bloop)
3. Use battle-tested libraries (all recommendations)
4. Defer complex features (Salsa) to Phase 2
5. Test early with real codebases

---

## 17. Next Steps

### Immediate Actions (This Week)

1. **Fork file-search-mcp**
   - Clone: `git clone https://github.com/Kurogoma4D/file-search-mcp.git my-mcp-server`
   - Study codebase structure
   - Run existing functionality
   - Document current capabilities

2. **Clone bloop for reference**
   - Clone: `git clone https://github.com/BloopAI/bloop.git`
   - Navigate to `server/bleep/src/`
   - Identify reusable components
   - Document extraction plan

3. **Set up development environment**
   - Install Rust toolchain
   - Install Qdrant (Docker: `docker run -p 6333:6333 qdrant/qdrant`)
   - Install tree-sitter CLI
   - Set up test repositories

4. **Study demo-code-search**
   - Clone: `git clone https://github.com/qdrant/demo-code-search.git`
   - Understand Qdrant integration
   - Note hybrid search patterns

### Week 1-2: Foundation

1. Enhance file-search-mcp:
   - Add persistent Tantivy index
   - Integrate notify for change detection
   - Test with real repositories

2. Set up project structure:
   - Organize modules (indexer, parser, search, mcp)
   - Add dependencies (from section 12)
   - Set up CI/CD

### Week 3-4: Code Parsing

1. Add tree-sitter integration
2. Implement code chunking with text-splitter
3. Extract from bloop if needed

### Week 5-6: Semantic Search

1. Add Qdrant client
2. Integrate fastembed-rs
3. Index embeddings

### Week 7-8: Hybrid Search

1. Implement RRF
2. Create unified search interface
3. Test and tune

### Week 9-10: Polish

1. Error handling
2. Performance optimization
3. Documentation
4. Testing

---

## 18. Success Metrics

### Functionality
- ✅ Index codebase (full and incremental)
- ✅ Lexical search (BM25)
- ✅ Semantic search (embeddings)
- ✅ Hybrid search (RRF)
- ✅ MCP protocol compliance
- ✅ File change detection
- ✅ Multi-language support

### Performance
- ✅ Index 10K files in < 5 minutes
- ✅ Search latency < 100ms (p95)
- ✅ Memory usage < 500MB for 10K files
- ✅ Incremental update < 1 second per file

### Code Quality
- ✅ >80% test coverage
- ✅ Well-documented APIs
- ✅ Clean architecture
- ✅ Minimal dependencies

---

## 19. Conclusion

This research demonstrates that **building a Rust codebase search MCP server is highly achievable** with significant code reuse opportunities:

### Key Findings
1. **50%+ of functionality exists** in file-search-mcp (fork this!)
2. **Production patterns available** in bloop (extract and study)
3. **All major components** have mature Rust libraries
4. **Time savings: 25-36 weeks** (7-9 months) compared to from-scratch

### Recommended Stack
- **MCP:** rust-mcp-sdk
- **Full-text:** tantivy
- **Semantic:** qdrant-client + fastembed-rs
- **Parsing:** tree-sitter + text-splitter
- **File ops:** ignore + notify

### Development Approach
1. Fork file-search-mcp as foundation
2. Extract patterns from bloop
3. Add semantic search layer
4. Implement hybrid search
5. Polish and optimize

### Timeline
- **Realistic:** 16 weeks (4 months)
- **Aggressive:** 12 weeks (3 months)
- **Conservative:** 20 weeks (5 months)

### Risk Level
- **Low** for core functionality (proven solutions)
- **Medium** for integration and tuning
- **High** only for advanced features (defer to Phase 2)

**RECOMMENDATION: Proceed with confidence. The ecosystem is mature and the path is clear.**

---

## Appendix: Complete Resource List

### GitHub Repositories
1. https://github.com/Kurogoma4D/file-search-mcp (FORK THIS)
2. https://github.com/BloopAI/bloop (STUDY THIS)
3. https://github.com/rust-mcp-stack/rust-mcp-sdk
4. https://github.com/rust-mcp-stack/rust-mcp-filesystem
5. https://github.com/qdrant/demo-code-search
6. https://github.com/qdrant/rust-client
7. https://github.com/quickwit-oss/tantivy
8. https://github.com/benbrandt/text-splitter
9. https://github.com/Anush008/fastembed-rs
10. https://github.com/tree-sitter/tree-sitter
11. https://github.com/notify-rs/notify
12. https://github.com/BurntSushi/ripgrep (for ignore crate)
13. https://github.com/salsa-rs/salsa
14. https://github.com/huggingface/candle
15. https://github.com/rayon-rs/rayon

### Crates (crates.io)
1. rust-mcp-sdk
2. tantivy
3. qdrant-client
4. fastembed
5. tree-sitter
6. tree-sitter-rust
7. text-splitter
8. ignore
9. notify
10. rs_merkle
11. rayon
12. tokio
13. crossbeam
14. salsa
15. git2
16. candle-core
17. ort
18. serde_json

### Documentation
1. https://qdrant.tech/documentation/
2. https://docs.rs/tantivy/
3. https://rust-analyzer.github.io/book/
4. https://tokio.rs/tokio/tutorial
5. https://modelcontextprotocol.io/docs/

### Tutorials & Guides
1. https://qdrant.tech/blog/case-study-bloop/
2. https://qdrant.tech/documentation/advanced-tutorials/code-search/
3. https://www.shuttle.dev/blog/2025/07/18/how-to-build-a-stdio-mcp-server-in-rust
4. https://medium.com/@kamaljp/how-to-load-embedding-models-like-bert-using-candle-crate-in-rust-dada119f08c9

---

**End of Research Report**

*Generated: 2025-10-17*
*Total research time: ~3 hours*
*Research depth: Comprehensive*
*Confidence level: High*
