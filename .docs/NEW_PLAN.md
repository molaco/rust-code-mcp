# NEW IMPLEMENTATION PLAN: Rust Codebase Search MCP
## Accelerated Development with Maximum Code Reuse

**Date:** 2025-10-17
**Status:** Ready to Start
**Estimated Timeline:** 16 weeks (4 months)
**Time Saved:** 24-38 weeks (6-9 months) vs from-scratch

---

## Executive Summary

After comprehensive research, we've identified that **50-60% of the required functionality already exists** in open-source projects. By forking `file-search-mcp` and extracting patterns from `bloop`, we can reduce development time from 40+ weeks to just 16 weeks.

### Key Changes from Original Plan

| Original Plan | New Plan | Savings |
|---------------|----------|---------|
| Build MCP from scratch | Fork file-search-mcp | 8-12 weeks |
| Implement all indexing | Extract from bloop | 6-10 weeks |
| Build all components | Use mature libraries | 10-16 weeks |
| **Total: 40-54 weeks** | **Total: 16 weeks** | **24-38 weeks** |

---

## Foundation: What We're Building On

### 1. **file-search-mcp** (Base Project - 50% Complete)
- **GitHub:** https://github.com/Kurogoma4D/file-search-mcp
- **What it has:**
  - ✅ Complete MCP server implementation
  - ✅ Tantivy integration (BM25 search)
  - ✅ File traversal and detection
  - ✅ Score-based ranking
  - ✅ Clean, modern codebase (2024-2025)
- **What we'll add:**
  - Semantic/vector search (Qdrant)
  - Code-aware chunking (tree-sitter + text-splitter)
  - Incremental updates (notify)
  - Rust-specific optimizations

### 2. **BloopAI/bloop** (Code Extraction Source)
- **GitHub:** https://github.com/BloopAI/bloop
- **Status:** Archived but production-tested
- **What to extract from `server/bleep/src/`:**
  - Qdrant integration patterns
  - Tree-sitter parsing logic
  - Hybrid search (RRF) implementation
  - Change detection and reindexing
  - File watching patterns

### 3. **Production-Ready Libraries**

All core functionality covered by mature crates:

```toml
[dependencies]
# MCP Protocol (official)
rust-mcp-sdk = "0.1"                    # or rmcp for official SDK

# Search engines
tantivy = "0.22"                        # BM25 full-text (already in file-search-mcp)
qdrant-client = "1.9"                   # Vector search

# Embeddings & ML
fastembed = "3.4"                       # Local embeddings (ONNX)

# Code parsing & chunking
tree-sitter = "0.20"
tree-sitter-rust = "0.20"
text-splitter = "0.13"                  # Semantic code chunking

# File operations
ignore = "0.4"                          # .gitignore-aware traversal
notify = "6.1"                          # Change detection

# Performance
rayon = "1.10"                          # Parallel processing
tokio = { version = "1.37", features = ["full"] }

# Utilities
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
tracing = "0.1"
```

---

## Revised Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    MCP Client (Claude)                       │
└─────────────────────┬───────────────────────────────────────┘
                      │ stdio/JSON-RPC
┌─────────────────────▼───────────────────────────────────────┐
│              MCP Server (from file-search-mcp)               │
│                  Enhanced with new tools                     │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│               Hybrid Search Coordinator                      │
│                    (NEW - Week 9-10)                         │
│  ┌────────────────────────┐   ┌──────────────────────────┐  │
│  │ Tantivy (BM25)         │   │ Qdrant (Semantic)        │  │
│  │ From file-search-mcp ✓ │   │ NEW - Week 8            │  │
│  └────────────────────────┘   └──────────────────────────┘  │
│                 └────────┬──────────┘                        │
│                          │                                   │
│                  ┌───────▼────────┐                          │
│                  │ RRF Fusion     │                          │
│                  │ From bloop     │                          │
│                  └────────────────┘                          │
└───────────────────────────────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                 Indexing Pipeline                            │
│                                                              │
│  File Watcher (notify) ──> NEW - Week 3                     │
│         │                                                    │
│         ▼                                                    │
│  File Traversal (ignore) ──> From file-search-mcp ✓         │
│         │                                                    │
│         ▼                                                    │
│  Tree-sitter Parser ──> NEW - Week 5 (patterns from bloop)  │
│         │                                                    │
│         ▼                                                    │
│  Code Chunker (text-splitter) ──> NEW - Week 6              │
│         │                                                    │
│         ├──────────────────┬───────────────────────┐        │
│         │                  │                       │        │
│         ▼                  ▼                       ▼        │
│  Tantivy Index      fastembed-rs            Qdrant Index    │
│  (existing ✓)       (NEW - Week 7)          (NEW - Week 8)  │
└───────────────────────────────────────────────────────────────┘
```

---

## Implementation Timeline: 16 Weeks

### **Phase 0: Setup & Forking** (Week 1)

#### Goals
- Fork file-search-mcp as base
- Clone bloop for reference
- Set up development environment
- Understand existing codebase

#### Tasks
- [ ] Fork file-search-mcp: `git clone https://github.com/Kurogoma4D/file-search-mcp.git rust-code-mcp`
- [ ] Clone bloop: `git clone https://github.com/BloopAI/bloop.git`
- [ ] Install Qdrant: `docker run -d -p 6333:6333 qdrant/qdrant`
- [ ] Set up Rust toolchain (latest stable)
- [ ] Create test repositories (small, medium, large Rust projects)
- [ ] Document file-search-mcp architecture
- [ ] Identify code to extract from bloop (`server/bleep/src/`)

#### Deliverables
- Forked repository with new name
- Dev environment ready
- Architecture documentation
- Extraction plan for bloop code

#### Code References
**From file-search-mcp:**
- Study `src/main.rs` - MCP server setup
- Study `src/search.rs` - Tantivy integration
- Study `src/indexer.rs` - File indexing logic

**From bloop to study:**
- `server/bleep/src/indexes/reader.rs` - Search patterns
- `server/bleep/src/indexes/writer.rs` - Indexing patterns
- `server/bleep/src/intelligence/code_navigation/` - Tree-sitter usage
- `server/bleep/src/semantic/` - Qdrant integration

---

### **Phase 1: Enhance Base MCP Server** (Week 2-4)

#### Goals
- Add persistent Tantivy index (currently in-memory)
- Implement incremental file watching
- Improve code-aware schema
- Test on real codebases

#### Week 2: Persistent Storage
**Tasks:**
- [ ] Convert in-memory Tantivy to on-disk
- [ ] Add index configuration (location, size limits)
- [ ] Implement index versioning
- [ ] Add metadata caching (file hashes, modified times)

**Code to write:**
```rust
// src/storage/mod.rs
pub struct PersistentIndex {
    tantivy_index: tantivy::Index,
    index_path: PathBuf,
    metadata_cache: HashMap<PathBuf, FileMetadata>,
}

struct FileMetadata {
    modified_time: SystemTime,
    content_hash: [u8; 32],
    indexed_at: SystemTime,
}

impl PersistentIndex {
    pub fn open_or_create(path: &Path) -> Result<Self>;
    pub fn needs_reindex(&self, file: &Path) -> Result<bool>;
}
```

**Extract from bloop:**
- Index directory structure: `server/bleep/src/indexes/mod.rs`
- Metadata tracking patterns

#### Week 3: Change Detection
**Tasks:**
- [ ] Integrate `notify` crate for file watching
- [ ] Implement debouncing (avoid rapid re-indexes)
- [ ] Handle file creation, modification, deletion
- [ ] Test with large directory changes

**Code to write:**
```rust
// src/watcher/mod.rs
use notify::{Watcher, RecursiveMode, DebouncedEvent};

pub struct FileWatcher {
    watcher: RecommendedWatcher,
    index: Arc<Mutex<PersistentIndex>>,
}

impl FileWatcher {
    pub fn watch(&mut self, path: &Path) -> Result<()> {
        let (tx, rx) = channel();
        self.watcher.watch(path, RecursiveMode::Recursive)?;

        for event in rx {
            match event {
                DebouncedEvent::Create(path) |
                DebouncedEvent::Write(path) => self.reindex_file(path)?,
                DebouncedEvent::Remove(path) => self.remove_file(path)?,
                _ => {}
            }
        }
        Ok(())
    }
}
```

**Extract from bloop:**
- Change detection: `server/bleep/src/background.rs`
- Debouncing patterns

#### Week 4: Code-Aware Schema
**Tasks:**
- [ ] Enhance Tantivy schema for code
- [ ] Add fields: file_path, function_name, struct_name, language
- [ ] Add docstring indexing
- [ ] Test search quality improvements

**Code to write:**
```rust
// src/schema.rs
pub fn build_code_schema() -> tantivy::schema::Schema {
    let mut schema_builder = Schema::builder();

    // Exact match fields
    schema_builder.add_text_field("file_path", STRING | STORED);
    schema_builder.add_text_field("symbol_name", TEXT | STORED);
    schema_builder.add_text_field("symbol_type", STRING); // function, struct, etc.

    // Full-text search
    schema_builder.add_text_field("code", TEXT);
    schema_builder.add_text_field("docstring", TEXT);

    // Metadata
    schema_builder.add_text_field("language", STRING | STORED);
    schema_builder.add_u64_field("line_start", STORED);
    schema_builder.add_u64_field("line_end", STORED);

    schema_builder.build()
}
```

**Reference:**
- Tantivy schema examples: https://github.com/quickwit-oss/tantivy/blob/main/examples/basic_search.rs
- Bloop's schema: `server/bleep/src/indexes/schema.rs`

#### Deliverables (Phase 1)
- ✅ Persistent Tantivy index
- ✅ File change detection working
- ✅ Enhanced schema for code
- ✅ Tested on 10k+ files

---

### **Phase 2: Tree-sitter Integration** (Week 5-6)

#### Goals
- Parse Rust code with tree-sitter
- Extract symbols (functions, structs, traits)
- Generate simple call graphs
- Prepare for semantic chunking

#### Week 5: Basic Parsing
**Tasks:**
- [ ] Add tree-sitter and tree-sitter-rust dependencies
- [ ] Implement AST parser
- [ ] Extract symbols (functions, structs, enums, traits, impls)
- [ ] Extract docstrings
- [ ] Test on various Rust patterns

**Code to write:**
```rust
// src/parser/mod.rs
use tree_sitter::{Parser, Tree, Node};

pub struct RustParser {
    parser: Parser,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    pub range: Range,
    pub docstring: Option<String>,
    pub visibility: Visibility,
}

#[derive(Debug, Clone)]
pub enum SymbolKind {
    Function { is_async: bool, is_unsafe: bool },
    Struct,
    Enum,
    Trait,
    Impl { trait_name: Option<String> },
    Module,
    Const,
    Static,
}

impl RustParser {
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
        Ok(Self { parser })
    }

    pub fn parse_file(&mut self, path: &Path) -> Result<Vec<Symbol>> {
        let source = fs::read_to_string(path)?;
        let tree = self.parser.parse(&source, None)
            .ok_or_else(|| anyhow!("Failed to parse"))?;

        self.extract_symbols(&tree, &source)
    }

    fn extract_symbols(&self, tree: &Tree, source: &str) -> Result<Vec<Symbol>> {
        let mut symbols = Vec::new();
        let root = tree.root_node();

        self.traverse_node(root, source, &mut symbols);
        Ok(symbols)
    }

    fn traverse_node(&self, node: Node, source: &str, symbols: &mut Vec<Symbol>) {
        // Walk AST and extract symbols
        match node.kind() {
            "function_item" => symbols.push(self.extract_function(node, source)),
            "struct_item" => symbols.push(self.extract_struct(node, source)),
            "enum_item" => symbols.push(self.extract_enum(node, source)),
            "trait_item" => symbols.push(self.extract_trait(node, source)),
            "impl_item" => symbols.push(self.extract_impl(node, source)),
            _ => {}
        }

        // Recurse
        for child in node.children(&mut node.walk()) {
            self.traverse_node(child, source, symbols);
        }
    }
}
```

**Extract from bloop:**
- Tree-sitter patterns: `server/bleep/src/intelligence/code_navigation/language_parsers.rs`
- Symbol extraction: `server/bleep/src/intelligence/scope.rs`

#### Week 6: Call Graph & Imports
**Tasks:**
- [ ] Extract function calls
- [ ] Extract import statements (use declarations)
- [ ] Build simple call graph (string-based, no type resolution)
- [ ] Track dependencies between files

**Code to write:**
```rust
// src/parser/call_graph.rs
pub struct CallGraph {
    // caller -> callees
    edges: HashMap<String, Vec<String>>,
}

impl CallGraph {
    pub fn build(symbols: &[Symbol], tree: &Tree, source: &str) -> Self {
        let mut graph = Self { edges: HashMap::new() };

        // Find all function calls
        let root = tree.root_node();
        Self::find_calls(root, source, &mut graph.edges);

        graph
    }

    fn find_calls(node: Node, source: &str, edges: &mut HashMap<String, Vec<String>>) {
        if node.kind() == "call_expression" {
            // Extract caller and callee
            // ...
        }

        for child in node.children(&mut node.walk()) {
            Self::find_calls(child, source, edges);
        }
    }

    pub fn find_callers(&self, function: &str) -> Vec<&str> {
        self.edges.iter()
            .filter(|(_, callees)| callees.contains(&function.to_string()))
            .map(|(caller, _)| caller.as_str())
            .collect()
    }
}

// src/parser/imports.rs
pub fn extract_imports(tree: &Tree, source: &str) -> Vec<String> {
    let root = tree.root_node();
    let mut imports = Vec::new();

    for child in root.children(&mut root.walk()) {
        if child.kind() == "use_declaration" {
            let import = child.utf8_text(source.as_bytes()).unwrap();
            imports.push(import.to_string());
        }
    }

    imports
}
```

**Reference:**
- tree-sitter-graph for queries: https://github.com/tree-sitter/tree-sitter-graph
- Bloop's call graph: `server/bleep/src/intelligence/code_navigation/`

#### Deliverables (Phase 2)
- ✅ Parse Rust files with tree-sitter
- ✅ Extract all symbol types
- ✅ Simple call graph working
- ✅ Import tracking

---

### **Phase 3: Semantic Chunking** (Week 7)

#### Goals
- Split code into semantic chunks using text-splitter
- Add context to chunks (file, module, docstring)
- Implement 20% overlap
- Prepare chunks for embedding

#### Tasks
- [ ] Integrate `text-splitter` with `CodeSplitter`
- [ ] Chunk by functions/structs/modules
- [ ] Enrich chunks with context metadata
- [ ] Add overlap between chunks
- [ ] Format chunks for embedding (Anthropic's contextual approach)

**Code to write:**
```rust
// src/chunker/mod.rs
use text_splitter::{TextSplitter, CodeSplitter};

#[derive(Debug, Clone)]
pub struct CodeChunk {
    pub id: ChunkId,
    pub content: String,
    pub context: ChunkContext,
    pub overlap_prev: Option<String>,
    pub overlap_next: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChunkContext {
    pub file_path: PathBuf,
    pub module_path: Vec<String>,
    pub symbol_name: String,
    pub symbol_kind: SymbolKind,
    pub docstring: Option<String>,
    pub imports: Vec<String>,
    pub outgoing_calls: Vec<String>,
    pub line_start: usize,
    pub line_end: usize,
}

pub struct Chunker {
    splitter: CodeSplitter,
    overlap_percentage: f64,
}

impl Chunker {
    pub fn new() -> Self {
        let splitter = CodeSplitter::new(tree_sitter_rust::LANGUAGE.into())
            .with_trim(true);

        Self {
            splitter,
            overlap_percentage: 0.2, // 20% overlap
        }
    }

    pub fn chunk_file(
        &self,
        file: &Path,
        symbols: Vec<Symbol>,
        call_graph: &CallGraph,
        imports: Vec<String>,
    ) -> Result<Vec<CodeChunk>> {
        let source = fs::read_to_string(file)?;
        let chunks = self.splitter.chunks(&source, 512); // Max 512 tokens

        let mut result = Vec::new();

        for (idx, chunk_text) in chunks.enumerate() {
            // Find which symbol this chunk belongs to
            let symbol = self.find_symbol_for_chunk(chunk_text, &symbols);

            // Get context
            let context = ChunkContext {
                file_path: file.to_path_buf(),
                module_path: self.extract_module_path(file),
                symbol_name: symbol.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
                symbol_kind: symbol.as_ref().map(|s| s.kind.clone()).unwrap_or(SymbolKind::Module),
                docstring: symbol.and_then(|s| s.docstring),
                imports: imports.clone(),
                outgoing_calls: call_graph.get_callees(&symbol_name).to_vec(),
                line_start: chunk_text.start_line(),
                line_end: chunk_text.end_line(),
            };

            // Add overlap
            let overlap_prev = if idx > 0 {
                Some(self.get_overlap(&chunks[idx - 1]))
            } else {
                None
            };

            let overlap_next = if idx < chunks.len() - 1 {
                Some(self.get_overlap(&chunks[idx + 1]))
            } else {
                None
            };

            result.push(CodeChunk {
                id: ChunkId::new(),
                content: chunk_text.to_string(),
                context,
                overlap_prev,
                overlap_next,
            });
        }

        Ok(result)
    }

    fn get_overlap(&self, chunk: &str) -> String {
        let overlap_size = (chunk.len() as f64 * self.overlap_percentage) as usize;
        chunk.chars().take(overlap_size).collect()
    }
}

// Format chunk for embedding (Anthropic's contextual retrieval approach)
pub fn format_for_embedding(chunk: &CodeChunk) -> String {
    format!(
        "// File: {}\n\
         // Module: {}\n\
         // Symbol: {} ({})\n\
         // Purpose: {}\n\
         // Imports: {}\n\
         // Calls: {}\n\
         \n\
         {}",
        chunk.context.file_path.display(),
        chunk.context.module_path.join("::"),
        chunk.context.symbol_name,
        chunk.context.symbol_kind.as_str(),
        chunk.context.docstring.as_deref().unwrap_or(""),
        chunk.context.imports.join(", "),
        chunk.context.outgoing_calls.join(", "),
        chunk.content
    )
}
```

**References:**
- text-splitter docs: https://docs.rs/text-splitter/
- text-splitter examples: https://github.com/benbrandt/text-splitter/tree/main/examples
- Anthropic contextual retrieval: (reduces errors by 49%)

#### Deliverables (Phase 3)
- ✅ Semantic code chunking working
- ✅ Context-enriched chunks
- ✅ 20% overlap implemented
- ✅ Formatted for embedding

---

### **Phase 4: Embedding Generation** (Week 8)

#### Goals
- Integrate fastembed-rs for local embeddings
- Generate embeddings for all chunks
- Optimize for performance (batch processing)
- Prepare for Qdrant indexing

#### Tasks
- [ ] Add fastembed-rs dependency
- [ ] Load all-MiniLM-L6-v2 model (384 dimensions)
- [ ] Implement batch embedding pipeline
- [ ] Add progress reporting
- [ ] Test embedding quality

**Code to write:**
```rust
// src/embeddings/mod.rs
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};

pub struct EmbeddingGenerator {
    model: TextEmbedding,
    dimensions: usize,
}

impl EmbeddingGenerator {
    pub fn new() -> Result<Self> {
        // Load all-MiniLM-L6-v2 (384 dimensions)
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2)
                .with_show_download_progress(true)
        )?;

        Ok(Self {
            model,
            dimensions: 384,
        })
    }

    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.model.embed(vec![text], None)?;
        Ok(embeddings.into_iter().next().unwrap())
    }

    pub fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.model.embed(texts, None)
    }
}

// src/embeddings/pipeline.rs
pub struct EmbeddingPipeline {
    generator: EmbeddingGenerator,
    batch_size: usize,
}

impl EmbeddingPipeline {
    pub fn new(generator: EmbeddingGenerator) -> Self {
        Self {
            generator,
            batch_size: 32, // Process 32 chunks at a time
        }
    }

    pub fn process_chunks(
        &self,
        chunks: Vec<CodeChunk>,
        progress: impl Fn(usize, usize),
    ) -> Result<Vec<(ChunkId, Vec<f32>)>> {
        let total = chunks.len();
        let mut results = Vec::new();

        // Format all chunks for embedding
        let texts: Vec<String> = chunks.iter()
            .map(format_for_embedding)
            .collect();

        // Process in batches
        for (batch_idx, batch) in texts.chunks(self.batch_size).enumerate() {
            let embeddings = self.generator.embed_batch(batch.to_vec())?;

            for (idx, embedding) in embeddings.into_iter().enumerate() {
                let chunk_idx = batch_idx * self.batch_size + idx;
                results.push((chunks[chunk_idx].id, embedding));
            }

            progress(batch_idx * self.batch_size, total);
        }

        Ok(results)
    }
}
```

**Optimizations:**
```rust
// Parallel processing with Rayon
use rayon::prelude::*;

pub fn process_files_parallel(
    files: Vec<PathBuf>,
    generator: Arc<EmbeddingGenerator>,
) -> Result<Vec<(ChunkId, Vec<f32>)>> {
    files.par_iter()
        .flat_map(|file| {
            let chunks = chunk_file(file)?;
            generator.embed_batch(chunks)
        })
        .collect()
}
```

**References:**
- fastembed-rs docs: https://docs.rs/fastembed/
- fastembed-rs examples: https://github.com/Anush008/fastembed-rs/tree/main/examples
- Model info: sentence-transformers/all-MiniLM-L6-v2 (80MB, 384d)

#### Deliverables (Phase 4)
- ✅ fastembed-rs integrated
- ✅ Batch embedding working
- ✅ Progress reporting
- ✅ Tested on 1000+ chunks

---

### **Phase 5: Qdrant Integration** (Week 9)

#### Goals
- Set up Qdrant collection
- Index embeddings with metadata
- Implement vector search
- Test retrieval quality

#### Tasks
- [ ] Add qdrant-client dependency
- [ ] Create collection with optimal config
- [ ] Implement upserting chunks with embeddings
- [ ] Implement vector search
- [ ] Test semantic search quality

**Code to write:**
```rust
// src/vector_db/mod.rs
use qdrant_client::{
    client::QdrantClient,
    qdrant::{
        CreateCollection, VectorParams, VectorsConfig, Distance,
        PointStruct, SearchPoints, Filter, Condition,
    },
};

pub struct VectorDB {
    client: QdrantClient,
    collection_name: String,
}

impl VectorDB {
    pub async fn new(url: &str, collection_name: &str) -> Result<Self> {
        let client = QdrantClient::from_url(url).build()?;

        let mut db = Self {
            client,
            collection_name: collection_name.to_string(),
        };

        db.create_collection_if_not_exists().await?;
        Ok(db)
    }

    async fn create_collection_if_not_exists(&self) -> Result<()> {
        // Check if collection exists
        let collections = self.client.list_collections().await?;
        if collections.collections.iter().any(|c| c.name == self.collection_name) {
            return Ok(());
        }

        // Create with optimal config for code search
        let config = CreateCollection {
            collection_name: self.collection_name.clone(),
            vectors_config: Some(VectorsConfig {
                config: Some(qdrant_client::qdrant::vectors_config::Config::Params(
                    VectorParams {
                        size: 384, // all-MiniLM-L6-v2
                        distance: Distance::Cosine.into(),
                        ..Default::default()
                    }
                )),
            }),
            // Optimizations for scale
            optimizers_config: Some(qdrant_client::qdrant::OptimizersConfigDiff {
                indexing_threshold: Some(10000),
                memmap_threshold: Some(50000), // Memory-map after 50k vectors
                ..Default::default()
            }),
            hnsw_config: Some(qdrant_client::qdrant::HnswConfigDiff {
                m: Some(16),               // Connections per node
                ef_construct: Some(100),   // Search depth during construction
                payload_m: Some(8),
                ..Default::default()
            }),
            ..Default::default()
        };

        self.client.create_collection(&config).await?;
        Ok(())
    }

    pub async fn upsert_chunks(
        &self,
        chunks: Vec<(ChunkId, Vec<f32>, CodeChunk)>,
    ) -> Result<()> {
        let points: Vec<PointStruct> = chunks.into_iter()
            .map(|(id, vector, chunk)| PointStruct {
                id: Some(id.as_u64().into()),
                vectors: Some(vector.into()),
                payload: serde_json::to_value(&chunk).unwrap().as_object().cloned().unwrap(),
            })
            .collect();

        self.client.upsert_points(&self.collection_name, points, None).await?;
        Ok(())
    }

    pub async fn search(
        &self,
        query_vector: Vec<f32>,
        limit: usize,
        filter: Option<Filter>,
    ) -> Result<Vec<SearchResult>> {
        let search_request = SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            limit: limit as u64,
            with_payload: Some(true.into()),
            filter,
            ..Default::default()
        };

        let response = self.client.search_points(&search_request).await?;

        let results = response.result.into_iter()
            .map(|point| SearchResult {
                chunk_id: ChunkId::from_u64(point.id.unwrap().try_into().unwrap()),
                score: point.score,
                payload: serde_json::from_value(point.payload.into()).unwrap(),
            })
            .collect();

        Ok(results)
    }

    pub async fn delete_chunks(&self, ids: Vec<ChunkId>) -> Result<()> {
        let point_ids: Vec<_> = ids.into_iter()
            .map(|id| id.as_u64().into())
            .collect();

        self.client.delete_points(&self.collection_name, &point_ids.into(), None).await?;
        Ok(())
    }
}

pub struct SearchResult {
    pub chunk_id: ChunkId,
    pub score: f32,
    pub payload: CodeChunk,
}
```

**Extract from bloop:**
- Qdrant setup: `server/bleep/src/semantic/embedder.rs`
- Collection config: `server/bleep/src/semantic/mod.rs`

**References:**
- Qdrant Rust client: https://github.com/qdrant/rust-client
- Qdrant code search tutorial: https://qdrant.tech/documentation/advanced-tutorials/code-search/
- demo-code-search: https://github.com/qdrant/demo-code-search

#### Deliverables (Phase 5)
- ✅ Qdrant collection created
- ✅ Chunks indexed with embeddings
- ✅ Vector search working
- ✅ Semantic search quality tested

---

### **Phase 6: Hybrid Search** (Week 10-11)

#### Goals
- Combine Tantivy (BM25) and Qdrant (vector) results
- Implement Reciprocal Rank Fusion (RRF)
- Add re-ranking (optional)
- Create unified search interface

#### Week 10: RRF Implementation
**Tasks:**
- [ ] Implement RRF algorithm
- [ ] Merge results from both search engines
- [ ] Handle duplicate results
- [ ] Test hybrid search quality

**Code to write:**
```rust
// src/search/hybrid.rs
pub struct HybridSearch {
    tantivy_search: TantivySearch,  // From Phase 1
    vector_search: VectorSearch,     // From Phase 5
}

impl HybridSearch {
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // 1. Get results from both engines
        let (bm25_results, vector_results) = tokio::join!(
            self.tantivy_search.search(query, 100),
            self.vector_search.search(query, 100)
        );

        let bm25_results = bm25_results?;
        let vector_results = vector_results?;

        // 2. Apply Reciprocal Rank Fusion
        let merged = self.reciprocal_rank_fusion(bm25_results, vector_results);

        // 3. Return top N
        Ok(merged.into_iter().take(limit).collect())
    }

    fn reciprocal_rank_fusion(
        &self,
        bm25_results: Vec<SearchResult>,
        vector_results: Vec<SearchResult>,
    ) -> Vec<SearchResult> {
        const K: f32 = 60.0; // Standard RRF constant

        let mut scores: HashMap<ChunkId, f32> = HashMap::new();
        let mut chunks: HashMap<ChunkId, SearchResult> = HashMap::new();

        // Score from BM25 results
        for (rank, result) in bm25_results.into_iter().enumerate() {
            let score = 1.0 / (rank as f32 + K);
            *scores.entry(result.chunk_id).or_insert(0.0) += score;
            chunks.insert(result.chunk_id, result);
        }

        // Score from vector results
        for (rank, result) in vector_results.into_iter().enumerate() {
            let score = 1.0 / (rank as f32 + K);
            *scores.entry(result.chunk_id).or_insert(0.0) += score;
            chunks.entry(result.chunk_id).or_insert(result);
        }

        // Sort by combined score
        let mut results: Vec<_> = scores.into_iter()
            .map(|(chunk_id, score)| {
                let mut result = chunks.remove(&chunk_id).unwrap();
                result.score = score;
                result
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results
    }
}

// src/search/vector.rs
pub struct VectorSearch {
    vector_db: VectorDB,
    embedding_generator: EmbeddingGenerator,
}

impl VectorSearch {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Generate embedding for query
        let query_vector = self.embedding_generator.embed(query)?;

        // Search Qdrant
        self.vector_db.search(query_vector, limit, None).await
    }
}
```

**Extract from bloop:**
- RRF implementation: `server/bleep/src/indexes/reader.rs` (search method)
- Hybrid search patterns: `server/bleep/src/webserver/answer.rs`

**References:**
- RRF paper: "Reciprocal Rank Fusion outperforms individual systems" (Cormack et al.)
- Qdrant hybrid search: Built-in RRF support

#### Week 11: Re-ranking (Optional)
**Tasks:**
- [ ] Add cross-encoder for re-ranking top 20 results
- [ ] Implement progressive disclosure
- [ ] Test final retrieval quality
- [ ] Benchmark performance

**Code to write:**
```rust
// src/search/reranker.rs (optional)
use candle_core::{Device, Tensor};

pub struct Reranker {
    model: Option<candle_transformers::models::bert::BertModel>,
}

impl Reranker {
    pub async fn rerank(
        &self,
        query: &str,
        results: Vec<SearchResult>,
    ) -> Result<Vec<SearchResult>> {
        if self.model.is_none() {
            return Ok(results);
        }

        // Cross-encoder scoring
        // Score each (query, result) pair
        // Re-sort by new scores
        // ...

        Ok(results)
    }
}

// src/search/progressive.rs
pub struct ProgressiveResults {
    pub summaries: Vec<ChunkSummary>,  // Return these first
    pub chunk_ids: Vec<ChunkId>,       // LLM can request full content via MCP
}

pub struct ChunkSummary {
    pub file: PathBuf,
    pub symbol: String,
    pub docstring: Option<String>,
    pub score: f32,
    pub snippet: String,  // First 3 lines
}

pub fn create_progressive_results(results: Vec<SearchResult>) -> ProgressiveResults {
    let summaries = results.iter()
        .map(|r| ChunkSummary {
            file: r.payload.context.file_path.clone(),
            symbol: r.payload.context.symbol_name.clone(),
            docstring: r.payload.context.docstring.clone(),
            score: r.score,
            snippet: r.payload.content.lines().take(3).collect::<Vec<_>>().join("\n"),
        })
        .collect();

    let chunk_ids = results.iter().map(|r| r.chunk_id).collect();

    ProgressiveResults { summaries, chunk_ids }
}
```

#### Deliverables (Phase 6)
- ✅ RRF hybrid search working
- ✅ Combined results from both engines
- ✅ Optional re-ranking implemented
- ✅ Progressive disclosure ready

---

### **Phase 7: Enhanced MCP Tools** (Week 12-13)

#### Goals
- Extend MCP server with new tools
- Implement all planned MCP tools
- Add MCP resources
- Test with Claude

#### Week 12: Core Tools
**Tasks:**
- [ ] Extend file-search-mcp's existing tools
- [ ] Add new tools: find_definition, find_references, get_dependencies
- [ ] Add get_call_graph, analyze_complexity
- [ ] Test tool integration

**Code to write:**
```rust
// src/mcp/tools.rs
use rust_mcp_sdk::*;

#[derive(Debug)]
pub enum Tool {
    SearchCode,          // Enhanced from file-search-mcp
    FindDefinition,      // NEW
    FindReferences,      // NEW
    GetDependencies,     // NEW
    GetCallGraph,        // NEW
    AnalyzeComplexity,   // NEW
    GetSimilarCode,      // NEW
}

impl MCPServer {
    pub async fn handle_tool_call(
        &self,
        tool: Tool,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match tool {
            Tool::SearchCode => self.search_code(args).await,
            Tool::FindDefinition => self.find_definition(args).await,
            Tool::FindReferences => self.find_references(args).await,
            Tool::GetDependencies => self.get_dependencies(args).await,
            Tool::GetCallGraph => self.get_call_graph(args).await,
            Tool::AnalyzeComplexity => self.analyze_complexity(args).await,
            Tool::GetSimilarCode => self.get_similar_code(args).await,
        }
    }

    async fn search_code(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let query: String = serde_json::from_value(args["query"].clone())?;
        let limit: usize = args["limit"].as_u64().unwrap_or(20) as usize;

        // Use hybrid search
        let results = self.hybrid_search.search(&query, limit).await?;

        Ok(json!({
            "results": results.iter().map(|r| json!({
                "file": r.payload.context.file_path,
                "symbol": r.payload.context.symbol_name,
                "kind": r.payload.context.symbol_kind.as_str(),
                "score": r.score,
                "snippet": r.payload.content.lines().take(5).collect::<Vec<_>>().join("\n"),
                "line_start": r.payload.context.line_start,
                "line_end": r.payload.context.line_end,
            })).collect::<Vec<_>>()
        }))
    }

    async fn find_definition(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let symbol: String = serde_json::from_value(args["symbol"].clone())?;

        // Search in symbol index
        let definitions = self.symbol_index.find_definitions(&symbol)?;

        Ok(json!({
            "definitions": definitions.iter().map(|def| json!({
                "file": def.file,
                "line": def.line,
                "kind": def.kind,
                "signature": def.signature,
                "docstring": def.docstring,
            })).collect::<Vec<_>>()
        }))
    }

    async fn find_references(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let symbol: String = serde_json::from_value(args["symbol"].clone())?;

        // Use Tantivy to find exact matches
        let references = self.tantivy_search.find_exact(&symbol)?;

        Ok(json!({
            "references": references.iter().map(|ref_| json!({
                "file": ref_.file,
                "line": ref_.line,
                "context": ref_.surrounding_code,
            })).collect::<Vec<_>>()
        }))
    }

    async fn get_dependencies(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let file: PathBuf = serde_json::from_value(args["file"].clone())?;

        // Get from dependency graph
        let deps = self.dependency_graph.get_dependencies(&file)?;
        let dependents = self.dependency_graph.get_dependents(&file)?;

        Ok(json!({
            "dependencies": deps,    // Files this file imports
            "dependents": dependents, // Files that import this file
        }))
    }

    async fn get_call_graph(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let function: String = serde_json::from_value(args["function"].clone())?;

        let callers = self.call_graph.find_callers(&function);
        let callees = self.call_graph.find_callees(&function);

        Ok(json!({
            "function": function,
            "callers": callers,
            "callees": callees,
        }))
    }

    async fn analyze_complexity(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let file: PathBuf = serde_json::from_value(args["file"].clone())?;

        // Use rust-code-analysis for metrics
        let metrics = self.code_analyzer.analyze_file(&file)?;

        Ok(json!({
            "cyclomatic_complexity": metrics.cyclomatic,
            "cognitive_complexity": metrics.cognitive,
            "lines_of_code": metrics.loc,
            "maintainability_index": metrics.mi,
            "num_functions": metrics.num_functions,
        }))
    }

    async fn get_similar_code(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let snippet: String = serde_json::from_value(args["snippet"].clone())?;
        let limit: usize = args["limit"].as_u64().unwrap_or(10) as usize;

        // Generate embedding for snippet
        let vector = self.embedding_generator.embed(&snippet)?;

        // Search for similar vectors
        let results = self.vector_db.search(vector, limit, None).await?;

        Ok(json!({
            "similar": results.iter().map(|r| json!({
                "file": r.payload.context.file_path,
                "symbol": r.payload.context.symbol_name,
                "similarity": r.score,
                "code": r.payload.content,
            })).collect::<Vec<_>>()
        }))
    }
}
```

#### Week 13: MCP Resources
**Tasks:**
- [ ] Implement MCP resources
- [ ] Add resource URIs: rust:///file/ast, rust:///symbol/docs, etc.
- [ ] Test resource fetching
- [ ] Document MCP interface

**Code to write:**
```rust
// src/mcp/resources.rs
impl MCPServer {
    pub async fn handle_resource_read(&self, uri: &str) -> Result<serde_json::Value> {
        // Parse URI: rust:///path/to/file/ast
        if !uri.starts_with("rust:///") {
            return Err(anyhow!("Invalid URI scheme"));
        }

        let path = &uri[8..]; // Remove "rust:///"
        let parts: Vec<&str> = path.split('/').collect();

        match parts.as_slice() {
            [file @ .., "ast"] => {
                let file_path = PathBuf::from(file.join("/"));
                self.get_ast(&file_path).await
            }
            [file @ .., "metrics"] => {
                let file_path = PathBuf::from(file.join("/"));
                self.get_metrics(&file_path).await
            }
            [symbol, "docs"] => {
                self.get_symbol_docs(symbol).await
            }
            [symbol, "references"] => {
                self.get_symbol_references(symbol).await
            }
            _ => Err(anyhow!("Invalid resource URI")),
        }
    }

    async fn get_ast(&self, file: &Path) -> Result<serde_json::Value> {
        let tree = self.parser.parse_file(file)?;
        // Convert tree to JSON
        Ok(serde_json::to_value(&tree)?)
    }

    async fn get_metrics(&self, file: &Path) -> Result<serde_json::Value> {
        let metrics = self.code_analyzer.analyze_file(file)?;
        Ok(serde_json::to_value(&metrics)?)
    }

    async fn get_symbol_docs(&self, symbol: &str) -> Result<serde_json::Value> {
        let symbol_info = self.symbol_index.get(symbol)?;
        Ok(json!({
            "name": symbol_info.name,
            "kind": symbol_info.kind,
            "docstring": symbol_info.docstring,
            "signature": symbol_info.signature,
        }))
    }
}
```

**Reference:**
- file-search-mcp MCP setup: Study existing tool implementation
- rust-mcp-sdk docs: https://docs.rs/rust-mcp-sdk/

#### Deliverables (Phase 7)
- ✅ All MCP tools implemented
- ✅ MCP resources working
- ✅ Tested with Claude
- ✅ Documentation complete

---

### **Phase 8: Optimization & Polish** (Week 14-16)

#### Goals
- Optimize performance
- Add comprehensive tests
- Write documentation
- Prepare for release

#### Week 14: Performance
**Tasks:**
- [ ] Benchmark on large codebases (100k, 500k, 1M LOC)
- [ ] Profile memory usage
- [ ] Optimize hot paths
- [ ] Add performance metrics

**Benchmarks:**
```rust
// benches/indexing.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_full_indexing(c: &mut Criterion) {
    c.bench_function("index 10k LOC", |b| {
        b.iter(|| {
            // Index small Rust project
        });
    });
}

fn bench_incremental_update(c: &mut Criterion) {
    c.bench_function("update single file", |b| {
        b.iter(|| {
            // Update one file
        });
    });
}

fn bench_hybrid_search(c: &mut Criterion) {
    c.bench_function("hybrid search", |b| {
        b.iter(|| {
            // Execute hybrid search
        });
    });
}

criterion_group!(benches, bench_full_indexing, bench_incremental_update, bench_hybrid_search);
criterion_main!(benches);
```

**Target Metrics:**
- Index 100k LOC: <2 minutes
- Index 1M LOC: <5 minutes
- Query latency (p95): <200ms
- Memory usage (1M LOC): <4GB
- Incremental update: <1s per file

#### Week 15: Testing
**Tasks:**
- [ ] Add unit tests for all components
- [ ] Add integration tests
- [ ] Test on real codebases (rustc, tokio, serde)
- [ ] Add CI/CD pipeline

**Tests:**
```rust
// tests/integration/full_pipeline.rs
#[tokio::test]
async fn test_full_indexing_pipeline() {
    // Create test codebase
    let temp_dir = create_test_codebase();

    // Index it
    let indexer = Indexer::new(temp_dir.path()).await?;
    indexer.index_all().await?;

    // Search
    let results = indexer.search("async function").await?;
    assert!(!results.is_empty());

    // Verify chunks stored
    assert_eq!(indexer.chunk_count(), EXPECTED_CHUNKS);
}

#[tokio::test]
async fn test_incremental_updates() {
    // Create index
    let indexer = Indexer::new(test_path()).await?;
    indexer.index_all().await?;

    // Modify file
    modify_file(&test_file);

    // Trigger reindex
    indexer.watch_and_update().await?;

    // Verify updated
    let results = indexer.search("new_function").await?;
    assert!(!results.is_empty());
}

#[tokio::test]
async fn test_mcp_protocol() {
    // Start MCP server
    let server = MCPServer::new().await?;

    // Send search tool call
    let response = server.handle_tool_call(Tool::SearchCode, json!({
        "query": "error handling",
        "limit": 10
    })).await?;

    // Verify response
    assert!(response["results"].as_array().unwrap().len() <= 10);
}
```

#### Week 16: Documentation & Release
**Tasks:**
- [ ] Write comprehensive README
- [ ] Add API documentation
- [ ] Create usage examples
- [ ] Write CONTRIBUTING guide
- [ ] Tag v0.1.0 release

**Documentation:**
```markdown
# README.md

# Rust Codebase MCP - Scalable Code Search for Large Rust Projects

A Model Context Protocol (MCP) server for semantic code search, optimized for large Rust codebases (1M+ LOC).

## Features

- **Hybrid Search**: Combines BM25 (lexical) + vector embeddings (semantic)
- **Scalable**: Handles codebases >1M LOC efficiently
- **Incremental**: Only reindexes changed files
- **Local**: All processing happens locally, no API calls
- **Fast**: Sub-200ms query latency

## Quick Start

```bash
# Install
cargo install rust-code-mcp

# Initialize project
rust-code-mcp init --path /path/to/rust/project

# Index codebase
rust-code-mcp index

# Start MCP server
rust-code-mcp serve
```

## MCP Integration

```json
// Claude Desktop config (~/.config/claude/config.json)
{
  "mcpServers": {
    "rust-codebase": {
      "command": "rust-code-mcp",
      "args": ["serve"]
    }
  }
}
```

## Architecture

[Include architecture diagram]

## Performance

- Index 100k LOC: ~1 minute
- Index 1M LOC: ~5 minutes
- Query latency: <200ms (p95)
- Memory usage: <4GB (1M LOC)

## License

MIT OR Apache-2.0
```

#### Deliverables (Phase 8)
- ✅ Performance benchmarks complete
- ✅ Comprehensive test coverage (>80%)
- ✅ Documentation complete
- ✅ v0.1.0 released

---

## Project Structure

```
rust-code-mcp/
├── Cargo.toml
├── README.md
├── LICENSE-MIT
├── LICENSE-APACHE
├── .github/
│   └── workflows/
│       ├── ci.yml
│       └── release.yml
│
├── src/
│   ├── main.rs                 # CLI entry point
│   ├── lib.rs
│   │
│   ├── mcp/                    # MCP server (from file-search-mcp + enhancements)
│   │   ├── mod.rs
│   │   ├── server.rs
│   │   ├── tools.rs            # MCP tool implementations
│   │   └── resources.rs        # MCP resource handlers
│   │
│   ├── storage/                # Enhanced from file-search-mcp
│   │   ├── mod.rs
│   │   ├── persistent.rs       # Persistent Tantivy index
│   │   └── metadata.rs         # File metadata cache
│   │
│   ├── watcher/                # NEW - Week 3
│   │   ├── mod.rs
│   │   └── file_watcher.rs     # notify integration
│   │
│   ├── parser/                 # NEW - Week 5-6
│   │   ├── mod.rs
│   │   ├── rust_parser.rs      # tree-sitter integration
│   │   ├── call_graph.rs
│   │   └── imports.rs
│   │
│   ├── chunker/                # NEW - Week 7
│   │   ├── mod.rs
│   │   └── code_chunker.rs     # text-splitter integration
│   │
│   ├── embeddings/             # NEW - Week 8
│   │   ├── mod.rs
│   │   ├── generator.rs        # fastembed integration
│   │   └── pipeline.rs         # Batch processing
│   │
│   ├── vector_db/              # NEW - Week 9
│   │   ├── mod.rs
│   │   └── qdrant.rs           # Qdrant client
│   │
│   ├── search/                 # Enhanced from file-search-mcp + NEW
│   │   ├── mod.rs
│   │   ├── tantivy_search.rs   # From file-search-mcp
│   │   ├── vector_search.rs    # NEW
│   │   ├── hybrid.rs           # NEW - RRF
│   │   └── reranker.rs         # NEW - Optional
│   │
│   ├── schema.rs               # Enhanced Tantivy schema
│   ├── config.rs
│   └── cli.rs
│
├── tests/
│   ├── integration/
│   │   ├── indexing.rs
│   │   ├── searching.rs
│   │   └── mcp.rs
│   └── fixtures/
│
├── benches/
│   ├── indexing.rs
│   └── searching.rs
│
└── examples/
    ├── basic_usage.rs
    └── mcp_server.rs
```

---

## Code Extraction Guide

### From file-search-mcp

**Keep as-is:**
- MCP server setup (`src/main.rs`)
- Tantivy indexing logic (`src/search.rs`)
- File traversal (`src/indexer.rs`)

**Enhance:**
- Make Tantivy index persistent
- Add incremental updates
- Improve schema for code

### From bloop (`server/bleep/src/`)

**Extract patterns from:**

1. **Qdrant integration**: `semantic/embedder.rs`, `semantic/mod.rs`
   - Collection setup
   - Batch embedding
   - Search patterns

2. **Tree-sitter parsing**: `intelligence/code_navigation/language_parsers.rs`
   - Symbol extraction
   - AST traversal
   - Call graph building

3. **Change detection**: `background.rs`
   - File watching
   - Debouncing
   - Reindexing logic

4. **Hybrid search**: `indexes/reader.rs`
   - RRF implementation
   - Result merging

**Don't extract:**
- Desktop app code (`apps/desktop/`)
- Frontend code (`client/`)
- Server/API code (`webserver/`) - we use MCP instead

---

## Dependencies Overview

### Core (Must-Have)

```toml
# From file-search-mcp
rust-mcp-sdk = "0.1"
tantivy = "0.22"

# NEW additions
qdrant-client = "1.9"          # Vector search
fastembed = "3.4"              # Embeddings
tree-sitter = "0.20"           # Parsing
tree-sitter-rust = "0.20"
text-splitter = "0.13"         # Code chunking
ignore = "0.4"                 # File traversal
notify = "6.1"                 # Change detection
rayon = "1.10"                 # Parallelism
tokio = { version = "1.37", features = ["full"] }
```

### Utilities

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
thiserror = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
clap = { version = "4.5", features = ["derive"] }
toml = "0.8"
```

### Optional (Advanced)

```toml
# For Salsa (Phase 2)
salsa = "0.17"

# For Merkle trees
rs_merkle = "1.4"

# For cross-encoder re-ranking
candle-core = "0.6"
hf-hub = "0.3"

# For code metrics
rust-code-analysis = "0.0.25"
```

---

## Risk Assessment

### ✅ Low Risk (Proven Solutions)

- **MCP server**: file-search-mcp provides working base
- **Tantivy**: Already integrated in file-search-mcp
- **Qdrant**: Mature library, used by bloop
- **fastembed-rs**: Production-ready, ONNX-based
- **tree-sitter**: Widely used, stable
- **text-splitter**: Built for this use case

### ⚠️ Medium Risk (Integration Complexity)

- **Hybrid search tuning**: May need experimentation with K value
- **Performance at scale**: Need to test on very large repos (1M+ LOC)
- **Incremental indexing edge cases**: File deletions, renames, moves

### 🔴 Higher Risk (Defer to Phase 2)

- **Salsa integration**: Complex, may not be needed
- **Custom embeddings**: Standard models work fine
- **Complex dataflow analysis**: Tree-sitter + simple call graph sufficient

### Mitigation

1. Start with file-search-mcp (working base)
2. Extract proven patterns from bloop
3. Use battle-tested libraries
4. Test early and often on real codebases
5. Defer complex features to v0.2

---

## Success Metrics

### MVP (Week 10)
- [ ] Index 100k LOC in <2 min
- [ ] Hybrid search working
- [ ] Query latency <500ms p95
- [ ] Basic MCP tools functional
- [ ] Tested with Claude

### Production (Week 16)
- [ ] Index 1M LOC in <5 min
- [ ] Query latency <200ms p95
- [ ] Memory usage <4GB for 1M LOC
- [ ] Retrieval accuracy >80%
- [ ] All MCP tools + resources working
- [ ] Comprehensive documentation
- [ ] v0.1.0 released

### Community (6 months post-release)
- [ ] 100+ GitHub stars
- [ ] 10+ contributors
- [ ] Used by 5+ teams
- [ ] Featured in Rust newsletter

---

## Post-MVP Roadmap

### v0.2 (Month 5-6)
- [ ] Multi-language support (use other tree-sitter grammars)
- [ ] Salsa for advanced incremental computation
- [ ] Better code metrics (rust-code-analysis)
- [ ] Web UI for exploration

### v0.3 (Month 7-9)
- [ ] Distributed indexing
- [ ] Custom fine-tuned embeddings
- [ ] GNN-based re-ranking
- [ ] Cross-crate analysis

### v1.0 (Month 10-12)
- [ ] Production-hardened
- [ ] Enterprise features
- [ ] Performance optimizations
- [ ] Commercial support

---

## Getting Started Checklist

### This Week

- [ ] Fork file-search-mcp: `git clone https://github.com/Kurogoma4D/file-search-mcp.git rust-code-mcp`
- [ ] Clone bloop for reference: `git clone https://github.com/BloopAI/bloop.git`
- [ ] Set up Qdrant: `docker run -d -p 6333:6333 qdrant/qdrant`
- [ ] Read file-search-mcp codebase
- [ ] Document architecture
- [ ] Create extraction plan from bloop

### Next Week

- [ ] Make Tantivy index persistent
- [ ] Add file metadata caching
- [ ] Test on real Rust projects

### Week 3

- [ ] Integrate notify for change detection
- [ ] Implement incremental updates
- [ ] Test rapid file changes

---

## Resources

### Forked/Reference Projects
- **file-search-mcp**: https://github.com/Kurogoma4D/file-search-mcp
- **bloop**: https://github.com/BloopAI/bloop (archived, for reference)
- **demo-code-search**: https://github.com/qdrant/demo-code-search
- **rust-mcp-filesystem**: https://github.com/rust-mcp-stack/rust-mcp-filesystem

### Documentation
- **Qdrant**: https://qdrant.tech/documentation/
- **Tantivy**: https://docs.rs/tantivy/
- **fastembed-rs**: https://docs.rs/fastembed/
- **text-splitter**: https://docs.rs/text-splitter/
- **tree-sitter**: https://tree-sitter.github.io/
- **rust-mcp-sdk**: https://docs.rs/rust-mcp-sdk/

### Tutorials
- **Qdrant code search**: https://qdrant.tech/documentation/advanced-tutorials/code-search/
- **Bloop case study**: https://qdrant.tech/blog/case-study-bloop/
- **Tantivy examples**: https://github.com/quickwit-oss/tantivy/tree/main/examples

### Research
- Full research report: `RUST_MCP_CODE_SEARCH_RESEARCH.md`
- Original plan: `PLAN.md`
- State-of-the-art analysis: `STATE_OF_THE_ART_CODEBASE_ANALYSIS.md`

---

## Comparison: Original vs New Plan

| Aspect | Original Plan | New Plan | Improvement |
|--------|---------------|----------|-------------|
| **Timeline** | 40-54 weeks | 16 weeks | **62-70% faster** |
| **MCP Server** | Build from scratch | Fork file-search-mcp | 8-12 weeks saved |
| **Tantivy** | Implement integration | Already done | 4-6 weeks saved |
| **Tree-sitter** | Build from scratch | Extract from bloop | 2-3 weeks saved |
| **Qdrant** | Build from scratch | Extract from bloop | 3-4 weeks saved |
| **Chunking** | Custom implementation | Use text-splitter | 3-4 weeks saved |
| **Embeddings** | Build pipeline | Use fastembed-rs | 4-6 weeks saved |
| **Risk Level** | Medium-High | Low-Medium | Better foundation |
| **Code Quality** | Unknown | Proven in production | Higher confidence |

**Total time saved: 24-38 weeks (6-9 months)**

---

## Conclusion

This new plan leverages maximum code reuse by:

1. **Forking file-search-mcp** (50% of functionality)
2. **Extracting from bloop** (proven patterns)
3. **Using mature libraries** (all core components)

**Result**: Reduce development time from ~1 year to 4 months while building on production-tested foundations.

**Next Step**: Fork file-search-mcp and begin Week 1 setup.

---

**Created:** 2025-10-17
**Status:** Ready to Implement
**Confidence Level:** High (based on comprehensive research)
