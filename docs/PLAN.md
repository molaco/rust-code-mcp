# Implementation Plan: Rust Codebase Search MCP

## Project Overview

**Goal**: Build a scalable, in-process MCP server for analyzing large Rust codebases (>1M LOC) using hybrid search (vector + BM25), tree-sitter parsing, and advanced retrieval techniques.

**Why This Project**: With Bloop archived (Jan 2025) and no existing open-source tool combining Rust performance + MCP integration + scalability + local-first design, there's a real gap in the ecosystem.

**Target Users**:
- Developers working with large Rust codebases
- AI coding agents (Claude, GPT-4, etc.) via MCP
- Teams needing privacy-focused, local code intelligence

## Success Metrics

### MVP (Week 10)
- [ ] Index 100k LOC in <2 minutes
- [ ] Query latency <500ms p95
- [ ] Memory usage <2GB for 100k LOC
- [ ] Basic MCP tools working (search_code, find_definition)

### Production (Week 16)
- [ ] Index 1M LOC in <5 minutes
- [ ] Query latency <200ms p95
- [ ] Memory usage <4GB for 1M LOC
- [ ] Retrieval accuracy >80% on test set
- [ ] Incremental update <1s for single file
- [ ] All MCP tools + resources implemented

## Technology Stack

### Core Dependencies

```toml
[dependencies]
# Parsing & Analysis
tree-sitter = "0.20"
tree-sitter-rust = "0.20"
rust-code-analysis = "0.0.25"  # Mozilla's metrics

# Storage & Indexing
qdrant-client = "1.10"         # Vector database client
rocksdb = "0.22"               # Metadata storage
memmap2 = "0.9"                # Memory-mapped files
rkyv = "0.7"                   # Zero-copy serialization

# Search & Retrieval
tantivy = "0.22"               # BM25 full-text search
fastembed = "3.4"              # Fast local embeddings

# Change Detection & Hashing
notify = "6.1"                 # File system watcher
rs_merkle = "1.4"              # Merkle tree
blake3 = "1.5"                 # Fast hashing

# Parallelism & Async
rayon = "1.10"                 # Data parallelism
tokio = { version = "1.37", features = ["full"] }

# MCP Protocol
# Use official Rust MCP SDK when available
# Or implement using:
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
async-trait = "0.1"

# ML/Embeddings
candle-core = "0.6"            # ML inference (optional for re-ranking)
hf-hub = "0.3"                 # HuggingFace model loading

# CLI & Config
clap = { version = "4.5", features = ["derive"] }
toml = "0.8"

# Optional: REST API
axum = "0.7"                   # HTTP server
tower = "0.4"                  # Middleware

# Utilities
anyhow = "1.0"                 # Error handling
tracing = "0.1"                # Logging
tracing-subscriber = "0.3"
```

## Project Structure

```
rust-codebase-mcp/
├── Cargo.toml
├── README.md
├── LICENSE
├── .github/
│   └── workflows/
│       ├── ci.yml
│       └── release.yml
├── src/
│   ├── main.rs                 # CLI entry point
│   ├── lib.rs                  # Library interface
│   ├── config.rs               # Configuration
│   │
│   ├── watcher/                # Phase 1: Foundation
│   │   ├── mod.rs
│   │   ├── file_watcher.rs     # notify-rs integration
│   │   └── merkle_tree.rs      # Change detection
│   │
│   ├── parser/                 # Phase 2: Parsing
│   │   ├── mod.rs
│   │   ├── tree_sitter.rs      # Tree-sitter parser
│   │   ├── metrics.rs          # rust-code-analysis
│   │   ├── ast.rs              # AST traversal
│   │   └── call_graph.rs       # Simple call graph
│   │
│   ├── chunker/                # Phase 3: Chunking
│   │   ├── mod.rs
│   │   ├── chunking.rs         # Smart chunking logic
│   │   ├── context.rs          # Context enrichment
│   │   └── overlap.rs          # Chunk overlap
│   │
│   ├── storage/                # Phase 4: Storage
│   │   ├── mod.rs
│   │   ├── vector_db.rs        # Qdrant client
│   │   ├── metadata_db.rs      # RocksDB
│   │   ├── memory_map.rs       # Memory-mapped indices
│   │   └── serialization.rs    # rkyv integration
│   │
│   ├── embeddings/             # Phase 5: Embeddings
│   │   ├── mod.rs
│   │   ├── model.rs            # Load embedding model
│   │   ├── encoder.rs          # Encode chunks
│   │   └── batch.rs            # Batch processing
│   │
│   ├── search/                 # Phase 6: Query
│   │   ├── mod.rs
│   │   ├── vector_search.rs    # Qdrant queries
│   │   ├── bm25.rs             # Tantivy integration
│   │   ├── hybrid.rs           # RRF fusion
│   │   ├── reranker.rs         # Cross-encoder
│   │   └── progressive.rs      # Progressive disclosure
│   │
│   ├── incremental/            # Phase 7: Incremental
│   │   ├── mod.rs
│   │   ├── dependency.rs       # File dependencies
│   │   └── invalidation.rs     # Cache invalidation
│   │
│   ├── mcp/                    # Phase 8: MCP
│   │   ├── mod.rs
│   │   ├── server.rs           # MCP server
│   │   ├── tools.rs            # Tool definitions
│   │   ├── resources.rs        # Resource handlers
│   │   └── protocol.rs         # MCP protocol impl
│   │
│   ├── api/                    # Phase 9: Interface
│   │   ├── mod.rs
│   │   ├── cli.rs              # CLI commands
│   │   ├── query_parser.rs     # Parse queries
│   │   └── rest.rs             # Optional REST API
│   │
│   └── metrics/                # Phase 10: Optimization
│       ├── mod.rs
│       ├── profiler.rs         # Performance profiling
│       └── benchmarks.rs       # Benchmark harness
│
├── tests/
│   ├── integration/
│   │   ├── indexing.rs
│   │   ├── searching.rs
│   │   └── mcp.rs
│   └── fixtures/
│       └── test_codebase/
│
├── benches/
│   ├── indexing.rs
│   ├── searching.rs
│   └── incremental.rs
│
└── examples/
    ├── basic_usage.rs
    ├── mcp_server.rs
    └── rest_api.rs
```

## Implementation Timeline

### Phase 1: Foundation (Week 1-2)

**Goal**: Set up project structure, configuration, and change detection.

#### Week 1: Project Setup
- [ ] Create Cargo workspace
- [ ] Set up CI/CD (GitHub Actions)
- [ ] Define project configuration format
- [ ] Implement config parser (TOML)

**Deliverable**: `Config` struct, `.codebase-mcp.toml` format

```toml
# .codebase-mcp.toml example
[project]
name = "my-rust-project"
root = "."

[indexing]
exclude = ["target/", ".git/", "node_modules/"]
include = ["src/", "tests/"]
max_file_size_mb = 10
parallel_workers = 8

[storage]
index_dir = "~/.cache/rust-codebase-mcp/"
memory_limit_gb = 4

[embeddings]
model = "all-MiniLM-L6-v2"
dimensions = 384
quantize = true

[search]
enable_bm25 = true
enable_vector = true
enable_reranking = true
max_results = 50
```

#### Week 2: Change Detection
- [ ] Implement file system watcher (`notify`)
- [ ] Build Merkle tree for file hashing
- [ ] Create change detection logic
- [ ] Test on large directory structures

**Deliverable**: `FileWatcher` and `MerkleTree` components

**Code sketch**:
```rust
pub struct FileWatcher {
    watcher: RecommendedWatcher,
    merkle_tree: MerkleTree,
}

impl FileWatcher {
    pub fn new(root: PathBuf) -> Result<Self>;
    pub fn detect_changes(&mut self) -> Vec<FileChange>;
}

pub struct MerkleTree {
    root: PathBuf,
    hashes: HashMap<PathBuf, Hash>,
    tree: HashMap<Hash, Vec<Hash>>,
}

impl MerkleTree {
    pub fn build(root: &Path) -> Result<Self>;
    pub fn update(&mut self, changed_files: Vec<PathBuf>) -> Vec<PathBuf>;
    pub fn verify(&self, file: &Path) -> bool;
}
```

### Phase 2: Parsing & Analysis (Week 3-4)

**Goal**: Parse Rust code, extract AST, metrics, and build simple call graphs.

#### Week 3: Tree-sitter Integration
- [ ] Set up tree-sitter-rust
- [ ] Implement AST traversal
- [ ] Extract symbols (functions, structs, traits, impls)
- [ ] Test on various Rust patterns

**Deliverable**: `Parser` component

```rust
pub struct Parser {
    parser: tree_sitter::Parser,
    language: tree_sitter::Language,
}

pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    pub range: Range,
    pub docstring: Option<String>,
}

pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
}

impl Parser {
    pub fn parse_file(&self, path: &Path) -> Result<Vec<Symbol>>;
    pub fn extract_ast(&self, source: &str) -> Result<Tree>;
}
```

#### Week 4: Metrics & Call Graph
- [ ] Integrate `rust-code-analysis`
- [ ] Extract code metrics (complexity, LOC, maintainability)
- [ ] Build simple call graph (string-based)
- [ ] Create symbol index structure

**Deliverable**: `MetricsExtractor` and `CallGraph`

```rust
pub struct CodeMetrics {
    pub cyclomatic_complexity: f64,
    pub cognitive_complexity: f64,
    pub lines_of_code: usize,
    pub maintainability_index: f64,
    pub num_functions: usize,
}

pub struct CallGraph {
    // caller -> callees
    edges: HashMap<String, Vec<String>>,
}

impl CallGraph {
    pub fn build(symbols: &[Symbol], ast: &Tree) -> Self;
    pub fn find_callers(&self, function: &str) -> Vec<&str>;
    pub fn find_callees(&self, function: &str) -> Vec<&str>;
}
```

### Phase 3: Smart Chunking (Week 5)

**Goal**: Implement intelligent code chunking with context and overlap.

- [ ] Implement semantic chunking (by function/struct/module)
- [ ] Add contextual metadata (file, module, docstring)
- [ ] Implement 20% overlap between chunks
- [ ] Enrich with imports and call relationships

**Deliverable**: `Chunker` component

```rust
pub struct CodeChunk {
    pub id: ChunkId,
    pub content: String,
    pub context: ChunkContext,
    pub overlap_prev: Option<String>,
    pub overlap_next: Option<String>,
}

pub struct ChunkContext {
    pub file_path: PathBuf,
    pub module_path: Vec<String>,
    pub symbol_name: String,
    pub symbol_kind: SymbolKind,
    pub docstring: Option<String>,
    pub imports: Vec<String>,
    pub outgoing_calls: Vec<String>,
    pub incoming_calls: Vec<String>,
    pub metrics: CodeMetrics,
    pub start_line: usize,
    pub end_line: usize,
}

pub struct Chunker {
    overlap_percentage: f64,
}

impl Chunker {
    pub fn chunk_file(&self,
        file: &Path,
        symbols: Vec<Symbol>,
        call_graph: &CallGraph,
        metrics: CodeMetrics
    ) -> Vec<CodeChunk>;
}
```

### Phase 4: Storage Layer (Week 6-7)

**Goal**: Set up Qdrant, RocksDB, and memory-mapped indices.

#### Week 6: Database Setup
- [ ] Set up local Qdrant instance
- [ ] Configure collection with optimization for scale
- [ ] Set up RocksDB for metadata
- [ ] Define storage schemas

**Deliverable**: `VectorDB` and `MetadataDB` components

```rust
pub struct VectorDB {
    client: QdrantClient,
    collection_name: String,
}

impl VectorDB {
    pub async fn new(config: &Config) -> Result<Self>;
    pub async fn upsert_chunks(&self, chunks: Vec<(ChunkId, Vec<f32>)>) -> Result<()>;
    pub async fn search(&self, query_vector: Vec<f32>, limit: usize) -> Result<Vec<ScoredPoint>>;
    pub async fn delete_chunks(&self, ids: Vec<ChunkId>) -> Result<()>;
}

pub struct MetadataDB {
    db: rocksdb::DB,
}

impl MetadataDB {
    pub fn new(path: &Path) -> Result<Self>;
    pub fn put_chunk(&self, id: ChunkId, chunk: &CodeChunk) -> Result<()>;
    pub fn get_chunk(&self, id: ChunkId) -> Result<Option<CodeChunk>>;
    pub fn get_file_chunks(&self, file: &Path) -> Result<Vec<ChunkId>>;
}
```

**Qdrant Configuration**:
```rust
let collection_config = CreateCollection {
    collection_name: "code_chunks".to_string(),
    vectors_config: Some(VectorsConfig {
        config: Some(Config::Params(VectorParams {
            size: 384,
            distance: Distance::Cosine.into(),
            ..Default::default()
        })),
    }),
    optimizers_config: Some(OptimizersConfigDiff {
        indexing_threshold: Some(10000),
        memmap_threshold: Some(50000),
        ..Default::default()
    }),
    hnsw_config: Some(HnswConfigDiff {
        m: Some(16),
        ef_construct: Some(100),
        payload_m: Some(8),
        ..Default::default()
    }),
    ..Default::default()
};
```

#### Week 7: Memory-Mapped Indices
- [ ] Implement memory-mapped symbol index
- [ ] Use `rkyv` for zero-copy serialization
- [ ] Create batch processing pipeline
- [ ] Test memory usage patterns

**Deliverable**: `MemoryMappedIndex` component

```rust
pub struct MemoryMappedIndex {
    mmap: Mmap,
    index: ArchivedSymbolIndex,
}

#[derive(Archive, Serialize, Deserialize)]
pub struct SymbolIndex {
    symbols: Vec<Symbol>,
    file_map: HashMap<PathBuf, Vec<usize>>,
}

impl MemoryMappedIndex {
    pub fn create(path: &Path, index: SymbolIndex) -> Result<Self>;
    pub fn open(path: &Path) -> Result<Self>;
    pub fn query_file(&self, file: &Path) -> Vec<&ArchivedSymbol>;
}
```

### Phase 5: Embeddings (Week 8)

**Goal**: Load embedding model, encode chunks, stream to Qdrant.

- [ ] Load `all-MiniLM-L6-v2` with quantization
- [ ] Implement batch encoding
- [ ] Create embedding pipeline
- [ ] Add progress reporting
- [ ] Test on large codebases

**Deliverable**: `EmbeddingModel` component

```rust
pub struct EmbeddingModel {
    model: fastembed::TextEmbedding,
    dimensions: usize,
}

impl EmbeddingModel {
    pub fn new(model_name: &str, quantize: bool) -> Result<Self>;

    pub fn embed(&self, text: &str) -> Result<Vec<f32>>;

    pub fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
}

pub struct EmbeddingPipeline {
    model: EmbeddingModel,
    vector_db: VectorDB,
    batch_size: usize,
}

impl EmbeddingPipeline {
    pub async fn process_chunks(
        &self,
        chunks: Vec<CodeChunk>,
        progress: impl Fn(usize, usize)
    ) -> Result<()>;
}
```

**Chunk Text Formatting** (Anthropic's contextual approach):
```rust
fn format_chunk_for_embedding(chunk: &CodeChunk) -> String {
    format!(
        "// File: {}\n\
         // Module: {}\n\
         // Symbol: {} ({})\n\
         // Purpose: {}\n\
         // Imports: {}\n\
         \n\
         {}",
        chunk.context.file_path.display(),
        chunk.context.module_path.join("::"),
        chunk.context.symbol_name,
        chunk.context.symbol_kind,
        chunk.context.docstring.as_deref().unwrap_or(""),
        chunk.context.imports.join(", "),
        chunk.content
    )
}
```

### Phase 6: Query & Retrieval (Week 9-10)

**Goal**: Implement hybrid search with re-ranking.

#### Week 9: Basic Search
- [ ] Implement vector search (Qdrant)
- [ ] Implement BM25 search (Tantivy)
- [ ] Create Tantivy index structure
- [ ] Test both search methods

**Deliverable**: `VectorSearch` and `BM25Search`

```rust
pub struct VectorSearch {
    vector_db: VectorDB,
    embedding_model: EmbeddingModel,
}

impl VectorSearch {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
}

pub struct BM25Search {
    index: tantivy::Index,
    reader: tantivy::IndexReader,
}

impl BM25Search {
    pub fn new(index_dir: &Path) -> Result<Self>;
    pub fn index_chunks(&self, chunks: &[CodeChunk]) -> Result<()>;
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
}

pub struct SearchResult {
    pub chunk_id: ChunkId,
    pub score: f32,
    pub snippet: String,
}
```

#### Week 10: Hybrid Search & Re-ranking
- [ ] Implement Reciprocal Rank Fusion (RRF)
- [ ] Add cross-encoder re-ranking (optional)
- [ ] Implement progressive disclosure
- [ ] Benchmark retrieval accuracy

**Deliverable**: `HybridSearch` and `Reranker`

```rust
pub struct HybridSearch {
    vector_search: VectorSearch,
    bm25_search: BM25Search,
    metadata_db: MetadataDB,
}

impl HybridSearch {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // 1. Get candidates from both
        let vector_results = self.vector_search.search(query, 100).await?;
        let bm25_results = self.bm25_search.search(query, 100)?;

        // 2. Reciprocal Rank Fusion
        let merged = reciprocal_rank_fusion(vector_results, bm25_results);

        // 3. Re-rank top 20 (optional)
        let reranked = self.rerank(query, merged.take(20)).await?;

        // 4. Return top N
        Ok(reranked.take(limit).collect())
    }
}

fn reciprocal_rank_fusion(
    results1: Vec<SearchResult>,
    results2: Vec<SearchResult>,
    k: f32,
) -> Vec<SearchResult> {
    // RRF formula: score = sum(1 / (k + rank))
    // where k=60 is typical
}

pub struct Reranker {
    model: Option<candle_core::Model>, // Optional cross-encoder
}

impl Reranker {
    pub async fn rerank(
        &self,
        query: &str,
        results: Vec<SearchResult>
    ) -> Result<Vec<SearchResult>>;
}
```

**Progressive Disclosure**:
```rust
pub struct ProgressiveResults {
    pub summaries: Vec<ChunkSummary>,  // Return these first
    pub chunk_ids: Vec<ChunkId>,       // LLM can request full content
}

pub struct ChunkSummary {
    pub file: PathBuf,
    pub symbol: String,
    pub docstring: Option<String>,
    pub score: f32,
    pub snippet: String,  // First 3 lines
}
```

### Phase 7: Incremental Updates (Week 11)

**Goal**: Only reindex changed files and their dependents.

- [ ] Build dependency graph between files
- [ ] Implement cache invalidation logic
- [ ] Handle file deletions
- [ ] Test incremental performance

**Deliverable**: `IncrementalIndexer`

```rust
pub struct DependencyGraph {
    // file -> files it imports
    dependencies: HashMap<PathBuf, Vec<PathBuf>>,
    // file -> files that import it
    dependents: HashMap<PathBuf, Vec<PathBuf>>,
}

impl DependencyGraph {
    pub fn build(files: &[PathBuf], parser: &Parser) -> Result<Self>;
    pub fn get_dependents(&self, file: &Path) -> Vec<&PathBuf>;
    pub fn update(&mut self, changed_file: &Path, new_deps: Vec<PathBuf>);
}

pub struct IncrementalIndexer {
    merkle_tree: MerkleTree,
    dependency_graph: DependencyGraph,
    vector_db: VectorDB,
    metadata_db: MetadataDB,
}

impl IncrementalIndexer {
    pub async fn reindex(&mut self, changed_files: Vec<PathBuf>) -> Result<Stats> {
        // 1. Find all affected files (changed + dependents)
        let mut affected = changed_files.clone();
        for file in &changed_files {
            affected.extend(self.dependency_graph.get_dependents(file));
        }

        // 2. Delete old chunks
        for file in &affected {
            let chunk_ids = self.metadata_db.get_file_chunks(file)?;
            self.vector_db.delete_chunks(chunk_ids).await?;
        }

        // 3. Reindex affected files only
        for file in &affected {
            self.index_file(file).await?;
        }

        // 4. Update dependency graph
        self.dependency_graph.update_batch(&affected)?;

        Ok(Stats { reindexed: affected.len() })
    }
}
```

### Phase 8: MCP Protocol (Week 12-13)

**Goal**: Implement Model Context Protocol server with tools and resources.

#### Week 12: MCP Server
- [ ] Implement MCP protocol (stdio transport)
- [ ] Define tool schemas
- [ ] Define resource schemas
- [ ] Handle protocol messages

**Deliverable**: `MCPServer`

```rust
pub struct MCPServer {
    indexer: Arc<IncrementalIndexer>,
    search: Arc<HybridSearch>,
    config: Config,
}

impl MCPServer {
    pub async fn run(&self) -> Result<()> {
        // Read from stdin, write to stdout
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        loop {
            let message = self.read_message(&stdin).await?;
            let response = self.handle_message(message).await?;
            self.write_message(&stdout, response).await?;
        }
    }

    async fn handle_message(&self, msg: Message) -> Result<Response>;
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum Message {
    #[serde(rename = "tools/list")]
    ListTools,

    #[serde(rename = "tools/call")]
    CallTool { name: String, arguments: Value },

    #[serde(rename = "resources/list")]
    ListResources,

    #[serde(rename = "resources/read")]
    ReadResource { uri: String },
}
```

#### Week 13: Tools & Resources
- [ ] Implement all MCP tools
- [ ] Implement all MCP resources
- [ ] Add error handling
- [ ] Test with Claude

**MCP Tools**:
```rust
pub enum Tool {
    SearchCode,
    FindDefinition,
    FindReferences,
    GetDependencies,
    GetCallGraph,
    AnalyzeComplexity,
    GetSimilarCode,
}

impl MCPServer {
    async fn search_code(&self, query: String, limit: usize) -> Result<Value> {
        let results = self.search.search(&query, limit).await?;
        Ok(json!({
            "results": results.iter().map(|r| {
                let chunk = self.indexer.metadata_db.get_chunk(r.chunk_id)?;
                json!({
                    "file": chunk.context.file_path,
                    "symbol": chunk.context.symbol_name,
                    "score": r.score,
                    "snippet": r.snippet,
                })
            }).collect::<Result<Vec<_>>>()?
        }))
    }

    async fn find_definition(&self, symbol: String) -> Result<Value>;
    async fn find_references(&self, symbol: String) -> Result<Value>;
    async fn get_dependencies(&self, file: String) -> Result<Value>;
    async fn get_call_graph(&self, function: String) -> Result<Value>;
    async fn analyze_complexity(&self, file: String) -> Result<Value>;
    async fn get_similar_code(&self, snippet: String) -> Result<Value>;
}
```

**MCP Resources**:
```rust
// rust:///{crate}/symbols
// rust:///{file}/ast
// rust:///{file}/metrics
// rust:///{symbol}/docs
// rust:///{symbol}/references

impl MCPServer {
    async fn read_resource(&self, uri: &str) -> Result<Value> {
        match uri {
            uri if uri.starts_with("rust:///") => {
                let path = &uri[8..]; // Remove "rust:///"
                let parts: Vec<&str> = path.split('/').collect();

                match parts.as_slice() {
                    [file, "ast"] => self.get_ast(file).await,
                    [file, "metrics"] => self.get_metrics(file).await,
                    [symbol, "docs"] => self.get_docs(symbol).await,
                    [symbol, "references"] => self.get_references(symbol).await,
                    _ => Err(anyhow!("Invalid resource URI")),
                }
            }
            _ => Err(anyhow!("Unsupported URI scheme")),
        }
    }
}
```

### Phase 9: Interface (Week 14)

**Goal**: Build CLI and optional REST API.

- [ ] Implement CLI commands
- [ ] Add query parser
- [ ] Create interactive mode
- [ ] Optional: REST API with axum

**Deliverable**: CLI and API

```rust
#[derive(Parser)]
#[command(name = "rust-code-mcp")]
#[command(about = "Scalable Rust codebase search via MCP")]
pub enum Cli {
    /// Initialize a new project
    Init {
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Index the codebase
    Index {
        #[arg(short, long)]
        path: Option<PathBuf>,
    },

    /// Search the codebase
    Search {
        query: String,
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Start MCP server
    Serve {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },

    /// Start REST API server
    Api {
        #[arg(short, long, default_value = "3000")]
        port: u16,
    },

    /// Show project statistics
    Stats,
}
```

**REST API** (optional):
```rust
pub async fn start_api_server(port: u16, indexer: Arc<IncrementalIndexer>) -> Result<()> {
    let app = Router::new()
        .route("/search", post(search_handler))
        .route("/definition/:symbol", get(definition_handler))
        .route("/references/:symbol", get(references_handler))
        .route("/stats", get(stats_handler))
        .layer(Extension(indexer));

    axum::Server::bind(&format!("0.0.0.0:{}", port).parse()?)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}
```

### Phase 10: Optimization & Polish (Week 15-16)

**Goal**: Benchmark, optimize, and prepare for release.

#### Week 15: Performance
- [ ] Benchmark on 1M LOC codebase (e.g., rustc)
- [ ] Profile memory usage
- [ ] Optimize hot paths
- [ ] Add performance metrics

**Benchmarking**:
```rust
// benches/indexing.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_indexing(c: &mut Criterion) {
    c.bench_function("index 10k LOC", |b| {
        b.iter(|| {
            // Benchmark indexing
        });
    });
}

fn bench_search(c: &mut Criterion) {
    c.bench_function("search query", |b| {
        b.iter(|| {
            // Benchmark search
        });
    });
}

criterion_group!(benches, bench_indexing, bench_search);
criterion_main!(benches);
```

**Metrics to Track**:
```rust
pub struct IndexStats {
    pub total_files: usize,
    pub total_chunks: usize,
    pub total_symbols: usize,
    pub index_time: Duration,
    pub memory_usage: usize,
}

pub struct QueryStats {
    pub query: String,
    pub results_count: usize,
    pub vector_time: Duration,
    pub bm25_time: Duration,
    pub rerank_time: Duration,
    pub total_time: Duration,
}
```

#### Week 16: Release Prep
- [ ] Write comprehensive documentation
- [ ] Create usage examples
- [ ] Set up GitHub repo
- [ ] Write README, CONTRIBUTING
- [ ] Tag v0.1.0 release

## Testing Strategy

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree_change_detection() {
        // Test merkle tree correctly detects changes
    }

    #[test]
    fn test_chunker_overlap() {
        // Test chunks have correct 20% overlap
    }

    #[tokio::test]
    async fn test_vector_search() {
        // Test vector search returns relevant results
    }
}
```

### Integration Tests
```rust
// tests/integration/indexing.rs
#[tokio::test]
async fn test_full_indexing_pipeline() {
    // Create test codebase
    // Index it
    // Verify all chunks stored
    // Verify search works
}

// tests/integration/mcp.rs
#[tokio::test]
async fn test_mcp_protocol() {
    // Start MCP server
    // Send tool call
    // Verify response
}
```

### Benchmarks
- Index speed: 10k, 100k, 1M LOC
- Search latency: p50, p95, p99
- Memory usage over time
- Incremental update speed

## Deployment Strategy

### Local Installation
```bash
cargo install --git https://github.com/yourusername/rust-codebase-mcp
```

### Docker
```dockerfile
FROM rust:1.77 as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3
COPY --from=builder /app/target/release/rust-code-mcp /usr/local/bin/
CMD ["rust-code-mcp", "serve"]
```

### MCP Integration
```json
// Claude Desktop config
{
  "mcpServers": {
    "rust-codebase": {
      "command": "rust-code-mcp",
      "args": ["serve", "--config", "/path/to/.codebase-mcp.toml"]
    }
  }
}
```

## Risk Assessment & Mitigation

### Technical Risks

**Risk 1: Embedding model size/speed**
- **Mitigation**: Use quantized models, benchmark early
- **Fallback**: Support multiple model sizes

**Risk 2: Qdrant performance at scale**
- **Mitigation**: Memory-mapped indices, proper config
- **Fallback**: Support alternative vector DBs (Milvus)

**Risk 3: Tree-sitter parsing edge cases**
- **Mitigation**: Extensive testing, graceful degradation
- **Fallback**: Skip unparseable files, log errors

**Risk 4: Memory usage on large codebases**
- **Mitigation**: Streaming, memory limits, profiling
- **Fallback**: Batch processing, disk spillover

### Project Risks

**Risk 1: Scope creep**
- **Mitigation**: Stick to MVP features, defer enhancements
- **Timeline buffer**: 2 weeks in Phase 10

**Risk 2: External dependencies breaking**
- **Mitigation**: Pin versions, integration tests
- **Monitor**: Dependabot for security updates

**Risk 3: Low adoption**
- **Mitigation**: Document well, post on HN/Reddit early
- **Community**: Build in public, accept contributions

## Success Criteria

### MVP Success (Week 10)
- [ ] Can index 100k LOC in <2 min
- [ ] Search works via MCP with Claude
- [ ] Incremental updates <5s
- [ ] No crashes on tested codebases

### Production Success (Week 16)
- [ ] Handles 1M+ LOC codebases
- [ ] Query latency <200ms p95
- [ ] Memory usage <4GB for 1M LOC
- [ ] Retrieval accuracy >80%
- [ ] Documentation complete
- [ ] 5+ example codebases tested

### Community Success (6 months)
- [ ] 100+ GitHub stars
- [ ] 10+ contributors
- [ ] Used in production by 5+ teams
- [ ] Featured in Rust newsletter

## Post-MVP Roadmap

### v0.2 (Month 5-6)
- [ ] Multi-language support (use tree-sitter grammars)
- [ ] Distributed indexing (horizontal scale)
- [ ] Web UI for exploration
- [ ] Custom fine-tuned embedding model

### v0.3 (Month 7-9)
- [ ] GNN-based re-ranking
- [ ] On-demand rust-analyzer integration
- [ ] Abstract interpretation for dataflow
- [ ] Cross-crate analysis

### v1.0 (Month 10-12)
- [ ] Production-hardened
- [ ] Performance optimizations
- [ ] Enterprise features (SSO, audit logs)
- [ ] Commercial support option

## Resources & References

### Documentation
- Tree-sitter: https://tree-sitter.github.io/
- Qdrant: https://qdrant.tech/documentation/
- Tantivy: https://github.com/quickwit-oss/tantivy
- MCP Spec: https://modelcontextprotocol.io/

### Research Papers
- Anthropic Contextual Retrieval (2024)
- GitHub Copilot Architecture (2024)
- BigDataflow (Academic, distributed analysis)

### Similar Projects
- Bloop (archived): https://github.com/BloopAI/bloop
- CocoIndex: https://github.com/cocoindex-io/cocoindex
- rust-code-analysis: https://github.com/mozilla/rust-code-analysis

### Community
- Rust subreddit: /r/rust
- Discord: Rust Programming Language Community
- Forum: https://users.rust-lang.org/

## Contributing

See CONTRIBUTING.md (to be created) for:
- Code style guidelines
- PR process
- Testing requirements
- Documentation standards

## License

MIT or Apache-2.0 (dual license, standard for Rust)

---

**Next Steps**: Begin Phase 1, Week 1 - Project Setup
