# tools — Detailed Logic

## Module: mod.rs

Pure module declaration file: `pub mod clear_cache_tool`, `pub mod health_tool`, `pub mod index_tool`, `pub mod project_paths`, `pub mod search_tool`, `pub mod indexing_tools`, `pub mod query_tools`, `pub mod analysis_tools`, `pub mod search_tool_router`, `pub mod graph_tools`. No functions or impl blocks.

## Module: project_paths

### `pub struct ProjectPaths`
Fields: `dir_hash: String`, `cache_path: PathBuf`, `tantivy_path: PathBuf`, `collection_name: String`, `vector_path: PathBuf` — bundled per-project derived paths used by the BM25 index, metadata cache, and vector store.

### `ProjectPaths::from_directory(dir: &Path) -> Self`
**Call graph:** Sha256::new -> Sha256::update -> Sha256::finalize -> indexing_tools::data_dir -> PathBuf::join
**Steps:**
1. Create a SHA-256 hasher and feed it `dir.to_string_lossy()` bytes.
2. Format the digest as a hex string and store as `dir_hash`.
3. Read the XDG-compliant base data directory via `data_dir()`.
4. Build `collection_name = "code_chunks_<first 8 hex chars>"`.
5. Compose `cache_path = base/cache/<dir_hash>`, `tantivy_path = base/index/<dir_hash>`, `vector_path = base/cache/vectors/<collection_name>`.
6. Return the populated `ProjectPaths`.

## Module: indexing_tools

### `pub fn data_dir() -> PathBuf`
**Call graph:** ProjectDirs::from -> ProjectDirs::data_dir -> PathBuf::from
**Steps:**
1. Try to acquire an XDG-compliant `ProjectDirs` for ("dev", "rust-code-mcp", "search").
2. On success, return the project's data directory.
3. On failure, fall back to a relative `.rust-code-mcp` directory.

### `pub fn open_or_create_index() -> Result<(Index, FileSchema), String>`
**Call graph:** FileSchema::new -> data_dir -> std::fs::create_dir_all -> Index::open_in_dir | Index::create_in_dir
**Steps:**
1. Construct a fresh `FileSchema`.
2. Compute `index_path = data_dir().join("index")` and ensure the directory exists.
3. If `meta.json` exists in `index_path`, open the existing Tantivy `Index`.
4. Otherwise, create a new `Index` using the schema.
5. Return `(index, schema)`, mapping errors to `String`.

### `pub fn open_cache() -> Result<MetadataCache, String>`
**Call graph:** data_dir -> MetadataCache::new
**Steps:**
1. Compute `cache_path = data_dir().join("cache")`.
2. Construct a `MetadataCache` rooted at that path, mapping errors to `String`.

## Module: health_tool

### `pub struct HealthCheckParams`
Field: `directory: Option<String>` — optional project directory to scope the health check.

### Private `fn data_dir() -> PathBuf`
Same XDG-fallback logic as `indexing_tools::data_dir`; returns the `dev/rust-code-mcp/search` data directory or `.rust-code-mcp`.

### `pub async fn health_check(Parameters(HealthCheckParams)) -> Result<CallToolResult, McpError>`
**MCP tool — `health_check`**: Returns BM25 / vector store / Merkle tree status as JSON plus a human-readable interpretation.
**Call graph:** Sha256::new/update/finalize -> data_dir -> get_snapshot_path -> Bm25Search::new -> VectorStore::new_embedded -> HealthMonitor::new -> HealthMonitor::check_health -> serde_json::to_string_pretty
**Steps:**
1. Log the health-check intent.
2. If `directory` is provided, hash it with SHA-256 and derive `bm25_path = data_dir/index/<hash>`, `merkle_path = get_snapshot_path(dir)`, `collection_name = "code_chunks_<first 8 hex>"`.
3. Otherwise, fall back to a system-wide check using `data_dir/index`, a sentinel non-existent merkle path, and the default collection name.
4. Try to open the Tantivy index via `Bm25Search::new(&bm25_path)` (wrapped in `Arc` if successful).
5. Try to open the LanceDB vector store at `data_dir/cache/vectors/<collection_name>` with dimension 384.
6. Construct a `HealthMonitor` and run `check_health().await`.
7. Pretty-serialize the report as JSON.
8. Append a status banner (`Healthy`/`Degraded`/`Unhealthy`), the JSON, an explanatory guide, and the directory context.
9. Return the response as `CallToolResult::success`.

## Module: clear_cache_tool

### `pub struct ClearCacheParams`
Field: `directory: Option<String>` — optional project directory to scope the clear; absent means clear all caches.

### Private `fn data_dir() -> PathBuf`
Same XDG-fallback logic as `indexing_tools::data_dir`.

### Private `fn compute_dir_hash(dir_path: &Path) -> String`
**Call graph:** Sha256::new -> Sha256::update -> Sha256::finalize
**Steps:**
1. Hash `dir_path.to_string_lossy()` bytes with SHA-256.
2. Return the digest as a 64-char hex string.

### `pub async fn clear_cache(params: ClearCacheParams) -> Result<CallToolResult, McpError>`
**MCP tool — `clear_cache`**: Removes the metadata cache, Tantivy index, and vector-store directories for one or all projects.
**Call graph:** data_dir -> compute_dir_hash -> std::fs::remove_dir_all -> PathBuf::join
**Steps:**
1. Initialize empty `cleared` and `errors` vectors.
2. Compute the base `data_dir`.
3. If a directory is supplied, hash it and derive `cache_path`, `tantivy_path`, and `vector_path` (with `collection_name`).
4. For each derived path that exists, attempt `remove_dir_all`, recording success or failure.
5. Otherwise, recursively remove the global `cache` and `index` subdirectories under `data_dir`.
6. Build a response string listing cleared paths and errors (or "No cache files found" when both lists empty).
7. Append a "will be re-indexed" hint scoped to the project or workspace.
8. Return `CallToolResult::success` with the response text.

## Module: index_tool

### `pub struct IndexCodebaseParams`
Fields: `directory: String` (absolute path), `force_reindex: Option<bool>` (default false).

### `pub async fn index_codebase(params: IndexCodebaseParams, sync_manager: Option<&Arc<SyncManager>>) -> Result<CallToolResult, McpError>`
**MCP tool — `index_codebase`**: Performs incremental indexing of a Rust codebase, optionally forcing a full reindex.
**Call graph:** PathBuf::from -> Path::exists -> Path::is_dir -> ProjectPaths::from_directory -> get_snapshot_path -> std::fs::remove_file -> IncrementalIndexer::new -> IncrementalIndexer::clear_all_data -> IncrementalIndexer::index_with_change_detection -> SyncManager::track_directory
**Steps:**
1. Convert `directory` to `PathBuf` and read `force_reindex` (default false).
2. Validate that the directory exists and is a directory; otherwise return `invalid_params`.
3. Log the operation and compute `ProjectPaths` for the workspace.
4. If `force` is set, delete the Merkle snapshot at `get_snapshot_path(&dir)` if present.
5. Construct an `IncrementalIndexer` with embedded LanceDB backend (cache, tantivy, collection, dim, no extra config).
6. If `force` is set, call `clear_all_data()` on the indexer.
7. Time `indexer.index_with_change_detection(&dir).await` and capture `IndexStats`.
8. If a sync manager is provided and any files were indexed/unchanged, call `sync_mgr.track_directory(dir.clone()).await`.
9. Format a result string covering one of three branches: no Rust files, no changes, or new changes indexed (each branch reports indexed/unchanged/skipped/chunks/elapsed/sync state/collection).
10. Return `CallToolResult::success(Content::text(result_text))`.

## Module: search_tool

This module is a backward-compatibility wrapper. It re-exports `search_tool_router::SearchToolRouter` as `SearchTool` and defines the parameter structs consumed by the router methods. No `pub fn` items — only data definitions.

### Re-export
- `pub use crate::tools::search_tool_router::SearchToolRouter as SearchTool`.

### Parameter Structs (each derives `Debug`, `serde::Deserialize`, `schemars::JsonSchema`)

- `SearchParams { directory, keyword }` — hybrid search input.
- `FileContentParams { file_path }` — file read input.
- `FindDefinitionParams { symbol_name, directory }` — definition lookup.
- `FindReferencesParams { symbol_name, directory }` — reference lookup.
- `GetDependenciesParams { file_path }` — file imports lookup.
- `GetCallGraphParams { file_path, symbol_name: Option<String> }` — call-graph query.
- `AnalyzeComplexityParams { file_path }` — complexity-metric input.
- `GetSimilarCodeParams { query, directory, limit: Option<usize> }` — semantic similarity.
- `BuildHypergraphParams { directory, force_rebuild: Option<bool> }` — hypergraph build.
- `GraphImportsParams { directory, module }` — module imports.
- `GraphExportsParams { directory, module, consumer }` — module exports.
- `GraphReexportsParams { directory, module, consumer }` — `pub use` subset.
- `GraphDeclaredReexportsParams { directory, module }` — explicit `pub use` declarations.
- `WhoImportsParams { directory, target }` — reverse importer lookup.
- `WhoUsesParams { directory, target }` — non-import reference lookup.
- `WhoUsesSummaryParams { directory, target }` — usage rollup.
- `WhoCallsParams { directory, target }` — fn-body callers.
- `CallsFromParams { directory, caller }` — fn-body callees.
- `CallGraphParams { directory, root, depth: Option<u32> }` — bounded recursive call graph.
- `CallersInCrateParams { directory, target, krate }` — `who_calls` filtered by caller crate.
- `RecursiveCallersCountParams { directory, target, depth: Option<u32> }` — reverse-BFS count.
- `DeadPubParams { directory, krate }` — single-crate dead-pub scan.
- `DeadPubReportParams { directory }` — workspace-wide dead-pub report.
- `CrateEdgesParams { directory }` — cross-crate edges.
- `OverlapsParams { directory }` — name-collision report.
- `ForbiddenDependencyRuleParam { consumer, producer, except, severity, message }` — single rule input for forbidden-dep check.
- `ForbiddenDependencyCheckParams { directory, rules }` — collection of rules.
- `EnumVariantsParams { directory, target }` — enum-variant enumeration.
- `ItemAttributesParams { directory, target }` — item attribute inspection.
- `ItemsWithAttributeParams { directory, crate_name, attribute_pattern }` — attribute search.
- `PubUsePubTypeAuditParams { directory, crate_name }` — alias-vs-reexport audit.
- `ReExportChainParams { directory, target }` — re-export chain walk.
- `CrateDependencyMetricParams { directory, top_n: Option<usize>, sort_by: Option<String> }` — Robert Martin metrics.
- `ModuleTreeParams { directory, krate, depth: Option<usize> }` — module/item tree dump.
- `WorkspaceStatsParams { directory }` — workspace counters.
- `FunctionSignatureParams { directory, target }` — single-fn signature lookup.
- `UnsafeAuditParams { directory }` — unsafe-block audit.
- `MutStaticAuditParams { directory }` — global-mutable-state audit.
- `MissingDocsAuditParams { directory, crate_name, item_kind, skip_test_items }` — missing-docs audit.
- `DeriveAuditParams { directory, crate_name, item_kind, required_derives, pub_only, skip_test_items }` — required-derive audit.
- `RecursionCheckParams { directory, crate_name, max_cycle_length }` — call-graph recursion audit.
- `ChannelCapacityAuditParams { directory, crate_name, skip_test_fns }` — channel-construction audit.
- `FnBodyAuditParams { directory, crate_name, patterns, skip_test_fns }` — fn-body pattern audit.
- `FunctionsWithFilterParams { directory, krate, min_param_count, has_param_type, returns_type_pattern, is_async, self_kind, limit, offset, summary }` — paginated function filter.
- `SimilarToItemParams { directory, target, limit, threshold, item_kind }` — semantic neighbors of one item.
- `SemanticOverlapsParams { directory, crate_name, item_kind, threshold, max_pairs, max_cluster_size, output_mode, skip_test_chunks, cross_crate_only }` — workspace-wide overlap audit.

## Module: query_tools

### `pub async fn read_file_content(file_path: &str) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `read_file_content`**: Returns a file's UTF-8 text content; rejects binaries.
**Call graph:** Path::new -> Path::exists -> Path::is_file -> tokio::fs::read_to_string -> tokio::fs::read
**Steps:**
1. Validate the path exists and is a regular file; return `invalid_params` on failure.
2. Try `fs::read_to_string`; on success, return the content (or "File is empty." for zero-length).
3. On UTF-8 failure, fall back to `fs::read` to inspect raw bytes.
4. If null bytes appear or >10% of bytes are non-printable controls, classify as binary and return `invalid_params`.
5. Otherwise, return the original UTF-8 error wrapped in `invalid_params`.

### Private `fn try_open_bm25(paths: &ProjectPaths) -> Option<Bm25Search>`
**Call graph:** TantivyConfig::default -> TantivyAdapter::new -> TantivyAdapter::create_bm25_search
**Steps:**
1. Build a default `TantivyConfig` rooted at `paths.tantivy_path`.
2. Create a `TantivyAdapter` and chain `create_bm25_search()`.
3. Convert any `Result::Err` to `None` so callers can detect missing/corrupt indexes.

### Private `fn clean_stale_index(paths: &ProjectPaths, dir: &Path)`
**Call graph:** get_snapshot_path -> std::fs::remove_dir_all -> std::fs::remove_file
**Steps:**
1. Remove the corrupt Tantivy index directory if it exists.
2. Remove the Merkle snapshot for `dir` so the next pass is full.
3. Remove the metadata cache directory so files are re-processed.

### Private `async fn ensure_indexed(dir_path, paths, sync_manager) -> Result<IndexStats, McpError>`
**Call graph:** UnifiedIndexer::for_embedded -> UnifiedIndexer::index_directory -> SyncManager::track_directory
**Steps:**
1. Initialize a `UnifiedIndexer` with the embedded LanceDB backend, mapping any error to `invalid_params`.
2. Run `index_directory(dir_path).await` to produce `IndexStats`.
3. Log file/chunk counters.
4. If a sync manager is provided and any files were indexed/unchanged, call `track_directory`.
5. Return the stats.

### `pub(crate) async fn create_hybrid_search(paths, bm25_search) -> Result<HybridSearch, McpError>`
**Call graph:** EmbeddingGenerator::new -> VectorStore::new_embedded -> HybridSearch::with_defaults
**Steps:**
1. Construct an `EmbeddingGenerator`; map errors to `invalid_params`.
2. Open the embedded vector store at `paths.vector_path` with `EMBEDDING_DIM`.
3. Build a `HybridSearch::with_defaults` from the generator, vector store, and optional `Bm25Search`.

### Private `fn format_results(results, keyword, stats, rebuilt) -> String`
**Steps:**
1. If results is empty, return a short "No results" message that optionally includes index stats.
2. Otherwise, prefix with a "rebuilt" notice if the index was just regenerated, then the result count and keyword.
3. For each result, append index, score, file, symbol name/kind, line range, optional doc, and a 3-line preview.
4. Append final indexing-stats line if stats are available.

### `pub async fn search(directory: &str, keyword: &str, sync_manager: Option<&Arc<SyncManager>>) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `search`**: Hybrid BM25 + vector search, rebuilding the index if stale or corrupt.
**Call graph:** Path::new -> Path::is_dir -> ProjectPaths::from_directory -> try_open_bm25 -> SyncManager::track_directory -> clean_stale_index -> ensure_indexed -> create_hybrid_search -> HybridSearch::search -> format_results
**Steps:**
1. Validate the directory and reject empty keywords.
2. Compute `ProjectPaths` for the workspace.
3. Try to open BM25; if it succeeds, optionally track the directory in the sync manager and skip indexing.
4. If BM25 cannot be opened, set `rebuilt = true` if a stale tantivy path exists, clean stale indexes, run `ensure_indexed`, then re-open BM25.
5. If first-time indexing produced no chunks AND no unchanged files, return early with a "no Rust files found" message.
6. Build a `HybridSearch` and run `search(keyword, 10).await`.
7. Format results via `format_results` and return them.

### `pub async fn get_similar_code(query: &str, directory: &str, limit: usize) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `get_similar_code`**: Vector-only semantic similarity search.
**Call graph:** Path::new -> Path::is_dir -> ProjectPaths::from_directory -> create_hybrid_search -> HybridSearch::vector_only_search
**Steps:**
1. Validate the directory.
2. Compute `ProjectPaths`.
3. Build a `HybridSearch` (no BM25).
4. Call `vector_only_search(query, limit)`.
5. If empty, return a "no similar code" message.
6. Otherwise, format each result with score, file, symbol name/kind, line range, optional doc, and a 3-line preview.
7. Return as `CallToolResult::success`.

## Module: analysis_tools

### `pub async fn find_definition(symbol_name: &str, directory: &str) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `find_definition`**: Locates symbol definitions via the global semantic index.
**Call graph:** Path::new -> SEMANTIC.lock -> SemanticIndex::symbol_search -> Location::to_string
**Steps:**
1. Lock the global `SEMANTIC` index, mapping poisoned-lock to `internal_error`.
2. Call `symbol_search(project_path, symbol_name, 50)`.
3. If no locations, return a "No definition found" message.
4. Otherwise, join each location's `to_string` with newlines and return a "Found N definition(s)" message.

### `pub async fn find_references(symbol_name: &str, directory: &str) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `find_references`**: Lists all references to a symbol.
**Call graph:** Path::new -> SEMANTIC.lock -> SemanticIndex::find_references_by_name -> Location::to_string
**Steps:**
1. Lock the global `SEMANTIC` index.
2. Call `find_references_by_name(project_path, symbol_name)`.
3. Return formatted output identical in shape to `find_definition`.

### `pub async fn get_dependencies(file_path: &str) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `get_dependencies`**: Lists imports parsed from a single file.
**Call graph:** Path::new -> Path::exists -> Path::is_file -> RustParser::new -> RustParser::parse_file_complete
**Steps:**
1. Validate the file path exists and is a file.
2. Build a `RustParser` and run `parse_file_complete`.
3. If no imports found, return "No imports found".
4. Otherwise, list each import path with bullet markers prefixed by `Imports (N):`.

### `pub async fn get_call_graph(file_path: &str, symbol_name: Option<&str>) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `get_call_graph`**: Returns either a per-symbol callees/callers list or the file-wide call graph.
**Call graph:** Path::new/exists/is_file -> RustParser::new -> RustParser::parse_file_complete -> CallGraph::get_callees -> CallGraph::get_callers -> CallGraph::all_functions -> CallGraph::edge_count
**Steps:**
1. Validate the file path exists and is a file.
2. Parse the file via `RustParser::parse_file_complete`.
3. If a `symbol_name` is supplied, fetch its callees and callers; print arrows (`→` callees, `←` callers) with counts; print "No call relationships found" when both empty.
4. Otherwise, list every function with its callees as `name → [a, b, c]`, prefaced by counts of functions and edges; print "No function calls found" when the graph has zero edges.

### `pub async fn analyze_complexity(file_path: &str) -> Result<CallToolResult, McpError>`
**MCP tool delegate — `analyze_complexity`**: Computes LOC, comment, symbol, cyclomatic, and call-graph metrics for one file.
**Call graph:** Path::new/exists/is_file -> std::fs::read_to_string -> RustParser::new -> RustParser::parse_file_complete -> CallGraph::edge_count
**Steps:**
1. Validate the file path.
2. Read the source via `fs::read_to_string`.
3. Parse symbols via `RustParser::parse_file_complete`.
4. Count total lines, non-empty lines, comment lines (`// ...`), function/struct/trait counts (filtering `parse_result.symbols`).
5. Sum cyclomatic decision points across all lines using keywords `if`, `else if`, `while`, `for`, `match`, `&&`, `||`.
6. Compute average complexity per function (zero when no functions).
7. Format a multi-line report with code metrics, symbol counts, complexity, and call-graph edge count.

## Module: search_tool_router

### `pub struct SearchToolRouter`
Fields: `tool_router: ToolRouter<Self>`, `sync_manager: Option<Arc<SyncManager>>`. Macro `#[tool_router]` registers all annotated methods with the rmcp tool registry.

### `SearchToolRouter::new() -> Self`
**Steps:**
1. Build a `ToolRouter` via `Self::tool_router()` (generated by macro).
2. Set `sync_manager = None`.

### `SearchToolRouter::with_sync_manager(sync_manager: Arc<SyncManager>) -> Self`
**Steps:**
1. Build a `ToolRouter` via `Self::tool_router()`.
2. Wrap `sync_manager` in `Some(_)`.

### Tool methods (all `async fn`, all delegate, all decorated with `#[tool(description = ...)]`)
Each method below unwraps a `Parameters<...>` wrapper and calls into another module. The router itself adds no business logic.

- `read_file_content(FileContentParams { file_path }) -> CallToolResult` — delegates to `query_tools::read_file_content(&file_path).await`.
- `search(SearchParams { directory, keyword }) -> CallToolResult` — delegates to `query_tools::search(&directory, &keyword, self.sync_manager.as_ref()).await`.
- `find_definition(FindDefinitionParams { symbol_name, directory }) -> CallToolResult` — delegates to `analysis_tools::find_definition`.
- `find_references(FindReferencesParams) -> CallToolResult` — delegates to `analysis_tools::find_references`.
- `get_dependencies(GetDependenciesParams { file_path }) -> CallToolResult` — delegates to `analysis_tools::get_dependencies`.
- `get_call_graph(GetCallGraphParams { file_path, symbol_name }) -> CallToolResult` — delegates to `analysis_tools::get_call_graph(&file_path, symbol_name.as_deref())`.
- `analyze_complexity(AnalyzeComplexityParams) -> CallToolResult` — delegates to `analysis_tools::analyze_complexity`.
- `health_check(HealthCheckParams) -> CallToolResult` — re-wraps `Parameters` and delegates to `health_tool::health_check`.
- `get_similar_code(GetSimilarCodeParams { query, directory, limit }) -> CallToolResult` — defaults `limit` to 5 and delegates to `query_tools::get_similar_code`.
- `index_codebase(IndexCodebaseParams) -> CallToolResult` — delegates to `index_tool::index_codebase(params, self.sync_manager.as_ref())`.
- `clear_cache(ClearCacheParams) -> CallToolResult` — delegates to `clear_cache_tool::clear_cache`.
- `build_hypergraph(BuildHypergraphParams) -> CallToolResult` — delegates to `graph_tools::build_hypergraph`.
- `get_imports(GraphImportsParams) -> CallToolResult` — delegates to `graph_tools::get_imports`.
- `get_exports(GraphExportsParams) -> CallToolResult` — delegates to `graph_tools::get_exports`.
- `get_reexports(GraphReexportsParams) -> CallToolResult` — delegates to `graph_tools::get_reexports`.
- `get_declared_reexports(GraphDeclaredReexportsParams) -> CallToolResult` — delegates to `graph_tools::get_declared_reexports`.
- `who_imports(WhoImportsParams) -> CallToolResult` — delegates to `graph_tools::who_imports`.
- `who_uses(WhoUsesParams) -> CallToolResult` — delegates to `graph_tools::who_uses`.
- `who_uses_summary(WhoUsesSummaryParams) -> CallToolResult` — delegates to `graph_tools::who_uses_summary`.
- `who_calls(WhoCallsParams) -> CallToolResult` — delegates to `graph_tools::who_calls`.
- `calls_from(CallsFromParams) -> CallToolResult` — delegates to `graph_tools::calls_from`.
- `call_graph(CallGraphParams) -> CallToolResult` — delegates to `graph_tools::call_graph`.
- `callers_in_crate(CallersInCrateParams) -> CallToolResult` — delegates to `graph_tools::callers_in_crate`.
- `recursive_callers_count(RecursiveCallersCountParams) -> CallToolResult` — delegates to `graph_tools::recursive_callers_count`.
- `dead_pub_in_crate(DeadPubParams) -> CallToolResult` — delegates to `graph_tools::dead_pub_in_crate`.
- `dead_pub_report(DeadPubReportParams) -> CallToolResult` — delegates to `graph_tools::dead_pub_report`.
- `crate_edges(CrateEdgesParams) -> CallToolResult` — delegates to `graph_tools::crate_edges`.
- `forbidden_dependency_check(ForbiddenDependencyCheckParams) -> CallToolResult` — delegates to `graph_tools::forbidden_dependency_check`.
- `enum_variants(EnumVariantsParams) -> CallToolResult` — delegates to `graph_tools::enum_variants`.
- `item_attributes(ItemAttributesParams) -> CallToolResult` — delegates to `graph_tools::item_attributes`.
- `items_with_attribute(ItemsWithAttributeParams) -> CallToolResult` — delegates to `graph_tools::items_with_attribute`.
- `pub_use_pub_type_audit(PubUsePubTypeAuditParams) -> CallToolResult` — delegates to `graph_tools::pub_use_pub_type_audit`.
- `re_export_chain(ReExportChainParams) -> CallToolResult` — delegates to `graph_tools::re_export_chain`.
- `crate_dependency_metric(CrateDependencyMetricParams) -> CallToolResult` — delegates to `graph_tools::crate_dependency_metric`.
- `overlaps(OverlapsParams) -> CallToolResult` — delegates to `graph_tools::overlaps`.
- `module_tree(ModuleTreeParams) -> CallToolResult` — delegates to `graph_tools::module_tree`.
- `workspace_stats(WorkspaceStatsParams) -> CallToolResult` — delegates to `graph_tools::workspace_stats`.
- `function_signature(FunctionSignatureParams) -> CallToolResult` — delegates to `graph_tools::function_signature`.
- `functions_with_filter(FunctionsWithFilterParams) -> CallToolResult` — delegates to `graph_tools::functions_with_filter`.
- `unsafe_audit(UnsafeAuditParams) -> CallToolResult` — delegates to `graph_tools::unsafe_audit`.
- `mut_static_audit(MutStaticAuditParams) -> CallToolResult` — delegates to `graph_tools::mut_static_audit`.
- `missing_docs_audit(MissingDocsAuditParams) -> CallToolResult` — delegates to `graph_tools::missing_docs_audit`.
- `derive_audit(DeriveAuditParams) -> CallToolResult` — delegates to `graph_tools::derive_audit`.
- `recursion_check(RecursionCheckParams) -> CallToolResult` — delegates to `graph_tools::recursion_check`.
- `channel_capacity_audit(ChannelCapacityAuditParams) -> CallToolResult` — delegates to `graph_tools::channel_capacity_audit`.
- `fn_body_audit(FnBodyAuditParams) -> CallToolResult` — delegates to `graph_tools::fn_body_audit`.
- `similar_to_item(SimilarToItemParams) -> CallToolResult` — delegates to `graph_tools::similar_to_item`.
- `semantic_overlaps(SemanticOverlapsParams) -> CallToolResult` — delegates to `graph_tools::semantic_overlaps`.

### `impl ServerHandler for SearchToolRouter`
The macro `#[tool_handler]` synthesizes the dispatch glue.

#### `fn get_info(&self) -> ServerInfo`
**Steps:**
1. Build a `ServerInfo` with `ProtocolVersion::V_2024_11_05`.
2. Enable prompts, resources, and tools capabilities via `ServerCapabilities::builder`.
3. Set `server_info` from `Implementation::from_build_env()`.
4. Attach a long human-readable `instructions` string enumerating every MCP tool the router exposes (used by the MCP client as per-tool documentation).

## Module: graph_tools

All tool functions follow a shared shape: open the LMDB hypergraph snapshot, resolve user-supplied qualified names to `NodeId`s, dispatch to an `OpenedSnapshot` query method, and serialize the result as pretty JSON via `json_result`.

### `pub async fn build_hypergraph(params: BuildHypergraphParams) -> Result<CallToolResult, McpError>`
**MCP tool — `build_hypergraph`**: Builds (or reuses) the persisted workspace hypergraph snapshot via `loader::load` + extract pass + LMDB writes, on a blocking thread.
**Call graph:** PathBuf::from -> Path::exists -> tokio::task::spawn_blocking -> graph::build_and_persist -> json_result
**Steps:**
1. Convert `directory` to a `PathBuf` and ensure it exists.
2. Build `BuildOptions { force_rebuild, .. }`.
3. Spawn a blocking task running `build_and_persist(&dir, opts)`.
4. Await the join handle, mapping join errors to `internal_error`.
5. Map build errors to `internal_error`.
6. Serialize a `BuildHypergraphResponse { graph_id, workspace_root, fingerprint, node_count, binding_count, usage_count, reused, snapshot_path }`.

### `pub async fn get_imports(params: GraphImportsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> resolve_required_node -> OpenedSnapshot::imports_of -> OpenedSnapshot::lookup_by_qualified_name -> enrich_bindings -> json_result
**Steps:**
1. Open the snapshot for `directory`.
2. Resolve `module` to a Module `NodeId`.
3. Run `imports_of(module_id)` on the snapshot.
4. Look up the canonical module name (fallback to user input if missing).
5. Enrich bindings via `enrich_bindings` and serialize a `BindingsListResponse`.

### `pub async fn get_exports(params: GraphExportsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> resolve_required_node -> OpenedSnapshot::exports_of -> enrich_bindings -> json_result
**Steps:**
1. Open snapshot, resolve `module` and `consumer` Modules.
2. Call `exports_of(module_id, consumer_id)`.
3. Enrich and serialize as `BindingsListResponse`.

### `pub async fn get_reexports(params: GraphReexportsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> resolve_required_node -> OpenedSnapshot::reexports_of -> enrich_bindings -> json_result
**Steps:**
1. Open snapshot, resolve `module` and `consumer`.
2. Call `reexports_of(module_id, consumer_id)`.
3. Enrich and serialize as `BindingsListResponse`.

### `pub async fn get_declared_reexports(params: GraphDeclaredReexportsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> resolve_required_node -> OpenedSnapshot::declared_reexports_of -> enrich_bindings -> json_result
**Steps:**
1. Open snapshot and resolve `module` to a Module.
2. Call `declared_reexports_of(module_id)`.
3. Enrich and serialize as `BindingsListResponse`.

### `pub async fn who_imports(params: WhoImportsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::who_imports -> enrich_bindings -> json_result
**Steps:**
1. Open snapshot.
2. Look up `target` via `lookup_by_qualified_name` (any node kind allowed).
3. Run `who_imports(target_id)`.
4. Enrich bindings and serialize as `BindingsListResponse { target: target_node.qualified_name, ... }`.

### `pub async fn who_uses(params: WhoUsesParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::usages_of -> enrich_usages -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Call `usages_of(target_id)`.
3. Enrich usages and serialize as `UsagesListResponse`.

### `pub async fn who_uses_summary(params: WhoUsesSummaryParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::who_uses_summary -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Call `who_uses_summary(target_id)`.
3. Serialize as `UsageSummaryResponse { target, rows }`.

### `pub async fn who_calls(params: WhoCallsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::who_calls -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Call `who_calls(target_id)`.
3. Serialize as `CallSitesResponse { target: Some(...), caller: None, call_sites }`.

### `pub async fn calls_from(params: CallsFromParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::calls_from -> json_result
**Steps:**
1. Open snapshot, look up `caller`.
2. Call `calls_from(caller_id)`.
3. Serialize as `CallSitesResponse { target: None, caller: Some(...), call_sites }`.

### `pub async fn call_graph(params: CallGraphParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::call_graph -> json_result
**Steps:**
1. Compute `depth = min(params.depth.unwrap_or(3), 8)`.
2. Open snapshot, look up `root`.
3. Call `call_graph(root_id, depth)` for a `CallGraphNode` tree.
4. Serialize as `CallGraphResponse { root, depth, tree }`.

### `pub async fn callers_in_crate(params: CallersInCrateParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::callers_in_crate -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Call `callers_in_crate(target_id, &params.krate)`.
3. Serialize as `CallersInCrateResponse`.

### `pub async fn recursive_callers_count(params: RecursiveCallersCountParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::recursive_callers_count -> json_result
**Steps:**
1. Compute `depth = min(params.depth.unwrap_or(3), 8)`.
2. Open snapshot, look up `target` (node value discarded).
3. Call `recursive_callers_count(target_id, depth)`.
4. Serialize the resulting `RecursiveCallersCount` directly.

### `pub async fn dead_pub_in_crate(params: DeadPubParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::dead_pub_in_crate -> enrich_dead_pub -> json_result
**Steps:**
1. Open snapshot, look up `krate`.
2. Promote a Module to its owning crate (via `crate_id` or `parent_id`); reject other node kinds.
3. Call `dead_pub_in_crate(crate_id)`.
4. Enrich each finding via `enrich_dead_pub` and serialize as `DeadPubResponse`.

### `pub async fn dead_pub_report(params: DeadPubReportParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::dead_pub_report -> enrich_crate_dead_pub -> json_result
**Steps:**
1. Open snapshot.
2. Call `dead_pub_report()` for a `Vec<CrateDeadPub>`.
3. Map each to `EnrichedCrateDeadPub`, summing total findings.
4. Serialize as `DeadPubReportResponse { workspace, total_findings, crates }`.

### `pub async fn crate_edges(params: CrateEdgesParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::crate_edges -> json_result
**Steps:**
1. Open snapshot.
2. Call `crate_edges()` and serialize as `CrateEdgesResponse { edges }`.

### `pub async fn enum_variants(params: EnumVariantsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::enum_variants -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Reject if `node.item_kind != Some(ItemKind::Enum)`.
3. Call `enum_variants(enum_id)`.
4. Map each variant to `EnrichedEnumVariant { display_name, qualified_name, file, span }`.
5. Serialize as `EnumVariantsResponse`.

### `pub async fn item_attributes(params: ItemAttributesParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::item_attributes -> item_kind_label -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Call `item_attributes(target_id)`.
3. Serialize as `ItemAttributesResponse { target, item_kind, file, span, attribute_count, attributes }`.

### `pub async fn items_with_attribute(params: ItemsWithAttributeParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::items_with_attribute -> item_kind_label -> json_result
**Steps:**
1. Open snapshot, resolve `crate_name` (Crate or Module-with-crate fallback).
2. Call `items_with_attribute(crate_id, &params.attribute_pattern)`.
3. Map each `ItemWithAttribute` to `EnrichedItemWithAttribute` (string-form `item_kind`).
4. Serialize as `ItemsWithAttributeResponse`.

### `pub async fn function_signature(params: FunctionSignatureParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::function_signature -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Call `function_signature(target_id)`.
3. Serialize as `FunctionSignatureResponse { target, signature }` (signature may be `None`).

### `pub async fn similar_to_item(params: SimilarToItemParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> std::fs::read_to_string -> ProjectPaths::from_directory -> query_tools::create_hybrid_search -> HybridSearch::vector_only_search -> json_result
**Steps:**
1. Open snapshot and resolve seed `target`; require both `file` and `span`.
2. Read `<directory>/<seed_file>` to a `String` and slice the `[start, end)` byte range as the seed source.
3. Build a `HybridSearch` for the workspace via `create_hybrid_search`.
4. Run `vector_only_search(seed_source, limit + 1)`.
5. For each result: drop chunks whose file path ends with the seed's relative path AND whose line range overlaps the seed's line range; drop scores below threshold; apply optional `item_kind` filter.
6. Build a 3-line preview from `chunk.content`; push as `SimilarMatch`; stop when `limit` reached.
7. Serialize as `SimilarToItemResp { seed, limit, threshold, item_kind_filter, match_count, matches }`.

### `pub async fn semantic_overlaps(params: SemanticOverlapsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> parse_item_kind_filter -> Sha256::new/update/finalize -> embeddings_by_target.get -> EmbeddingGenerator::new -> EmbeddingGenerator::embed_batch_async -> embeddings_by_target.put -> cosine -> build_clusters -> node_to_item_ref -> json_result
**Steps:**
1. Validate `output_mode` is `pairs` or `clusters`; capture `threshold`, `max_pairs`, `max_cluster_size`, `skip_tests`, `cross_crate_only`, `crate_name`, `item_kind` defaults.
2. Open snapshot.
3. If `crate_name` provided, resolve to a Crate id (Module promoted to crate).
4. Parse `item_kind` filter into an `Option<ItemKind>`.
5. Iterate `nodes_by_id` LMDB cursor: keep only `NodeKind::Item` whose crate (optional) and item_kind (optional) match, with a real file+span, dropping any `::tests::` qualified names when `skip_tests`.
6. For each seed: read its file (cached in `file_cache`), slice the byte range, trim, hash with SHA-256 truncated to 16 bytes, look up `embeddings_by_target` — reuse vector if `content_hash` AND `embedder_version` match, else queue for embedding.
7. Batch-embed misses via `EmbeddingGenerator::embed_batch_async` in chunks of 64; persist each fresh `EmbeddingRecord { content_hash, vector, embedder_version, generated_at_unix }` and update the in-memory `cached_vec`.
8. Identical-source short-circuit: items sharing a `content_hash` get score=1.0 (subject to `cross_crate_only`); use canonical (smaller-id-first) edge keys.
9. In-memory pairwise cosine over remaining (cached_vec, cached_vec) pairs; skip same-hash pairs (already handled), skip same-crate pairs when `cross_crate_only`, drop scores below threshold; accumulate into a `HashMap<EdgeKey, Vec<f32>>`.
10. Average per-direction scores per edge, sort by similarity descending; record `pair_count`.
11. Build `seed_index` lookup table for response item refs.
12. Pairs mode: take `max_pairs`, map to `SimilarityPair { a, b, similarity }`, return `SemanticOverlapsResp` with `pairs` populated.
13. Clusters mode (default): call `build_clusters(&pairs, max_pairs, lookup)`, drop clusters with `size > max_cluster_size`, return `SemanticOverlapsResp` with `clusters` populated.

### `pub async fn functions_with_filter(params: FunctionsWithFilterParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::functions_with_filter -> json_result
**Steps:**
1. Open snapshot, resolve `krate` to a Crate id (Module promoted).
2. Parse `self_kind` string into `Option<SelfKindFilter>` (rejecting unknown values).
3. Build a `FunctionFilter { min_param_count, has_param_type, returns_type_pattern, is_async, self_kind }`.
4. Call `functions_with_filter(crate_id, &filter)` for `Vec<FunctionWithSignature>`.
5. Compute `total_match_count`; apply `offset` and `limit` (defaults 0/50).
6. Build `FunctionsWithFilterMatch` per row, dropping `signature` when `summary=true`.
7. Serialize as `FunctionsWithFilterResponse { krate, total_match_count, offset, limit, match_count, matches }`.

### `pub async fn forbidden_dependency_check(params: ForbiddenDependencyCheckParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::forbidden_dependency_check -> json_result
**Steps:**
1. Open snapshot.
2. Map each input `ForbiddenDependencyRuleParam` to `ForbiddenDependencyRule`.
3. Call `forbidden_dependency_check(&rules)`.
4. Serialize as `ForbiddenDependencyCheckResponse { rule_count, violation_count, violations }`.

### `pub async fn pub_use_pub_type_audit(params: PubUsePubTypeAuditParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::pub_use_pub_type_audit -> OpenedSnapshot::node_by_id -> json_result
**Steps:**
1. Open snapshot, resolve `crate_name` to a Crate id.
2. Call `pub_use_pub_type_audit(crate_id)`.
3. For each finding, look up the `pub use` target's qualified name via `node_by_id` (best-effort).
4. Serialize as `PubUsePubTypeAuditResponse`.

### `pub async fn re_export_chain(params: ReExportChainParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> OpenedSnapshot::re_export_chain -> json_result
**Steps:**
1. Open snapshot, look up `target`.
2. Call `re_export_chain(target_id)`.
3. Map each `ReExportLink` to `EnrichedReExportLink { from_module, visible_name, depth }`.
4. Serialize as `ReExportChainResponse`.

### `pub async fn crate_dependency_metric(params: CrateDependencyMetricParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::crate_dependency_metric -> NodeId::to_hex -> json_result
**Steps:**
1. Open snapshot.
2. Call `crate_dependency_metric()` for `Vec<CrateMetric>`.
3. If `sort_by` is supplied, sort descending by `instability` / `abstractness` / `item_count` / `afferent` / `efferent` (rejecting unknown keys).
4. Apply `top_n` truncation.
5. Map each `CrateMetric` to `CrateMetricRendered` (hex `crate_id`).
6. Serialize as `CrateDependencyMetricResponse`.

### `pub async fn overlaps(params: OverlapsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::overlaps -> json_result
**Steps:**
1. Open snapshot.
2. Call `overlaps()` and serialize the `OverlapsReport` directly.

### `pub async fn module_tree(params: ModuleTreeParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::module_tree -> json_result
**Steps:**
1. Open snapshot.
2. Call `module_tree(&params.krate, params.depth)`.
3. Serialize as `ModuleTreeResponse { tree }`.

### `pub async fn workspace_stats(params: WorkspaceStatsParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::workspace_stats -> json_result
**Steps:**
1. Open snapshot.
2. Call `workspace_stats()` and serialize the resulting `WorkspaceStats` directly.

### `pub async fn unsafe_audit(params: UnsafeAuditParams) -> Result<CallToolResult, McpError>`
**Call graph:** tokio::task::spawn_blocking -> open_workspace_snapshot -> Path::canonicalize -> graph::loader::load -> OpenedSnapshot::unsafe_audit -> NodeId::to_hex -> json_result
**Steps:**
1. Spawn a blocking task: open snapshot, canonicalize directory, run `loader::load`, and call `snap.unsafe_audit(&loaded)`.
2. Await join, double-`?` propagating both join errors and inner errors.
3. Map each `UnsafeFinding` to a local `UnsafeFindingRendered` (hex `enclosing_function`).
4. Serialize as `Resp { directory, finding_count, findings }`.

### `pub async fn mut_static_audit(params: MutStaticAuditParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::mut_static_audit -> NodeId::to_hex -> json_result
**Steps:**
1. Open snapshot.
2. Call `mut_static_audit()`.
3. Map each finding to a local `MutStaticFindingRendered` (hex `item`).
4. Serialize as `Resp { directory, finding_count, findings }`.

### `pub async fn missing_docs_audit(params: MissingDocsAuditParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> docs_audit::default_kind_filter -> parse_item_kind_filter -> docs_audit::missing_docs_audit -> item_kind_label -> NodeId::to_hex -> json_result
**Steps:**
1. Open snapshot.
2. Resolve optional `crate_name` to a Crate id (Module promoted) → `crate_id_filter`.
3. Build the `kind_filter` set: default kinds when `item_kind` is `None`, else parse each label via `parse_item_kind_filter`.
4. Build `AuditOpts { crate_id_filter, kind_filter, skip_test_items: default true }`.
5. Call `docs_audit::missing_docs_audit(&snap, opts)`.
6. Map each finding to a local `MissingDocsFindingRendered` (hex `target`).
7. Serialize as `Resp { scope, finding_count, findings }`.

### `pub async fn derive_audit(params: DeriveAuditParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> derive_audit::default_kind_filter -> parse_item_kind_filter -> derive_audit::derive_audit -> item_kind_label -> NodeId::to_hex -> json_result
**Steps:**
1. Open snapshot.
2. Resolve optional `crate_name` to Crate id.
3. Build `kind_filter` from defaults or parsed labels (rejecting any kind outside Struct/Enum/Union).
4. Reject empty `required_derives` with `invalid_params`.
5. Build `AuditOpts { crate_id_filter, kind_filter, required_derives, pub_only: default true, skip_test_items: default true }`.
6. Call `derive_audit::derive_audit(&snap, opts)`.
7. Map each finding to a local `DeriveFindingRendered` (hex `target`, list `current_derives` and `missing_derives`).
8. Serialize as `Resp { scope, required_derives, finding_count, findings }`.

### `pub async fn recursion_check(params: RecursionCheckParams) -> Result<CallToolResult, McpError>`
**Call graph:** open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> recursion_check::clamp_cycle_length -> recursion_check::recursion_check -> recursion_check::enclosing_fn_qualified_names -> NodeId::to_hex -> json_result
**Steps:**
1. Open snapshot.
2. Resolve optional `crate_name` to Crate id.
3. Clamp `max_cycle_length` to [1, 12] via `clamp_cycle_length` (defaults to 5).
4. Build `RecursionOpts { crate_id_filter, max_cycle_length }` and call `recursion_check`.
5. For each cycle, resolve each member's qualified name, take the first member's hex as `starting_node_id`.
6. Push to `Vec<RecursionCycleRendered>` and serialize as `Resp { scope, max_cycle_length, cycle_count, cycles }`.

### `struct RecursionCycleRendered`
Fields: `fns: Vec<String>`, `cycle_length: usize`, `direct_recursion: bool`, `starting_node_id: String` — JSON-rendered cycle row.

### `pub async fn channel_capacity_audit(params: ChannelCapacityAuditParams) -> Result<CallToolResult, McpError>`
**Call graph:** tokio::task::spawn_blocking -> open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> Path::canonicalize -> graph::loader::load -> channel_audit::channel_capacity_audit -> NodeId::to_hex -> json_result
**Steps:**
1. Capture `directory`, `crate_name`, `skip_test_fns` (default true) up front.
2. Spawn a blocking task: open snapshot, resolve optional crate scope, canonicalize directory, run `loader::load`, build `ChannelAuditOpts`, run `channel_capacity_audit(&loaded, &snap, opts)`.
3. Await with double-`?` to propagate errors.
4. Map each `ChannelFinding` to `ChannelFindingRendered` (hex `enclosing_function`).
5. Serialize as `Resp { scope, finding_count, findings }`.

### `pub async fn fn_body_audit(params: FnBodyAuditParams) -> Result<CallToolResult, McpError>`
**Call graph:** fn_body_audit::parse_pattern_filter -> tokio::task::spawn_blocking -> open_workspace_snapshot -> OpenedSnapshot::lookup_by_qualified_name -> Path::canonicalize -> graph::loader::load -> fn_body_audit::fn_body_audit -> NodeId::to_hex -> json_result
**Steps:**
1. Parse `patterns` via `parse_pattern_filter` (an empty/None defaults to all 8 built-in patterns; unknown labels return `invalid_params`).
2. Sort the resulting label set into `patterns_used`.
3. Spawn a blocking task: open snapshot, resolve optional crate scope, canonicalize directory, run `loader::load`, build `FnBodyAuditOpts`, run `fn_body_audit(&loaded, &snap, opts)`.
4. Await with double-`?`.
5. Map each `FnBodyFinding` to `FnBodyFindingRendered` (hex `target` when present).
6. Serialize as `Resp { scope, patterns_used, finding_count, findings }`.

### Private helpers

#### `fn open_workspace_snapshot(directory: &str) -> Result<OpenedSnapshot, McpError>`
**Steps:**
1. Canonicalize `directory`, mapping I/O errors to `invalid_params`.
2. Build `GraphPaths::for_workspace(&canonical)`.
3. Call `open_current(&paths, GraphEnvOptions::default())` — return `invalid_params` when no snapshot exists with a hint to run `build_hypergraph`.

#### `fn resolve_required_node(snap, qualified_name, expect_kind) -> Result<NodeId, McpError>`
**Steps:**
1. Look up `qualified_name`, returning `invalid_params` when missing.
2. Return the NodeId immediately if `node.kind == expect_kind`.
3. Transparent fallback: when expecting a Module but receiving a Crate, call `find_root_module_of` and return that id.
4. Otherwise return `invalid_params` with the actual node kind.

#### `fn enrich_bindings(snap, bindings) -> Vec<EnrichedBinding>`
**Steps:**
1. Open a read txn (return empty vec on failure).
2. For each binding, look up `target` and `from_module` nodes by id.
3. Build an `EnrichedBinding` with namespace/kind/visibility labels and qualified-name strings.

#### `fn enrich_usages(snap, usages) -> Vec<EnrichedUsage>`
**Steps:**
1. Open a read txn (return empty vec on failure).
2. For each usage, look up `consumer_module` and optional `consumer_function`.
3. Build an `EnrichedUsage` with `category` label and qualified-name strings.

#### `fn enrich_dead_pub(snap, f: DeadPubFinding) -> EnrichedDeadPub`
**Steps:**
1. Open a read txn (best-effort).
2. Format `declared_visibility` via `visibility_label`.
3. Look up the item's file/span via `node_by_id`.
4. Return `EnrichedDeadPub { qualified_name, item_kind, declared_visibility, file, span }`.

#### `fn enrich_crate_dead_pub(snap, c: CrateDeadPub) -> EnrichedCrateDeadPub`
**Steps:**
1. Map each `DeadPubFinding` through `enrich_dead_pub`.
2. Wrap with `krate: c.crate_qualified_name`.

#### `fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError>`
**Steps:**
1. Serialize the value with `serde_json::to_string_pretty`.
2. Wrap as `CallToolResult::success(vec![Content::text(json)])`.

#### `fn internal_error(label: &'static str) -> impl Fn(anyhow::Error) -> McpError`
Returns a closure that maps `anyhow::Error` to `McpError::internal_error` with the static `label` prefix and `{e:#}` debug formatting.

#### `fn namespace_label(ns: Namespace) -> &'static str`
Maps `Namespace::Type → "Type"`, `Namespace::Value → "Value"`.

#### `fn usage_category_label(c: UsageCategory) -> &'static str`
Maps `Read | Write | Test | Other` to their string forms.

#### `fn item_kind_label(k: ItemKind) -> &'static str`
Maps every `ItemKind` variant to its full PascalCase string form (e.g. `Function`, `Struct`, `Enum`, `EnumVariant`, `AssocFunction`, etc.).

#### `fn binding_kind_label(kind: BindingKind) -> &'static str`
Maps `Declared | NamedImport | GlobImport | ExternCrateImport` to identical strings.

#### `fn node_kind_label(node: &Node) -> String`
Returns `"Workspace"`, `"Crate"`, `"Module"`, or `"ExternalSymbol"` directly. For `NodeKind::Item`, returns `Item.<short_label>` when `item_kind` is set, else `"Item"`.

#### `fn short_item_kind_label(k: ItemKind) -> &'static str`
Like `item_kind_label` but emits the short forms `Fn` / `AssocFn` (instead of `Function` / `AssocFunction`); pairs with `node_kind_label` to produce `Item.Fn` etc.

#### `fn visibility_label(snap, rtxn, vis: &BindingVisibility) -> String`
**Steps:**
1. `Public → "pub"`.
2. `Private → "private"`.
3. `Crate(id) → "pub(crate=<qualified_name>)"` (falling back to `"pub(crate)"` on lookup failure).
4. `RestrictedTo(id) → "pub(in <qualified_name>)"` (falling back to `"pub(in ?)"`).

#### `fn parse_item_kind_filter(s: Option<&str>) -> Result<Option<ItemKind>, McpError>`
**Steps:**
1. Return `Ok(None)` for `None`.
2. Lowercase the input and match against the full label set (`function|fn`, `struct`, `enum`, `union`, `trait`, `typealias|type_alias|type`, `const`, `static`, `assocfunction|assocfn|assoc_function`, `assocconst|assoc_const`, `assoctype|assoc_type`, `method`, `enumvariant|enum_variant|variant`).
3. Return `invalid_params` for unknown labels.

#### `fn line_range_overlaps(a_start, a_end, b_start, b_end) -> bool` (dead-code allowed)
Returns `a_start <= b_end && a_end >= b_start`. Retained for future tools and unit tests.

#### `fn cosine(a: &[f32], b: &[f32]) -> f32`
**Steps:**
1. Iterate paired elements computing dot product and per-vector squared norms.
2. Return `0.0` when either norm is zero (instead of `NaN`).
3. Otherwise return `dot / (sqrt(na) * sqrt(nb))`.

#### `fn resolve_chunk_to_item(snap, chunk_file, chunk_line_start, chunk_line_end, file_contents_cache) -> Option<(NodeId, Node)>` (dead-code allowed)
**Steps:**
1. Open a read txn and iterate `nodes_by_id`.
2. For each Item with file+span, perform component-aware suffix match `chunk_file.ends_with(node.file)`.
3. Read the file (cached) and convert byte spans to line numbers.
4. Return the first item whose line range overlaps the chunk's range.

#### `fn node_to_item_ref(node: &Node) -> ItemRef`
Builds an `ItemRef { qualified_name, item_kind: short label, file, span }` (defaults file to empty / span to (0,0) when missing).

#### `fn build_clusters<F>(edges, max_members, lookup) -> Vec<SimilarityCluster>`
**Call graph:** find (inner closure) -> lookup -> SimilarityCluster::sort_by
**Steps:**
1. Collect the unique node set across all edges, mapping each `NodeId` to a dense index.
2. Run union-find with path compression: every edge unions its two endpoints.
3. Group node indices by representative root.
4. For each group with at least 2 members, gather scores from edges whose endpoints both lie in the group.
5. Compute `avg`, `min` similarity, drop groups whose score list is empty.
6. Cap members at `max_members`, marking `truncated` when the cap kicks in.
7. Sort clusters by `avg_similarity desc`, tie-break by `size desc`, then `min_similarity desc`.

#### `fn _path_marker(_: &Path)` (dead-code allowed)
No-op function preserved only to suppress dead-code warnings on the `Path` import.

### Response shape structs (private, all `#[derive(Debug, Serialize)]`)

- `BuildHypergraphResponse { graph_id, workspace_root, fingerprint, node_count, binding_count, usage_count, reused, snapshot_path }`.
- `BindingsListResponse { module?, consumer?, target?, bindings }`.
- `EnrichedBinding { visible_name, namespace, kind, visibility, from_module?, target?, target_kind? }`.
- `UsagesListResponse { target, usages }`.
- `EnrichedUsage { file, start, end, category, consumer_module?, consumer_function? }`.
- `CallSitesResponse { target?, caller?, call_sites }`.
- `DeadPubResponse { krate, findings }` (renames `krate` JSON key to `crate`).
- `EnrichedDeadPub { qualified_name, item_kind, declared_visibility, file?, span? }`.
- `DeadPubReportResponse { workspace, total_findings, crates }`.
- `EnrichedCrateDeadPub { krate, findings }` (renames JSON key to `crate`).
- `CrateEdgesResponse { edges }`.
- `EnumVariantsResponse { enum_qualified_name, variant_count, variants }`.
- `EnrichedEnumVariant { display_name, qualified_name, file?, span? }`.
- `ForbiddenDependencyCheckResponse { rule_count, violation_count, violations }`.
- `ItemAttributesResponse { target, item_kind?, file?, span?, attribute_count, attributes }`.
- `ItemsWithAttributeResponse { krate, attribute_pattern, match_count, items }` (renames `krate` to `crate`).
- `EnrichedItemWithAttribute { qualified_name, item_kind?, matched_attribute, match_location, file?, span? }`.
- `PubUsePubTypeAuditResponse { krate, finding_count, findings }`.
- `EnrichedPubTypeAuditFinding { alias_qualified_name, file?, span?, suspicious_pub_use_visible_name, suspicious_pub_use_target? }`.
- `ReExportChainResponse { canonical, link_count, links }`.
- `EnrichedReExportLink { from_module, visible_name, depth }`.
- `CrateDependencyMetricResponse { crate_count, metrics }`.
- `CrateMetricRendered { crate_id, crate_name, efferent, afferent, instability, abstractness, item_count }` — `crate_id` is hex.
- `UsageSummaryResponse { target, rows }`.
- `CallGraphResponse { root, depth, tree }`.
- `CallersInCrateResponse { target, krate, call_sites }` (renames `krate` to `crate`).
- `ModuleTreeResponse { tree }`.
- `FunctionSignatureResponse { target, signature }` (signature optional).
- `SimilarToItemResp { seed, limit, threshold, item_kind_filter, match_count, matches }`.
- `SeedItemRef { qualified_name, file, span, item_kind? }`.
- `SimilarMatch { similarity, symbol_name, symbol_kind, file, line_start, line_end, preview }`.
- `SemanticOverlapsResp { scope, threshold, pair_count, output_mode, pairs?, clusters? }`.
- `ScopeSummary { directory, crate_name?, item_kind?, seed_count }`.
- `SimilarityPair { a, b, similarity }`.
- `SimilarityCluster { members, avg_similarity, min_similarity, size, truncated }`.
- `ItemRef { qualified_name, item_kind?, file, span }`.
- `FunctionsWithFilterResponse { krate, total_match_count, offset, limit, match_count, matches }`.
- `FunctionsWithFilterMatch { target, qualified_name, signature? }`.
