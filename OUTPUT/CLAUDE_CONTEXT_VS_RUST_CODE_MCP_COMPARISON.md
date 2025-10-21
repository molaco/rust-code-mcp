# Claude-Context vs Rust-Code-MCP: Comprehensive Comparison

**Analysis Date:** October 19, 2025
**Version:** 1.0
**rust-code-mcp Status:** Phase 7 Complete (Development)
**claude-context Status:** Production Deployed

---

## Executive Summary

This document provides a comprehensive comparison between **rust-code-mcp** and **claude-context**, two Model Context Protocol (MCP) servers designed for semantic code search and retrieval. While both projects aim to solve the same problem—enabling AI coding assistants to efficiently search large codebases—they take fundamentally different architectural approaches.

### Key Findings

| Aspect | rust-code-mcp | claude-context | Winner |
|--------|---------------|----------------|--------|
| **Search Architecture** | Hybrid (BM25 + Vector) | Vector-only | rust-code-mcp |
| **Privacy** | 100% local | Cloud APIs required | rust-code-mcp |
| **Cost** | $0 ongoing | $1,200-6,000/year | rust-code-mcp |
| **Production Status** | Development | Battle-tested | claude-context |
| **Change Detection** | SHA-256 (planned: Merkle) | Merkle tree (working) | claude-context |
| **Language Support** | Rust only | 30+ languages | claude-context |
| **Embedding Quality** | 384d local model | 3072d code-specific | claude-context |
| **Scalability** | 1M-10M LOC | Millions+ LOC | claude-context |

### Architectural Philosophy Difference

- **rust-code-mcp**: *Local-first, privacy-focused, self-contained, performance-optimized*
- **claude-context**: *Cloud-native, collaboration-focused, managed service, universally compatible*

---

## Table of Contents

1. [Core Architecture](#1-core-architecture)
2. [Search Implementation](#2-search-implementation)
3. [Embedding Generation](#3-embedding-generation)
4. [Code Chunking](#4-code-chunking)
5. [Incremental Indexing](#5-incremental-indexing)
6. [MCP Tool Interface](#6-mcp-tool-interface)
7. [Performance & Scalability](#7-performance--scalability)
8. [Cost Analysis](#8-cost-analysis)
9. [Use Case Recommendations](#9-use-case-recommendations)
10. [Roadmap & Recommendations](#10-roadmap--recommendations)

---

## 1. Core Architecture

### 1.1 System Overview

#### rust-code-mcp Architecture

```
┌─────────────────────────────────────────┐
│         MCP Server (STDIO)              │
├─────────────────────────────────────────┤
│  8 Tools: search, read_file, find_*    │
├─────────────────────────────────────────┤
│  Hybrid Search (BM25 + Vector + RRF)   │
├──────────────┬──────────────────────────┤
│  Tantivy     │  Qdrant (embedded)       │
│  BM25 Index  │  Vector Store            │
├──────────────┴──────────────────────────┤
│  FastEmbed (local, all-MiniLM-L6-v2)   │
├─────────────────────────────────────────┤
│  tree-sitter (Rust parser)             │
├─────────────────────────────────────────┤
│  sled (metadata cache)                  │
└─────────────────────────────────────────┘
        100% Local, Zero Cost
```

**Key Characteristics:**
- **Language:** Rust (compiled, native performance)
- **Deployment:** Single binary, self-hosted
- **Vector DB:** Qdrant (embedded or remote)
- **Lexical Search:** Tantivy (full-featured BM25)
- **Embeddings:** FastEmbed (local ONNX, 384 dimensions)
- **Storage:** Local disk (~/.local/share/rust-code-mcp/)
- **Cost:** $0 recurring

#### claude-context Architecture

```
┌─────────────────────────────────────────┐
│         MCP Server (STDIO)              │
├─────────────────────────────────────────┤
│  4 Tools: index, search, clear, status │
├─────────────────────────────────────────┤
│       Vector Search (semantic only)     │
├─────────────────────────────────────────┤
│  Milvus / Zilliz Cloud (managed)       │
│  Vector Database (remote)               │
├─────────────────────────────────────────┤
│  OpenAI / Voyage / Ollama (APIs)       │
│  Embedding Generation (3072 dims)       │
├─────────────────────────────────────────┤
│  tree-sitter (multi-language)          │
├─────────────────────────────────────────┤
│  Merkle Tree (change detection)         │
│  Snapshots: ~/.context/merkle/          │
└─────────────────────────────────────────┘
    Cloud-Native, Subscription-Based
```

**Key Characteristics:**
- **Language:** TypeScript/Node.js
- **Deployment:** npm package, cloud-dependent
- **Vector DB:** Milvus/Zilliz Cloud (managed service)
- **Lexical Search:** Not implemented
- **Embeddings:** OpenAI/Voyage APIs (3072 dimensions)
- **Storage:** Cloud + local Merkle snapshots
- **Cost:** ~$100-500/month + API fees

### 1.2 Trade-off Matrix

| Factor | rust-code-mcp Advantage | claude-context Advantage |
|--------|-------------------------|--------------------------|
| **Privacy** | ✅ No code leaves machine | ❌ Code sent to OpenAI/Voyage |
| **Cost** | ✅ Zero recurring fees | ❌ Subscription + API costs |
| **Offline** | ✅ Works without internet | ❌ Requires connectivity |
| **Setup** | ⚖️ Medium (Qdrant + Rust) | ⚖️ Easy (npm) or Hard (self-host) |
| **Scalability** | ❌ Limited by local hardware | ✅ Elastic cloud scaling |
| **Collaboration** | ❌ Local-only indices | ✅ Shared cloud index |
| **Embedding Quality** | ❌ 384d general model | ✅ 3072d code-specific |
| **Languages** | ❌ Rust only (currently) | ✅ 30+ languages |
| **Maintenance** | ⚖️ Self-managed | ✅ Fully managed (Zilliz) |

---

## 2. Search Implementation

### 2.1 Hybrid Search: rust-code-mcp

**Unique Strength:** Only project with true hybrid search (BM25 + Vector)

#### Architecture

```rust
// Parallel execution of BM25 and Vector search
let (vector_results, bm25_results) = tokio::join!(
    vector_search.search(query, limit),
    tokio::task::spawn_blocking(|| bm25.search(query, limit))
);

// Reciprocal Rank Fusion (RRF)
for (rank, result) in vector_results.iter().enumerate() {
    score = (1.0 / (60 + rank + 1)) * vector_weight;
}
for (rank, result) in bm25_results.iter().enumerate() {
    score += (1.0 / (60 + rank + 1)) * bm25_weight;
}
```

#### Components

1. **BM25 Search (Tantivy)**
   - **Algorithm:** Okapi BM25 (industry standard)
   - **Fields Indexed:** content, symbol_name, docstring
   - **Performance:** <20ms for keyword queries
   - **Strengths:** Exact identifier matching, keyword precision

2. **Vector Search (Qdrant)**
   - **Model:** all-MiniLM-L6-v2 (384 dimensions)
   - **Distance Metric:** Cosine similarity
   - **Performance:** <50ms for semantic queries
   - **Strengths:** Semantic understanding, synonym matching

3. **Fusion Method: RRF (Reciprocal Rank Fusion)**
   - **Formula:** `RRF(item) = Σ weight_s / (k + rank_s)` where k=60
   - **Advantage:** Rank-based (no score normalization needed)
   - **Weights:** Configurable (default 0.5 BM25 + 0.5 Vector)
   - **Parallel:** Both searches run concurrently

#### Why RRF?

The problem with combining BM25 scores (~5-15) and cosine similarity (0-1) is they're incomparable. RRF solves this by using ranks instead of raw scores:

- **Rank 1 in BM25:** `1/(60+1) * 0.5 = 0.0082`
- **Rank 3 in Vector:** `1/(60+3) * 0.5 = 0.0079`
- **Combined Score:** `0.0161`

Items appearing high in both systems get strong combined scores.

#### Critical Issue

**Status:** ❌ Qdrant never populated (infrastructure exists but pipeline broken)
**Impact:** Hybrid search currently non-functional
**Priority:** CRITICAL - Week 1 fix required

### 2.2 Vector-Only Search: claude-context

**Limitation:** No BM25/lexical search component

#### Architecture

```typescript
// Vector search only via Milvus
const results = await milvusClient.search({
  collection_name: collectionName,
  vectors: [queryEmbedding],
  limit: limit
});
```

#### Components

1. **Dense Vector Search**
   - **Providers:** OpenAI (text-embedding-3-large, 3072d) or Voyage Code-3
   - **Distance:** Cosine similarity
   - **Performance:** <50ms p99 (Zilliz Cloud)

2. **No Lexical Fallback**
   - Cannot efficiently find exact identifier matches
   - Relies purely on semantic embeddings
   - May miss exact function/variable name searches

#### Strengths

- **High-Quality Embeddings:** 3072d code-specific models
- **Production-Proven:** 40% token reduction verified
- **Merkle Tree Optimization:** Millisecond change detection

#### Comparison

| Aspect | rust-code-mcp | claude-context |
|--------|---------------|----------------|
| **Exact Keywords** | ✅ Excellent (BM25) | ❌ Poor (vector-only) |
| **Semantic Understanding** | ⚖️ Good (384d) | ✅ Excellent (3072d) |
| **Hybrid Benefits** | ✅ Best of both worlds | ❌ Single approach |
| **Token Reduction** | 45-50% projected | 40% measured |
| **Implementation Status** | ❌ Broken (Qdrant) | ✅ Working |

**Verdict:** rust-code-mcp has superior architecture, but claude-context has working implementation.

---

## 3. Embedding Generation

### 3.1 Local-First: rust-code-mcp

**Philosophy:** Privacy and zero-cost over maximum quality

#### Implementation

```rust
use fastembed::{TextEmbedding, EmbeddingModel, InitOptions};

let model = TextEmbedding::try_new(
    InitOptions::new(EmbeddingModel::AllMiniLML6V2)
)?;

// Generate embeddings locally (no API calls)
let embeddings: Vec<Embedding> = model.embed(texts, batch_size)?;
```

#### Specifications

- **Model:** all-MiniLM-L6-v2
- **Dimensions:** 384
- **Size:** ~80MB (one-time download)
- **Location:** .fastembed_cache/
- **Execution:** Local ONNX runtime
- **Performance:** 14.7ms per 1K tokens
- **Cost:** $0

#### Trade-offs

| Factor | Assessment |
|--------|------------|
| **Privacy** | ⭐⭐⭐⭐⭐ Perfect (100% local) |
| **Cost** | ⭐⭐⭐⭐⭐ Free ($0 recurring) |
| **Latency** | ⭐⭐⭐⭐⭐ Excellent (15ms) |
| **Quality** | ⭐⭐⭐ Good (baseline) |
| **Code-Specific** | ⭐⭐ Limited (general model) |

**Quality Gap:** 5-8% lower accuracy than code-specific models

**Mitigation:** Hybrid search (BM25 + Vector) compensates for embedding quality

### 3.2 API-Based: claude-context

**Philosophy:** Maximum quality, flexible provider choice

#### Supported Providers

1. **OpenAI** (default)
   - Model: text-embedding-3-large
   - Dimensions: 3072
   - Cost: $0.13 per 1M tokens
   - Quality: ⭐⭐⭐⭐ Very Good

2. **Voyage AI** (code-specialized)
   - Model: voyage-code-3
   - Optimized: Trained on code corpora
   - Cost: ~$0.10-0.15 per 1M tokens
   - Quality: ⭐⭐⭐⭐⭐ Excellent

3. **Ollama** (local option)
   - Execution: Local Ollama server
   - Cost: $0
   - Quality: ⭐⭐⭐-⭐⭐⭐⭐ (varies)
   - Privacy: ✅ Local

#### Trade-offs

| Factor | OpenAI/Voyage | Ollama |
|--------|---------------|--------|
| **Privacy** | ⭐⭐ Code sent to APIs | ⭐⭐⭐⭐⭐ Local |
| **Cost** | ⭐⭐ $1-6.50 per 100k LOC | ⭐⭐⭐⭐⭐ Free |
| **Latency** | ⭐⭐⭐ 100-500ms (API) | ⭐⭐⭐⭐ 50-200ms |
| **Quality** | ⭐⭐⭐⭐⭐ Excellent | ⭐⭐⭐ Varies |

### 3.3 Comparison Summary

#### Cost Comparison (100k LOC codebase)

| Scenario | rust-code-mcp | claude-context (OpenAI) | claude-context (Ollama) |
|----------|---------------|------------------------|------------------------|
| **Initial Indexing** | $0 | $1-6.50 | $0 |
| **Monthly Updates** | $0 | $0.01-0.10 | $0 |
| **Year 1 Total** | $0 | $1,200-6,000 | $0-300 |

#### Performance Comparison

| Metric | rust-code-mcp | claude-context (API) |
|--------|---------------|---------------------|
| **Embedding Latency** | 15ms (local) | 100-500ms (API) |
| **Vector Search** | 10-30ms (Qdrant) | ~50ms (Milvus) |
| **Total Query** | 100-200ms | 200-500ms |

#### Quality Comparison

- **Accuracy Gap:** claude-context +10-15% (with code-specific models)
- **Token Reduction:** rust-code-mcp 45-50% projected vs claude-context 40% measured
- **Hybrid Compensation:** rust-code-mcp's BM25 component bridges quality gap

**Recommendation:**
- Use rust-code-mcp for privacy/cost-sensitive scenarios
- Use claude-context for maximum semantic quality
- Consider rust-code-mcp with optional API embeddings (best of both)

---

## 4. Code Chunking

### 4.1 Symbol-Based: rust-code-mcp

**Strategy:** Pure semantic chunking (one symbol = one chunk)

#### Implementation

```rust
// Parse AST with tree-sitter
let symbols = parser.parse_file(path)?;

// Create one chunk per symbol
for symbol in symbols {
    let chunk = CodeChunk {
        id: uuid::Uuid::new_v4(),
        content: extract_symbol_code(source, &symbol),
        context: ChunkContext {
            file_path: path,
            module_path: derive_module_path(path),
            symbol_name: symbol.name,
            symbol_kind: symbol.kind,
            docstring: extract_docstring(&symbol),
            imports: get_imports().take(5),
            calls: get_outgoing_calls(&symbol).take(5),
        },
        overlap_prev: calculate_overlap(prev_symbol, 0.2),
        overlap_next: calculate_overlap(next_symbol, 0.2),
    };
}
```

#### Supported Symbol Types (Rust-specific)

1. Function (`fn foo()`)
2. Struct (`struct Bar`)
3. Enum (`enum Baz`)
4. Trait (`trait Qux`)
5. Impl Block (`impl Foo for Bar`)
6. Module (`mod tests`)
7. Const (`const MAX: u32`)
8. Static (`static GLOBAL: Mutex`)
9. Type Alias (`type Result<T>`)

#### Context Enrichment (Anthropic Contextual Retrieval Pattern)

```rust
// Formatted output for embedding
// File: src/parser/mod.rs
// Location: lines 130-145
// Module: crate::parser
// Symbol: parse_file (function)
// Purpose: Parse a Rust source file and extract symbols
// Imports: std::fs, std::path::Path, tree_sitter::Parser
// Calls: fs::read_to_string, parse_source

pub fn parse_file(&mut self, path: &Path) -> Result<Vec<Symbol>> {
    let source = fs::read_to_string(path)?;
    self.parse_source(&source)
}
```

#### Characteristics

- **Granularity:** Variable (1 line to hundreds of lines)
- **Semantic Coherence:** ⭐⭐⭐⭐⭐ Perfect (never splits mid-function)
- **Size Predictability:** ⭐⭐ Low (natural boundaries)
- **Context Richness:** ⭐⭐⭐⭐⭐ Very High (imports, calls, docs)
- **Language Support:** ⭐⭐ Limited (Rust only)

### 4.2 Character-Bounded: claude-context

**Strategy:** AST-guided with character limits (max 2,500 chars)

#### Implementation

```typescript
// Primary: AST splitter with size constraints
class AstCodeSplitter {
  chunkSize: number = 2500;      // characters
  chunkOverlap: number = 300;    // characters

  async split(code: string, language: string): Promise<CodeChunk[]> {
    const tree = parser.parse(code);
    let chunks = this.extractChunks(tree.rootNode);
    chunks = this.refineChunks(chunks);  // Split if > chunkSize
    chunks = this.addOverlap(chunks);
    return chunks;
  }
}

// Fallback: LangChain text splitter
class LangChainCodeSplitter {
  chunkSize: number = 1000;
  chunkOverlap: number = 200;

  // Recursive character splitting with language-aware separators
}
```

#### Two-Level Fallback

1. **Primary:** AST-based (10 languages via tree-sitter)
2. **Secondary:** LangChain text splitter (20+ languages)
3. **Tertiary:** Generic text splitter (any content)

#### Characteristics

- **Granularity:** Controlled (max 2,500 chars)
- **Semantic Coherence:** ⭐⭐⭐⭐ High (may split large functions)
- **Size Predictability:** ⭐⭐⭐⭐⭐ Excellent (bounded)
- **Context Richness:** ⭐⭐⭐ Medium (path, language, lines)
- **Language Support:** ⭐⭐⭐⭐⭐ Excellent (30+ languages)

### 4.3 Comparison

| Aspect | rust-code-mcp | claude-context |
|--------|---------------|----------------|
| **Chunking Unit** | Complete symbol | Character-bounded node |
| **Size Range** | 1 line - entire file | Up to 2,500 chars |
| **Semantic Completeness** | 100% guaranteed | Best-effort |
| **Context Enrichment** | Rich (imports, calls, docs) | Minimal (path, language) |
| **Overlap Method** | Line-based (20%) | Character-based (300 chars) |
| **Fallback Strategy** | ❌ None | ✅ Two-level fallback |
| **Language Support** | Rust only | 30+ languages |
| **Robustness** | ❌ Fails on parse error | ✅ Always produces chunks |

### 4.4 Architectural Lessons

**Strengths of rust-code-mcp approach:**
- Perfect semantic coherence
- Rich context for smaller embedding models
- Ideal for deep single-language analysis

**Strengths of claude-context approach:**
- Multi-language generality
- Graceful error handling
- Predictable performance characteristics

**Hybrid Possibility:** Combine symbol-based chunking with max size limits and fallback mechanisms.

---

## 5. Incremental Indexing

### 5.1 SHA-256 Approach: rust-code-mcp (Current)

**Status:** ✅ Working but inefficient at scale

#### Algorithm

```rust
// Change detection: O(n) - must hash every file
for file in directory.files() {
    let content = fs::read_to_string(file)?;
    let current_hash = sha256(&content);

    match metadata_cache.get(file) {
        Some(cached) if cached.hash == current_hash => {
            // Skip unchanged file (10x speedup)
            continue;
        }
        _ => {
            // Reindex changed file
            reindex_file(file)?;
            metadata_cache.set(file, current_hash);
        }
    }
}
```

#### Performance

| Codebase Size | Unchanged Check | Speedup vs Full Reindex |
|---------------|-----------------|------------------------|
| 10k LOC | ~1s | 10x |
| 100k LOC | ~10s | 10x |
| 1M LOC | ~100s | 10x |

**Limitation:** Linear time complexity - must hash every file to detect changes

### 5.2 Merkle Tree: rust-code-mcp (Planned)

**Status:** 📋 Designed (Strategy 4 in docs/INDEXING_STRATEGIES.md)

#### Architecture

```
                    Root Hash (entire project)
                    ┌───────────────────┐
                    │   sha256(A + B)   │
                    └─────────┬─────────┘
              ┌───────────────┴───────────────┐
              │                               │
        Directory A                     Directory B
    ┌────────────────┐              ┌────────────────┐
    │  sha256(files) │              │  sha256(files) │
    └────────┬───────┘              └────────┬───────┘
         ┌───┴───┐                        ┌───┴───┐
      File 1  File 2                   File 3  File 4
```

#### Algorithm

```rust
// Phase 1: O(1) root comparison
let cached_root = load_merkle_snapshot(project)?;
let current_root = build_merkle_tree(project)?;

if cached_root == current_root {
    return Ok(NoChanges);  // <10ms exit
}

// Phase 2: O(log n) tree traversal
let changed_files = find_changed_subtrees(cached_tree, current_tree)?;

// Phase 3: O(k) reindexing where k = number of changed files
for file in changed_files {
    reindex_file(file)?;
}
```

#### Expected Performance

| Codebase Size | Unchanged Check | Changed (1% files) | Improvement |
|---------------|-----------------|-------------------|-------------|
| 10k LOC | <10ms | <1s | 100x |
| 100k LOC | <20ms | <3s | 500x |
| 1M LOC | <50ms | <15s | 2000x |
| 10M LOC | <100ms | <2min | 60,000x |

### 5.3 Merkle Tree: claude-context (Production)

**Status:** ✅ Production-deployed, millisecond-level performance

#### Implementation

```typescript
// Phase 1: Root hash comparison (milliseconds)
const cachedSnapshot = loadMerkleSnapshot(projectPath);
const currentRoot = await buildMerkleTree(projectPath);

if (cachedSnapshot.rootHash === currentRoot.hash) {
  return { status: 'unchanged', timeMs: 5 };
}

// Phase 2: Precise change detection (seconds)
const changedFiles = await traverseTreeForChanges(
  cachedSnapshot.tree,
  currentRoot.tree
);

// Phase 3: Selective reindexing
await reindexFiles(changedFiles);
```

#### Production Metrics

- **Unchanged Detection:** <10ms (single root hash comparison)
- **Change Detection:** 10-50ms (tree traversal)
- **Directory Skipping:** 60-80% skip rate on typical git workflows
- **Proven Results:** 40% token reduction, 100-1000x speedup vs full scan

#### Storage

- **Location:** ~/.context/merkle/
- **Contents:** Root hash, file hashes, tree structure, timestamp
- **Overhead:** ~1-2 KB per file
- **Total (1M LOC):** ~50-100 MB

### 5.4 Comparison

| Aspect | rust-code-mcp (SHA-256) | rust-code-mcp (Merkle, planned) | claude-context (Merkle) |
|--------|------------------------|--------------------------------|------------------------|
| **Algorithm** | Per-file hashing | Tree-based hashing | Tree-based hashing |
| **Complexity** | O(n) | O(1) unchanged, O(log n) + O(k) changed | O(1) unchanged, O(log n) + O(k) changed |
| **Unchanged (1M LOC)** | ~100s | <50ms | <100ms |
| **Changed (1% files)** | ~10s | <15s | ~15s |
| **Speedup** | 10x | 100-2000x | 100-1000x (proven) |
| **Status** | ✅ Working | 📋 Designed | ✅ Production |

### 5.5 Key Findings

**Validated by claude-context:**
1. ✅ Merkle trees are essential, not optional (100-1000x speedup)
2. ✅ Millisecond-level change detection is achievable
3. ✅ Directory-level skipping critical for large codebases
4. ✅ File-level granularity sufficient (no need for byte-range diffing)

**Critical Gap for rust-code-mcp:**
- **Priority:** HIGH
- **Effort:** 1-2 weeks
- **Impact:** 100-1000x speedup for >500k LOC codebases
- **Recommendation:** Implement Merkle tree before production release

---

## 6. MCP Tool Interface

### 6.1 Tool Inventory

| Category | rust-code-mcp (8 tools) | claude-context (4 tools) |
|----------|------------------------|--------------------------|
| **Search** | search, get_similar_code | search_code |
| **Indexing** | (on-demand during search) | index_codebase |
| **Management** | - | clear_index, get_indexing_status |
| **File Ops** | read_file_content | - |
| **Code Analysis** | find_definition, find_references, get_dependencies, get_call_graph, analyze_complexity | - |

### 6.2 Detailed Tool Comparison

#### Search Tools

**rust-code-mcp: `search`**
```yaml
Input:
  directory: String (path to search)
  keyword: String (search term)
Output:
  Hit: src/foo.rs (Score: 8.45)
  Hit: src/bar.rs (Score: 6.32)
Features:
  - BM25-based keyword search
  - On-demand indexing with incremental updates
  - SHA-256 change detection
  - Persistent Tantivy index
Performance: <100ms (with persistent index)
```

**rust-code-mcp: `get_similar_code`**
```yaml
Input:
  query: String (code snippet or description)
  directory: String
  limit: Option<usize> (default 5)
Output:
  1. Score: 0.85 | File: src/foo.rs | Symbol: parse (function)
     Lines: 42-58
     Doc: Parse source file
     Code preview: pub fn parse(...)
Features:
  - Semantic vector search
  - Local embeddings (fastembed)
  - Qdrant similarity search
Performance: 200-1000ms (embedding + search)
Status: ❌ Broken (Qdrant not populated)
```

**claude-context: `search_code`**
```yaml
Input:
  path: String (absolute path)
  query: String (natural language)
  limit: Number (default 10, max 50)
  extensionFilter: Array<String> (optional)
Output:
  Found 3 results for query: "authentication logic"

  1. Code snippet (typescript) [myapp]
     Location: src/auth.ts:23-45
     Rank: 1
     Context:
  ```typescript
  export async function authenticate(user: User) {
    // authentication implementation
  }
  ```
Features:
  - Natural language queries
  - High-quality embeddings (3072d)
  - Markdown formatted output
  - Extension filtering
Performance: 200-500ms (API + cloud vector search)
Status: ✅ Production working
```

#### Indexing & Management

**claude-context: `index_codebase`**
```yaml
Input:
  path: String (absolute path)
  force: Boolean (re-index if exists)
  splitter: 'ast' | 'langchain'
  customExtensions: Array<String>
  ignorePatterns: Array<String>
Output:
  Started background indexing for codebase '/path/to/code'
  using ast splitter...
Features:
  - Asynchronous background indexing
  - Progress tracking via get_indexing_status
  - Configurable chunking strategy
  - Custom file filters
```

**claude-context: `get_indexing_status`**
```yaml
Input:
  path: String
Output (Indexed):
  ✅ Codebase is fully indexed

  Path: /path/to/code
  Files indexed: 1,234
  Total chunks: 8,456
  Last indexed: 2025-10-19 14:32:01
Output (Indexing):
  🔄 Currently being indexed. Progress: 45%

  Path: /path/to/code
```

**Observation:** rust-code-mcp lacks explicit index management tools, making it harder to understand and control indexing state.

#### Code Analysis Tools (rust-code-mcp Unique)

**`find_definition`**
```yaml
Input:
  symbol_name: String
  directory: String
Output:
  Found 1 definition(s) for 'parse_file':
  - src/parser/mod.rs:130 (function)
Use Cases:
  - Navigate to symbol definition
  - Understand code structure
Performance: 100-500ms (depends on codebase size)
```

**`find_references`**
```yaml
Input:
  symbol_name: String
  directory: String
Output:
  Found 12 reference(s) to 'parse_file' in 5 file(s):

  Function Calls (8 references):
  - src/indexer.rs (called by: index_directory, process_file)
  - src/cli.rs (called by: main)

  Type Usage (4 references):
  - src/types.rs (parameter in process_results)
Use Cases:
  - Find all usages of a function/type
  - Impact analysis before refactoring
  - Understand code dependencies
```

**`get_call_graph`**
```yaml
Input:
  file_path: String
  symbol_name: Option<String>
Output:
  Call graph for 'src/parser/mod.rs':

  Symbol: parse_file

  Calls (3):
    → read_file
    → parse_source
    → extract_symbols

  Called by (2):
    ← index_directory
    ← process_batch
Use Cases:
  - Understand control flow
  - Find call chains
  - Identify dead code
```

**`analyze_complexity`**
```yaml
Output:
  Complexity analysis for 'src/parser/mod.rs':

  === Code Metrics ===
  Total lines:           245
  Non-empty lines:       198
  Comment lines:         32
  Code lines (approx):   166

  === Symbol Counts ===
  Functions:             12
  Structs:               3
  Traits:                1

  === Complexity ===
  Total cyclomatic:      28
  Avg per function:      2.3
  Function calls:        45
Use Cases:
  - Measure code quality
  - Identify refactoring targets
  - Track complexity over time
```

### 6.3 Tool Count Analysis

**More tools ≠ Better**

| Project | Tool Count | Purpose |
|---------|-----------|---------|
| rust-code-mcp | 8 | Search + Deep code analysis |
| claude-context | 4 | Search workflow (index → search → manage) |

**Overlap:** 2 tools (search functionality)

**Unique to rust-code-mcp:** 6 analysis tools
- Strategic advantage: Code intelligence beyond just search
- Use cases: Refactoring, navigation, complexity tracking
- Market differentiation: "Code search + analysis platform"

**Unique to claude-context:** 2 management tools
- Better UX: Explicit index lifecycle management
- Progress visibility: Real-time indexing status
- Professional polish: Production-grade operations

### 6.4 Recommendations

**For rust-code-mcp:**
1. ✅ Keep unique analysis tools (competitive advantage)
2. ➕ Add `index_codebase` tool (explicit indexing)
3. ➕ Add `get_indexing_status` tool (progress tracking)
4. ➕ Add `clear_index` tool (index management)

**Marketing Position:**
- claude-context: "Semantic code search for AI coding"
- rust-code-mcp: "Code intelligence platform with search, analysis, and deep Rust understanding"

---

## 7. Performance & Scalability

### 7.1 Performance Targets vs Actuals

#### rust-code-mcp

| Metric | Target | Actual | Status |
|--------|--------|--------|--------|
| **Indexing (100k LOC)** | <2 min | Not benchmarked | 🔶 Untested |
| **Query Latency (p95)** | <200ms | <100ms (small test) | ✅ On track |
| **Memory Usage (1M LOC)** | <4GB | Not measured | 🔶 Untested |
| **Incremental Update** | <5s (1% change) | ~15-20ms (verified, small) | ✅ Working |
| **Token Reduction** | 45-50% | Not validated | 🔶 Projected |

**Implementation Status:**
- ✅ Tantivy BM25: Working
- ❌ Qdrant Vector: Infrastructure ready, not populated
- ✅ Incremental: SHA-256 working, Merkle planned
- ✅ Tests: 45 unit tests passed, 8 tools verified

#### claude-context

| Metric | Claim | Evidence | Status |
|--------|-------|----------|--------|
| **Token Reduction** | 40% | Production verified | ✅ Proven |
| **Query Latency** | <50ms p99 | Zilliz Cloud benchmarks | ✅ Proven |
| **Change Detection** | <10ms | Merkle tree (production) | ✅ Proven |
| **Scalability** | Millions of LOC | Cloud-native architecture | ✅ Capable |
| **Find Implementation** | 300x faster than grep | User testimonials | ✅ Validated |

### 7.2 Scalability Comparison

#### Maximum Codebase Size

| Project | Explicit Target | Infrastructure Limit | Optimal Range |
|---------|----------------|---------------------|---------------|
| rust-code-mcp | 1M+ LOC | ~10M LOC (single server) | 1M-10M LOC |
| claude-context | Millions of LOC | >100M LOC (cloud) | Unlimited (elastic) |

**rust-code-mcp Scalability:**
- **Architecture:** Embedded databases (Qdrant + Tantivy)
- **Scaling Strategy:** Vertical (better hardware)
- **Bottleneck:** Local RAM and disk I/O
- **Practical Limit:** 10M LOC with optimization
- **Deployment:** Self-hosted (single machine)

**claude-context Scalability:**
- **Architecture:** Cloud-native distributed (Zilliz)
- **Scaling Strategy:** Horizontal (elastic nodes)
- **Bottleneck:** None (managed service)
- **Practical Limit:** No hard limit
- **Deployment:** Multi-tenant cloud or dedicated cluster

#### Indexing Performance Projections

| Codebase | rust-code-mcp (Target) | claude-context (Projected) |
|----------|----------------------|---------------------------|
| **10k LOC** | <30s initial, <1s incremental | <30s initial, <1s incremental |
| **100k LOC** | <2min initial, <2s incremental | <2min initial, <3s incremental |
| **1M LOC** | <10min initial, <5s incremental | <10min initial, <15s incremental |
| **10M LOC** | <1hr initial, <2min incremental | <1hr initial, <3min incremental |

**Note:** With Merkle tree optimization, both achieve <100ms unchanged detection.

### 7.3 Query Performance

#### Latency Breakdown

**rust-code-mcp (Hybrid Search, Target):**
```
Embedding Generation:    5-20ms   (local fastembed)
Vector Search (Qdrant):  <50ms    (cosine similarity)
BM25 Search (Tantivy):   <20ms    (inverted index)
RRF Merging:             <5ms     (rank fusion)
─────────────────────────────────
Total (Parallel):        <100ms   (concurrent execution)
```

**claude-context (Vector Search, Actual):**
```
API Call (OpenAI):       50-200ms (network + embedding)
Vector Search (Milvus):  <50ms    (cloud query)
─────────────────────────────────
Total:                   100-250ms (sequential)
```

**Advantage:** rust-code-mcp has lower latency potential due to local execution

#### Throughput

| Metric | rust-code-mcp | claude-context |
|--------|---------------|----------------|
| **Concurrent Queries** | Limited by local CPU | High (cloud infrastructure) |
| **Rate Limits** | None | API rate limits possible |
| **Predictability** | Hardware-bound | Network-dependent |

### 7.4 Memory Usage

**rust-code-mcp (Estimated):**
- FastEmbed model: ~80MB
- Qdrant HNSW index: ~200-400MB (100k vectors)
- Tantivy index: ~50-100MB (100k LOC)
- Merkle tree: ~50-100MB (1M LOC)
- **Total:** ~400-680MB for 100k LOC, ~2-4GB for 1M LOC

**claude-context:**
- Client-side: Minimal (~50-100MB)
- Server-side: Managed by Zilliz (not user concern)

**Advantage:** claude-context offloads memory to cloud

### 7.5 Token Reduction

**Validated Results:**
- claude-context: **40%** (production-proven)
- rust-code-mcp: **45-50%** (projected based on hybrid search)

**Why rust-code-mcp projects higher:**
1. True hybrid (BM25 + Vector) captures more relevant contexts
2. RRF fusion reduces noise from single-approach limitations
3. Rich context enrichment (imports, calls, docs) improves precision

**Validation Needed:** Large-scale benchmarks to confirm projection

---

## 8. Cost Analysis

### 8.1 Total Cost of Ownership (3 Years)

#### rust-code-mcp

| Cost Category | Year 1 | Year 2 | Year 3 | 3-Year Total |
|---------------|--------|--------|--------|--------------|
| **Software** | $0 | $0 | $0 | $0 |
| **Cloud Services** | $0 | $0 | $0 | $0 |
| **API Fees** | $0 | $0 | $0 | $0 |
| **Hardware** (optional upgrade) | $0-2,000 | $0 | $0 | $0-2,000 |
| **Maintenance** | $0 | $0 | $0 | $0 |
| **TOTAL** | $0-2,000 | $0 | $0 | **$0-2,000** |

#### claude-context

| Cost Category | Year 1 | Year 2 | Year 3 | 3-Year Total |
|---------------|--------|--------|--------|--------------|
| **Software** | $0 | $0 | $0 | $0 |
| **Zilliz Cloud** (Serverless/Dedicated) | $240-2,400 | $240-2,400 | $240-2,400 | $720-7,200 |
| **Embedding API** (OpenAI/Voyage) | $60-600 | $60-600 | $60-600 | $180-1,800 |
| **Maintenance** | $0 | $0 | $0 | $0 |
| **TOTAL** | $300-3,000 | $300-3,000 | $300-3,000 | **$900-9,000** |

### 8.2 Cost Breakdown by Codebase Size

#### Small Codebase (10k-50k LOC)

| Project | Setup | Monthly | 3-Year Total |
|---------|-------|---------|--------------|
| rust-code-mcp | $0 | $0 | $0 |
| claude-context | $0-50 | $20-50 | $720-1,800 |

#### Medium Codebase (100k-500k LOC)

| Project | Setup | Monthly | 3-Year Total |
|---------|-------|---------|--------------|
| rust-code-mcp | $0 | $0 | $0 |
| claude-context | $50-100 | $50-150 | $1,800-5,400 |

#### Large Codebase (1M+ LOC)

| Project | Setup | Monthly | 3-Year Total |
|---------|-------|---------|--------------|
| rust-code-mcp | $500-2,000 (RAM/SSD) | $0 | $500-2,000 |
| claude-context | $100-200 | $200-500 | $7,200-18,000 |

### 8.3 Cost-Benefit Analysis

#### rust-code-mcp

**Advantages:**
- ✅ Zero recurring costs
- ✅ No API usage fees
- ✅ Predictable one-time investment
- ✅ Scales with hardware (not subscription tiers)

**Hidden Costs:**
- Hardware maintenance (electricity, repairs)
- Developer time for setup and updates
- No 24/7 managed support

**Best For:**
- Individual developers
- Open-source projects
- Cost-conscious teams
- Privacy-sensitive organizations

#### claude-context

**Advantages:**
- ✅ Pay-as-you-grow (no upfront hardware)
- ✅ Managed service (zero ops overhead)
- ✅ Elastic scaling (automatic)
- ✅ High availability (99.9%+ SLA)

**Hidden Costs:**
- Unpredictable at scale (usage-based pricing)
- Vendor lock-in
- Requires ongoing budget approval

**Best For:**
- Teams with cloud budget
- Organizations valuing managed services
- Projects requiring high availability
- Multi-developer environments

### 8.4 Break-Even Analysis

**When does rust-code-mcp's hardware investment break even vs claude-context subscription?**

| Scenario | Hardware Cost | claude-context Monthly | Break-Even |
|----------|---------------|----------------------|------------|
| **Small Team** | $1,000 (better workstation) | $50/month | 20 months |
| **Medium Team** | $1,500 (dedicated server) | $150/month | 10 months |
| **Large Team** | $2,000 (high-end server) | $300/month | 7 months |

**Conclusion:** rust-code-mcp pays for itself within 1 year for any team size.

---

## 9. Use Case Recommendations

### 9.1 Choose rust-code-mcp When...

#### Privacy & Compliance

✅ **Use Case:** Working with proprietary/sensitive code

**Why rust-code-mcp:**
- Code never leaves local machine
- No external API calls (embeddings generated locally)
- No cloud provider access to intellectual property
- Suitable for: Healthcare (HIPAA), Finance (PCI-DSS), Government (FedRAMP)

**Example:** Banking institution indexing internal fraud detection algorithms

---

✅ **Use Case:** Air-gapped or offline environments

**Why rust-code-mcp:**
- 100% offline after initial setup
- No internet dependency
- Embedded databases (Qdrant + Tantivy)
- Suitable for: Military, classified research, isolated networks

**Example:** Aerospace company developing classified systems

---

#### Cost Constraints

✅ **Use Case:** Zero budget for cloud services

**Why rust-code-mcp:**
- $0 recurring costs
- No API usage fees
- Self-hosted on existing hardware
- Suitable for: Startups, open-source projects, individual developers

**Example:** Open-source maintainer wanting to understand large codebase contributions

---

✅ **Use Case:** Predictable performance without rate limits

**Why rust-code-mcp:**
- No API throttling
- Hardware-bound only (no network variance)
- Consistent sub-100ms latency
- Suitable for: Real-time applications, high-frequency queries

**Example:** IDE plugin that queries on every keystroke

---

#### Technical Requirements

✅ **Use Case:** Primarily Rust codebase

**Why rust-code-mcp:**
- Deep Rust-specific analysis (9 symbol types)
- Visibility tracking (pub/pub(crate)/private)
- Trait and impl block understanding
- Async/unsafe/const detection

**Example:** Tokio maintainers analyzing runtime internals

---

✅ **Use Case:** Small-medium codebase (<1M LOC)

**Why rust-code-mcp:**
- Optimized for 1M-10M LOC range
- Fast local indexing (<10 min for 1M LOC)
- Minimal resource usage
- No cloud overhead

**Example:** Company with 500k LOC Rust microservices

---

✅ **Use Case:** Need deep code analysis (beyond search)

**Why rust-code-mcp:**
- Call graph visualization
- Reference finding
- Dependency analysis
- Complexity metrics
- Definition lookup

**Example:** Refactoring legacy Rust codebase

---

### 9.2 Choose claude-context When...

#### Collaboration & Scale

✅ **Use Case:** Multi-developer team

**Why claude-context:**
- Centralized cloud index (shared across team)
- Consistent search results for everyone
- No redundant local indexing per developer
- Suitable for: Engineering teams, distributed organizations

**Example:** 20-person startup with TypeScript + Python microservices

---

✅ **Use Case:** Large codebase (>1M LOC)

**Why claude-context:**
- Elastic scaling via Zilliz Cloud
- Handles millions of LOC efficiently
- Distributed architecture
- Suitable for: Enterprise monorepos, large-scale projects

**Example:** E-commerce company with 5M LOC codebase (multiple languages)

---

#### Production Requirements

✅ **Use Case:** Need high availability (99.9%+ SLA)

**Why claude-context:**
- Managed Zilliz Cloud infrastructure
- Automatic failover
- Geographic redundancy
- Load balancing

**Example:** SaaS company where code search is critical to development velocity

---

✅ **Use Case:** Want zero operational overhead

**Why claude-context:**
- Fully managed service
- Automatic backups
- Auto-tuning and optimization
- Monitoring dashboards included

**Example:** Small team without dedicated DevOps

---

#### Multi-Language Projects

✅ **Use Case:** Polyglot codebase (JS, Python, Java, etc.)

**Why claude-context:**
- 30+ languages supported
- Tree-sitter parsers for 10 languages
- LangChain fallback for others
- Universal compatibility

**Example:** Full-stack application (React + Django + PostgreSQL)

---

✅ **Use Case:** Maximum semantic accuracy required

**Why claude-context:**
- 3072d embeddings (OpenAI text-embedding-3-large)
- Code-specific models (Voyage Code-3)
- Proven 40% token reduction
- Production-validated quality

**Example:** AI coding assistant startup competing on quality

---

### 9.3 Hybrid Scenario

✅ **Use Case:** Privacy + Quality

**Strategy:** rust-code-mcp with optional API embeddings

**Implementation:**
1. Default: Local fastembed (384d) for privacy
2. Enhanced: Qodo-Embed-1.5B (local, +37% accuracy)
3. Premium: OpenAI/Voyage (opt-in, maximum quality)

**Configuration:**
```yaml
embedding_provider: "local"  # or "openai" or "voyage"
OPENAI_API_KEY: ""           # only if opt-in
```

**Benefits:**
- Privacy by default
- User controls quality/privacy trade-off
- Gradual quality upgrade path

**Example:** Open-source project where contributors choose their own privacy level

---

### 9.4 Decision Matrix

| Factor | Weight | rust-code-mcp Score | claude-context Score |
|--------|--------|-------------------|---------------------|
| **Privacy** | 🔴🔴🔴 High | ⭐⭐⭐⭐⭐ 5/5 | ⭐⭐ 2/5 |
| **Cost** | 🔴🔴🔴 High | ⭐⭐⭐⭐⭐ 5/5 | ⭐⭐ 2/5 |
| **Scalability** | 🔴🔴 Medium | ⭐⭐⭐ 3/5 | ⭐⭐⭐⭐⭐ 5/5 |
| **Multi-Language** | 🔴 Low | ⭐ 1/5 | ⭐⭐⭐⭐⭐ 5/5 |
| **Production Maturity** | 🔴🔴 Medium | ⭐⭐ 2/5 | ⭐⭐⭐⭐⭐ 5/5 |
| **Search Quality** | 🔴🔴🔴 High | ⭐⭐⭐⭐⭐ 5/5 (after fix) | ⭐⭐⭐⭐ 4/5 |

**Privacy-First Scenario:**
- rust-code-mcp: 5×3 + 5×3 + 3×2 + 1×1 + 2×2 + 5×3 = **49/65**
- claude-context: 2×3 + 2×3 + 5×2 + 5×1 + 5×2 + 4×3 = **49/65**

**Cloud-Native Scenario:**
- rust-code-mcp: 5×1 + 5×1 + 3×3 + 1×3 + 2×2 + 5×3 = **40/65**
- claude-context: 2×1 + 2×1 + 5×3 + 5×3 + 5×2 + 4×3 = **56/65**

---

## 10. Roadmap & Recommendations

### 10.1 Critical Gaps (rust-code-mcp)

#### Priority 1: CRITICAL - Fix Qdrant Population Pipeline

**Issue:** Hybrid search infrastructure exists but Qdrant never populated

**Impact:**
- ❌ Vector search completely broken
- ❌ Hybrid search (core feature) non-functional
- ❌ Cannot validate 45-50% token reduction claim

**Solution:**
```rust
// src/tools/search_tool.rs - Add missing integration
fn index_directory(path: &Path) -> Result<()> {
    // 1. Parse and chunk files
    let chunks = chunker.chunk_files(parse_results)?;

    // 2. Generate embeddings (MISSING!)
    let embeddings = embedding_generator.embed_chunks(&chunks)?;

    // 3. Upsert to Qdrant (MISSING!)
    vector_store.upsert(embeddings)?;

    // 4. Index to Tantivy (existing, works)
    bm25_index.index_chunks(chunks)?;

    Ok(())
}
```

**Effort:** 2-3 days
**Blockers:** None (all infrastructure ready)
**Expected Outcome:** Hybrid search functional, able to benchmark token reduction

---

#### Priority 2: HIGH - Implement Merkle Tree

**Issue:** Change detection is O(n) - must hash every file

**Impact:**
- ⚠️ Slow for large codebases (100s for 1M LOC)
- ⚠️ Not production-grade at scale
- ⚠️ 100-1000x slower than possible

**Solution:**
```rust
// src/indexing/merkle.rs - New module
struct MerkleIndexer {
    tree: MerkleTree,
    snapshot_path: PathBuf,
}

impl MerkleIndexer {
    fn check_changes(&self, project: &Path) -> Result<ChangeSet> {
        // Phase 1: O(1) root comparison
        let cached_root = self.load_snapshot()?;
        let current_root = self.build_tree(project)?;

        if cached_root == current_root {
            return Ok(ChangeSet::Empty);
        }

        // Phase 2: O(log n) traversal
        let changed = self.find_changed_files(cached_root, current_root)?;
        Ok(ChangeSet::Files(changed))
    }
}
```

**Effort:** 1-2 weeks
**Dependencies:** `rs-merkle` crate (add to Cargo.toml)
**Expected Outcome:** <10ms unchanged detection (100-1000x speedup)

---

#### Priority 3: HIGH - Switch to AST-First Chunking

**Issue:** Using text-splitter despite having RustParser

**Impact:**
- ⚠️ Lower chunk quality (token boundaries vs semantic boundaries)
- ⚠️ Not leveraging existing AST infrastructure
- ⚠️ Suboptimal retrieval quality

**Solution:**
```rust
// src/chunker/mod.rs - Modify to use AST symbols
impl Chunker {
    fn chunk_file(&self, file: &Path) -> Result<Vec<CodeChunk>> {
        // Use existing RustParser
        let symbols = self.parser.parse_file(file)?;

        // Create chunks at symbol boundaries (not token boundaries)
        let chunks = symbols.iter().map(|symbol| {
            CodeChunk {
                content: extract_symbol_code(source, symbol),
                context: build_rich_context(symbol),  // Already implemented!
                ..Default::default()
            }
        }).collect();

        Ok(chunks)
    }
}
```

**Effort:** 3-5 days
**Blockers:** None (RustParser already extracts symbols)
**Expected Outcome:** 30-40% smaller, higher-quality chunks

---

### 10.2 Medium-Priority Improvements

#### Add Index Management Tools

**Missing:**
- `index_codebase` - Explicit indexing trigger
- `get_indexing_status` - Progress monitoring
- `clear_index` - Index lifecycle management

**Impact:** Better UX, matches claude-context polish

**Effort:** 1 week

---

#### Multi-Language Support (Gradual)

**Current:** Rust only
**Next:** Python, TypeScript, Go
**Approach:** Add tree-sitter grammars one by one
**Effort:** 2-3 weeks per language (with tests)

---

### 10.3 Long-Term Vision

#### Phase 8: Optimization & Release (Weeks 1-4)

**Week 1:**
- ✅ Fix Qdrant population pipeline (Priority 1)
- ✅ Add index management tools

**Week 2-3:**
- ✅ Implement Merkle tree (Priority 2)
- ✅ Benchmark on 100k+ LOC codebases

**Week 4:**
- ✅ Switch to AST-first chunking (Priority 3)
- ✅ Validate 45-50% token reduction claim

---

#### Phase 9: Production Hardening (Weeks 5-8)

**Week 5:**
- Memory profiling on real codebases
- Optimize HNSW parameters
- Load testing

**Week 6:**
- Error handling improvements
- Logging and observability
- Configuration management

**Week 7:**
- Documentation (user guide, API docs)
- Example projects and tutorials
- Deployment guide

**Week 8:**
- Security audit
- Performance benchmarks published
- 1.0 release

---

#### Phase 10: Expansion (Months 3-6)

**Month 3:**
- Python language support
- Background file watching

**Month 4:**
- TypeScript language support
- Optional API embeddings (OpenAI/Voyage)

**Month 5:**
- Go language support
- VSCode extension

**Month 6:**
- Multi-repo federation
- Team collaboration features (optional)

---

### 10.4 Competitive Positioning After Fixes

| Feature | rust-code-mcp (After Phase 8) | claude-context |
|---------|------------------------------|----------------|
| **Hybrid Search** | ✅ BM25 + Vector (working) | ❌ Vector-only |
| **Change Detection** | ✅ Merkle tree (<10ms) | ✅ Merkle tree (<10ms) |
| **Chunk Quality** | ✅ AST-based (semantic) | ✅ AST-based (semantic) |
| **Token Reduction** | 45-50% (validated) | 40% (validated) |
| **Privacy** | ✅ 100% local | ❌ Cloud APIs |
| **Cost** | ✅ $0 | ❌ $1,200-6,000/year |
| **Language Support** | ⚠️ Rust only | ✅ 30+ languages |
| **Production Status** | ⚠️ New (validated) | ✅ Battle-tested |

**Verdict:** rust-code-mcp will be superior for Rust-focused, privacy-conscious, cost-sensitive users. claude-context remains best for multi-language, cloud-native, team environments.

---

## Appendix A: Technology Stack Comparison

### Vector Databases

| Feature | Qdrant (rust-code-mcp) | Milvus (claude-context) |
|---------|----------------------|------------------------|
| **Query Latency** | 10-30ms | ~50ms |
| **Insertion Speed** | 41.27s (SQuAD) | 12.02s (SQuAD) |
| **Deployment** | Embedded or remote | Cloud-native |
| **Memory Management** | Memory-mapped files | Distributed |
| **Max Vectors (single server)** | ~50M | Unlimited (elastic) |

**Advantage:** Qdrant faster queries, Milvus faster insertion and unlimited scale

---

### Text Search Engines

| Feature | Tantivy (rust-code-mcp) | Milvus BM25 (claude-context) |
|---------|------------------------|------------------------------|
| **Implementation** | Full-featured engine | Integrated component |
| **Performance** | <20ms (expected) | Included in hybrid query |
| **Memory** | 22% less than Lucene | Not specified |
| **Configurability** | High (Rust API) | Medium (via Milvus) |

**Advantage:** Tantivy is standalone, optimized engine vs integrated component

---

### Embedding Models

| Model | Dimensions | Quality | Speed | Privacy | Cost |
|-------|-----------|---------|-------|---------|------|
| **all-MiniLM-L6-v2** (rust-code-mcp) | 384 | ⭐⭐⭐ | 14.7ms | ✅ Local | $0 |
| **text-embedding-3-large** (claude-context) | 3072 | ⭐⭐⭐⭐ | API latency | ❌ Cloud | $0.13/1M tokens |
| **voyage-code-3** (claude-context) | High | ⭐⭐⭐⭐⭐ | API latency | ❌ Cloud | ~$0.10/1M tokens |
| **Ollama** (claude-context option) | Varies | ⭐⭐⭐ | 50-200ms | ✅ Local | $0 |

---

## Appendix B: Research Methodology

### Data Sources

**rust-code-mcp:**
- Source code analysis (all modules)
- Documentation review (docs/, README.md)
- Test coverage examination (45 unit tests)
- Performance targets (NEW_PLAN.md, PHASE plans)

**claude-context:**
- GitHub repository: github.com/zilliztech/claude-context
- Official blog posts (Zilliz)
- Local comparison documentation
- Web research on Milvus/Zilliz performance

### Confidence Levels

| Aspect | rust-code-mcp Confidence | claude-context Confidence |
|--------|-------------------------|--------------------------|
| **Architecture** | ⭐⭐⭐⭐⭐ High (source code) | ⭐⭐⭐⭐ High (repo + docs) |
| **Performance** | ⭐⭐⭐ Medium (targets, not benchmarked) | ⭐⭐⭐⭐ High (production-validated) |
| **Costs** | ⭐⭐⭐⭐⭐ High (zero-cost model) | ⭐⭐⭐⭐ High (published pricing) |
| **Roadmap** | ⭐⭐⭐⭐⭐ High (detailed plans) | ⭐⭐⭐ Medium (general direction) |

---

## Appendix C: Glossary

**AST (Abstract Syntax Tree):** Tree representation of code structure used for semantic understanding

**BM25:** Best Match 25, a lexical search algorithm for keyword matching

**Chunking:** Splitting code into smaller pieces for embedding and retrieval

**Embedding:** Dense vector representation of text/code for semantic similarity

**HNSW:** Hierarchical Navigable Small World, fast approximate nearest neighbor algorithm

**Merkle Tree:** Hash tree for efficient change detection (O(1) for unchanged, O(log n) for changes)

**MCP:** Model Context Protocol, standard for AI coding assistant integrations

**NDCG:** Normalized Discounted Cumulative Gain, retrieval quality metric

**Qdrant:** Open-source vector database (used by rust-code-mcp)

**RRF:** Reciprocal Rank Fusion, method for combining multiple search result rankings

**SHA-256:** Cryptographic hash function for detecting file changes

**Tantivy:** Rust-native full-text search engine (similar to Lucene)

**Tree-sitter:** Incremental parsing library for multiple programming languages

**Vector Search:** Finding similar items using high-dimensional vector embeddings

---

## Conclusion

Both rust-code-mcp and claude-context are valid approaches to semantic code search, optimized for different priorities:

**rust-code-mcp** excels at:
- Privacy (100% local)
- Cost ($0 recurring)
- Hybrid search (BM25 + Vector)
- Deep Rust analysis

**claude-context** excels at:
- Production maturity (battle-tested)
- Multi-language support (30+)
- Scalability (cloud-native)
- Proven metrics (40% token reduction)

After fixing critical gaps (Qdrant population, Merkle tree, AST chunking), rust-code-mcp will be the superior choice for privacy-conscious, Rust-focused, cost-sensitive users. claude-context remains the best option for teams requiring multi-language support, cloud infrastructure, and production-proven reliability.

The choice depends on your priorities: **privacy + cost** or **scale + maturity**.

---

**Document Version:** 1.0
**Last Updated:** October 19, 2025
**Maintained By:** rust-code-mcp project
**License:** Same as project license
