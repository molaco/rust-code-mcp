# tools — Abstract Logic

## Module: mod.rs
**Purpose:** Declares all submodules that compose the MCP tool layer.

1. **Expose every tool submodule for the rmcp router** -> `pub mod clear_cache_tool`, `pub mod health_tool`, `pub mod index_tool`, `pub mod project_paths`, `pub mod search_tool`, `pub mod indexing_tools`, `pub mod query_tools`, `pub mod analysis_tools`, `pub mod search_tool_router`, `pub mod graph_tools`

## Module: project_paths
**Purpose:** Derive per-project on-disk paths (BM25 index, metadata cache, vector store) from a workspace directory using a SHA-256 hash.

1. **Hash a directory and assemble cache/index/vector paths plus a stable collection name** -> `ProjectPaths::from_directory()`

## Module: indexing_tools
**Purpose:** Locate and open the shared on-disk Tantivy index and metadata cache rooted in the XDG data directory.

1. **Resolve the XDG data directory with a fallback to a relative folder** -> `data_dir()`
2. **Open or create the Tantivy index for the workspace** -> `open_or_create_index()`
3. **Construct the metadata cache** -> `open_cache()`

## Module: health_tool
**Purpose:** MCP tool that reports BM25, vector store, and Merkle snapshot health for one project or the whole data directory.

1. **Run the health probe and emit a JSON status report** -> `health_check()`
2. **Resolve the local data directory shared with sibling tools** -> private `data_dir()`

## Module: clear_cache_tool
**Purpose:** MCP tool that removes per-project or global cache, index, and vector-store directories.

1. **Delete cache/tantivy/vector directories for one or all projects** -> `clear_cache()`
2. **Resolve the data root and hash a directory into its identifier** -> private `data_dir()`, private `compute_dir_hash()`

## Module: index_tool
**Purpose:** MCP tool that performs incremental (or forced full) indexing of a Rust workspace and tracks it for live sync.

1. **Validate the directory, run incremental indexing, optionally clear prior data, and report stats** -> `index_codebase()`

## Module: search_tool
**Purpose:** Backward-compatibility wrapper that re-exports the router type and defines all parameter structs consumed by router methods.

1. **Re-export the router under the legacy name** -> `pub use SearchToolRouter as SearchTool`
2. **Declare every per-tool parameter struct (Deserialize + JsonSchema) used by the router** -> `SearchParams`, `FileContentParams`, `FindDefinitionParams`, `FindReferencesParams`, `GetDependenciesParams`, `GetCallGraphParams`, `AnalyzeComplexityParams`, `GetSimilarCodeParams`, `BuildHypergraphParams`, `GraphImportsParams`, `GraphExportsParams`, `GraphReexportsParams`, `GraphDeclaredReexportsParams`, `WhoImportsParams`, `WhoUsesParams`, `WhoUsesSummaryParams`, `WhoCallsParams`, `CallsFromParams`, `CallGraphParams`, `CallersInCrateParams`, `RecursiveCallersCountParams`, `DeadPubParams`, `DeadPubReportParams`, `CrateEdgesParams`, `OverlapsParams`, `ForbiddenDependencyRuleParam`, `ForbiddenDependencyCheckParams`, `EnumVariantsParams`, `ItemAttributesParams`, `ItemsWithAttributeParams`, `PubUsePubTypeAuditParams`, `ReExportChainParams`, `CrateDependencyMetricParams`, `ModuleTreeParams`, `WorkspaceStatsParams`, `FunctionSignatureParams`, `UnsafeAuditParams`, `MutStaticAuditParams`, `MissingDocsAuditParams`, `DeriveAuditParams`, `RecursionCheckParams`, `ChannelCapacityAuditParams`, `FnBodyAuditParams`, `FunctionsWithFilterParams`, `SimilarToItemParams`, `SemanticOverlapsParams`

## Module: query_tools
**Purpose:** Implements file reading, hybrid keyword search, and pure-vector semantic search, including index bootstrap/repair logic.

1. **Read a file as UTF-8 with binary detection fallback** -> `read_file_content()`
2. **Run hybrid BM25 + vector search, transparently rebuilding a stale or corrupt index** -> `search()`
3. **Run vector-only semantic similarity search** -> `get_similar_code()`
4. **Build a fresh hybrid search engine for a workspace** -> `create_hybrid_search()`
5. **Open BM25, scrub stale state, and ensure the workspace is indexed** -> private `try_open_bm25()`, private `clean_stale_index()`, private `ensure_indexed()`
6. **Render hit lists into the canonical text response** -> private `format_results()`

## Module: analysis_tools
**Purpose:** Per-file static analyses (definitions, references, imports, call graph, complexity) that delegate to the shared semantic index and Rust parser.

1. **Look up symbol definitions in the global semantic index** -> `find_definition()`
2. **List all references to a symbol** -> `find_references()`
3. **List a single file's imports** -> `get_dependencies()`
4. **Render per-symbol or whole-file call graphs from a parsed file** -> `get_call_graph()`
5. **Compute LOC / comment / symbol / cyclomatic / call-graph metrics for a file** -> `analyze_complexity()`

## Module: search_tool_router
**Purpose:** The rmcp `ToolRouter` host: a thin `#[tool_router]` shell that unwraps `Parameters<T>` and forwards each tool call to the matching submodule, plus the `ServerHandler` providing tool documentation.

1. **Construct the router with or without a sync manager** -> `SearchToolRouter::new()`, `SearchToolRouter::with_sync_manager()`
2. **Delegate file-scoped tools to query_tools** -> `read_file_content()`, `search()`, `get_similar_code()`
3. **Delegate symbol/file analyses to analysis_tools** -> `find_definition()`, `find_references()`, `get_dependencies()`, `get_call_graph()`, `analyze_complexity()`
4. **Delegate operational tools to their dedicated modules** -> `health_check()`, `index_codebase()`, `clear_cache()`
5. **Delegate every hypergraph query to graph_tools** -> `build_hypergraph()`, `get_imports()`, `get_exports()`, `get_reexports()`, `get_declared_reexports()`, `who_imports()`, `who_uses()`, `who_uses_summary()`, `who_calls()`, `calls_from()`, `call_graph()`, `callers_in_crate()`, `recursive_callers_count()`, `dead_pub_in_crate()`, `dead_pub_report()`, `crate_edges()`, `forbidden_dependency_check()`, `enum_variants()`, `item_attributes()`, `items_with_attribute()`, `pub_use_pub_type_audit()`, `re_export_chain()`, `crate_dependency_metric()`, `overlaps()`, `module_tree()`, `workspace_stats()`, `function_signature()`, `functions_with_filter()`, `unsafe_audit()`, `mut_static_audit()`, `missing_docs_audit()`, `derive_audit()`, `recursion_check()`, `channel_capacity_audit()`, `fn_body_audit()`, `similar_to_item()`, `semantic_overlaps()`
6. **Advertise server capabilities and per-tool documentation to MCP clients** -> `ServerHandler::get_info()`

## Module: graph_tools
**Purpose:** All hypergraph-backed MCP tools — open the LMDB workspace snapshot, resolve user-supplied qualified names to `NodeId`s, run a query method on `OpenedSnapshot`, and serialize the result to pretty JSON.

1. **Build or reuse the persisted hypergraph snapshot for a workspace** -> `build_hypergraph()`
2. **Inspect a module's bindings (imports, exports, reexports, declared reexports)** -> `get_imports()`, `get_exports()`, `get_reexports()`, `get_declared_reexports()`
3. **Reverse-look-up importers and non-import users of a symbol** -> `who_imports()`, `who_uses()`, `who_uses_summary()`
4. **Query the function call graph forwards, backwards, and recursively** -> `who_calls()`, `calls_from()`, `call_graph()`, `callers_in_crate()`, `recursive_callers_count()`
5. **Detect dead `pub` items in one crate or across the workspace** -> `dead_pub_in_crate()`, `dead_pub_report()`
6. **Report cross-crate edges, name overlaps, and forbidden-dependency rule violations** -> `crate_edges()`, `overlaps()`, `forbidden_dependency_check()`
7. **Inspect enum variants, attributes, signatures, module trees, and workspace counters** -> `enum_variants()`, `item_attributes()`, `items_with_attribute()`, `function_signature()`, `module_tree()`, `workspace_stats()`
8. **Audit `pub use` aliases, walk re-export chains, and rank crates by Robert Martin metrics** -> `pub_use_pub_type_audit()`, `re_export_chain()`, `crate_dependency_metric()`
9. **Filter functions by signature shape (params, return, async, self-kind), with pagination** -> `functions_with_filter()`
10. **Run code-quality audits over the workspace** -> `unsafe_audit()`, `mut_static_audit()`, `missing_docs_audit()`, `derive_audit()`, `recursion_check()`, `channel_capacity_audit()`, `fn_body_audit()`
11. **Run semantic-similarity tools backed by cached embeddings** -> `similar_to_item()`, `semantic_overlaps()`
12. **Open snapshots and resolve names to NodeIds** -> private `open_workspace_snapshot()`, private `resolve_required_node()`
13. **Enrich raw hypergraph rows with human-readable fields (qualified names, file/span, labels)** -> private `enrich_bindings()`, private `enrich_usages()`, private `enrich_dead_pub()`, private `enrich_crate_dead_pub()`
14. **Serialize results and standardize error mapping** -> private `json_result()`, private `internal_error()`
15. **Render enum and string labels for namespaces, usage categories, item kinds, binding kinds, node kinds, and visibility** -> private `namespace_label()`, private `usage_category_label()`, private `item_kind_label()`, private `short_item_kind_label()`, private `binding_kind_label()`, private `node_kind_label()`, private `visibility_label()`
16. **Parse the `item_kind` filter string supplied by users** -> private `parse_item_kind_filter()`
17. **Score and cluster semantic-similarity edges** -> private `cosine()`, private `build_clusters()`, private `node_to_item_ref()`
18. **Reserved helpers retained for future tools** -> private `line_range_overlaps()`, private `resolve_chunk_to_item()`, private `_path_marker()`
19. **Define the JSON response shapes returned by every graph tool** -> `BuildHypergraphResponse`, `BindingsListResponse`, `EnrichedBinding`, `UsagesListResponse`, `EnrichedUsage`, `CallSitesResponse`, `DeadPubResponse`, `EnrichedDeadPub`, `DeadPubReportResponse`, `EnrichedCrateDeadPub`, `CrateEdgesResponse`, `EnumVariantsResponse`, `EnrichedEnumVariant`, `ForbiddenDependencyCheckResponse`, `ItemAttributesResponse`, `ItemsWithAttributeResponse`, `EnrichedItemWithAttribute`, `PubUsePubTypeAuditResponse`, `EnrichedPubTypeAuditFinding`, `ReExportChainResponse`, `EnrichedReExportLink`, `CrateDependencyMetricResponse`, `CrateMetricRendered`, `UsageSummaryResponse`, `CallGraphResponse`, `CallersInCrateResponse`, `ModuleTreeResponse`, `FunctionSignatureResponse`, `RecursionCycleRendered`, `SimilarToItemResp`, `SeedItemRef`, `SimilarMatch`, `SemanticOverlapsResp`, `ScopeSummary`, `SimilarityPair`, `SimilarityCluster`, `ItemRef`, `FunctionsWithFilterResponse`, `FunctionsWithFilterMatch`
