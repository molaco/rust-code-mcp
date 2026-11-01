//! MCP tool router for `rust-code-mcp`.
//!
//! `SearchToolRouter` is the MCP-protocol entry point. Every tool the server
//! exposes is declared here as a `#[tool]` method; bodies delegate to focused
//! modules under `endpoints/` (`query`, `analysis`, `index`, `health`,
//! `cache`) and `tools::graph`. The router itself
//! only does parameter wrapping, response formatting, and optional sync-manager
//! plumbing — no business logic.
//!
//! ```text
//! MCP Client
//!     ↓
//! SearchToolRouter (this module)
//!     ├─→ endpoints::query / endpoints::analysis (search, navigation, similarity)
//!     ├─→ tools::graph                           (persisted-hypergraph endpoints)
//!     ├─→ endpoints::index                       (hypergraph build, codebase index)
//!     ├─→ endpoints::health                      (health_check)
//!     └─→ endpoints::cache                       (clear_cache)
//! ```
//!
//! External callers should use the stable facade path:
//!
//! ```rust,no_run
//! use rmc_server::tools::SearchTool;
//!
//! let _router = SearchTool::new();
//! ```

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    tool, tool_router,
};

use crate::tools::params::{
    AnalyzeComplexityParams, FileContentParams, FindDefinitionParams, FindReferencesParams,
    GetCallGraphParams, GetDependenciesParams, GetSimilarCodeParams, RenameSymbolParams,
    SearchParams,
};

/// Main tool router struct
#[derive(Clone)]
pub struct SearchToolRouter {
    tool_router: ToolRouter<Self>,
    /// Runtime-owned sync, cache, semantic, and lifecycle state.
    runtime: crate::mcp::RuntimeState,
}

impl SearchToolRouter {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            runtime: crate::mcp::RuntimeState::standalone(),
        }
    }

    /// Create a new SearchToolRouter with background sync manager
    pub fn with_sync_manager(sync_manager: std::sync::Arc<crate::mcp::SyncManager>) -> Self {
        Self::with_runtime_state(crate::mcp::RuntimeState::with_sync_manager(sync_manager))
    }

    pub fn with_server_runtime(runtime: &crate::mcp::ServerRuntime) -> Self {
        Self::with_runtime_state(runtime.state())
    }

    pub fn with_runtime_state(runtime: crate::mcp::RuntimeState) -> Self {
        Self {
            tool_router: Self::tool_router(),
            runtime,
        }
    }

    /// Names of every tool this server offers — what `tools/list` answers,
    /// without a client. The daemon's client checks its list of state-changing
    /// tools against it.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }
}

#[tool_router]
impl SearchToolRouter {
    /// Read and return the content of a specified file
    #[tool(description = "Read the content of a file from the specified path")]
    async fn read_file_content(
        &self,
        Parameters(FileContentParams { file_path }): Parameters<FileContentParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::query::read_file_content(&file_path).await
    }

    /// Perform hybrid search (BM25 + Vector) on Rust code in the specified directory
    #[tool(
        description = "Search for keywords in Rust code using hybrid search (BM25 + semantic vectors)"
    )]
    async fn search(
        &self,
        Parameters(SearchParams {
            directory,
            keyword,
            embedding_profile,
        }): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::query::search(
            &directory,
            &keyword,
            embedding_profile.as_deref(),
            self.runtime.background_sync_manager(),
            self.runtime.workspace_locks(),
            Some(self.runtime.search_cache()),
        )
        .await
    }

    /// Find the definition of a symbol by name
    #[tool(
        description = "Find where a Rust symbol (function, struct, trait, const, etc.) is defined. Default matching preserves rust-analyzer substring/fuzzy search and ranks exact hits first; set `exact=true` to return only full-name matches. Each result line is tagged with `exact=true/false`."
    )]
    async fn find_definition(
        &self,
        Parameters(FindDefinitionParams {
            symbol_name,
            directory,
            exact,
        }): Parameters<FindDefinitionParams>,
    ) -> Result<CallToolResult, McpError> {
        let semantic = self.runtime.semantic();
        crate::tools::endpoints::analysis::find_definition_with_semantic(
            &semantic,
            &symbol_name,
            &directory,
            exact.unwrap_or(false),
        )
        .await
    }

    /// Find all references to a symbol by name
    #[tool(description = "Find all places where a symbol is used (calls, type references, etc.). Default matching preserves rust-analyzer substring/fuzzy search and ranks exact source symbols first; set `exact=true` to resolve only full-name matches. Each result line is tagged with `exact=true/false`.")]
    async fn find_references(
        &self,
        Parameters(FindReferencesParams {
            symbol_name,
            directory,
            exact,
        }): Parameters<FindReferencesParams>,
    ) -> Result<CallToolResult, McpError> {
        let semantic = self.runtime.semantic();
        crate::tools::endpoints::analysis::find_references_with_semantic(
            &semantic,
            &symbol_name,
            &directory,
            exact.unwrap_or(false),
        )
        .await
    }

    /// Preview a rename of a symbol across the project (read-only, no files modified)
    #[tool(
        description = "Preview renaming a Rust symbol project-wide using rust-analyzer. Returns the set of edits and file moves WITHOUT modifying any files. If symbol_name is ambiguous, rerun with file_path, line, and column from the candidate list. Caveat: rust-analyzer's rename can miss occurrences inside macro bodies (e.g. vec![]) — cross-check with grep before applying."
    )]
    async fn rename_symbol(
        &self,
        Parameters(RenameSymbolParams {
            symbol_name,
            new_name,
            directory,
            file_path,
            line,
            column,
        }): Parameters<RenameSymbolParams>,
    ) -> Result<CallToolResult, McpError> {
        let semantic = self.runtime.semantic();
        crate::tools::endpoints::analysis::rename_symbol_with_semantic(
            &semantic,
            &symbol_name,
            &new_name,
            &directory,
            file_path.as_deref(),
            line,
            column,
        )
        .await
    }

    /// Get dependencies for a file (imports and files that depend on it)
    #[tool(description = "Get import dependencies for a Rust source file")]
    async fn get_dependencies(
        &self,
        Parameters(GetDependenciesParams { file_path }): Parameters<GetDependenciesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::analysis::get_dependencies(&file_path).await
    }

    /// Get call graph for a file or specific symbol
    #[tool(description = "Get the call graph showing function call relationships")]
    async fn get_call_graph(
        &self,
        Parameters(GetCallGraphParams {
            file_path,
            symbol_name,
        }): Parameters<GetCallGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::analysis::get_call_graph(&file_path, symbol_name.as_deref()).await
    }

    /// Analyze code complexity metrics for a file
    #[tool(
        description = "Analyze code complexity metrics (LOC, cyclomatic complexity, function count)"
    )]
    async fn analyze_complexity(
        &self,
        Parameters(AnalyzeComplexityParams { file_path }): Parameters<AnalyzeComplexityParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::analysis::analyze_complexity(&file_path).await
    }

    /// Check system health status
    #[tool(
        description = "Check the health status of the code search system (BM25, Vector store, Merkle tree, index coverage and freshness). Pass 'directory': without it neither coverage nor freshness can be judged."
    )]
    async fn health_check(
        &self,
        Parameters(params): Parameters<crate::tools::endpoints::health::HealthCheckParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::health::health_check(Parameters(params)).await
    }

    /// Find semantically similar code using vector search
    #[tool(description = "Find code snippets semantically similar to a query using embeddings")]
    async fn get_similar_code(
        &self,
        Parameters(GetSimilarCodeParams {
            query,
            directory,
            limit,
            embedding_profile,
        }): Parameters<GetSimilarCodeParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = limit.unwrap_or(5);
        crate::tools::endpoints::query::get_similar_code(
            &query,
            &directory,
            limit,
            embedding_profile.as_deref(),
            self.runtime.workspace_locks(),
            Some(self.runtime.search_cache()),
        )
        .await
    }

    /// Manually index a codebase directory with automatic change detection
    #[tool(
        description = "Manually index a codebase directory (incremental indexing with Merkle tree change detection)"
    )]
    async fn index_codebase(
        &self,
        Parameters(params): Parameters<crate::tools::endpoints::index::IndexCodebaseParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::index::index_codebase(
            params,
            self.runtime.background_sync_manager(),
            self.runtime.workspace_locks(),
            Some(self.runtime.search_cache()),
        )
        .await
    }

    /// Clear corrupted cache, index, and vector store files
    #[tool(
        description = "Delete the on-disk metadata cache, tantivy index and vector store (fixes 'Failed to open MetadataCache'). include_hypergraph=true also wipes the persisted graph snapshot, so the next build is full; dry_run=true only lists what would go. The response names every directory cleared."
    )]
    async fn clear_cache(
        &self,
        Parameters(params): Parameters<crate::tools::endpoints::cache::ClearCacheParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::cache::clear_cache(
            params,
            self.runtime.sync_manager(),
            self.runtime.workspace_locks(),
            Some(self.runtime.search_cache()),
        )
        .await
    }

    /// Report runtime-owned background task, sync, cache, semantic, and process state
    #[tool(
        description = "Report MCP server runtime state: tracked sync directories, search cache keys, semantic project cache, background sync task status, PID, and RSS."
    )]
    async fn runtime_status(
        &self,
        Parameters(params): Parameters<crate::tools::endpoints::runtime::RuntimeStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::runtime::runtime_status(&self.runtime, params).await
    }

    /// Report per-query memory of one loaded rust-analyzer analysis
    #[tool(
        description = "Memory of an ALREADY LOADED rust-analyzer analysis per salsa ingredient (memo count, heap where measured, largest first), plus a whole-daemon breakdown of rss + swap (mappings, allocator free space, salsa, interners, unattributed). Never loads a workspace to answer. `heap` is a lower bound: compare accounted_bytes with rss_kib. `top` caps the lists."
    )]
    async fn analysis_memory(
        &self,
        Parameters(params): Parameters<crate::tools::endpoints::runtime::AnalysisMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::runtime::analysis_memory(&self.runtime, params).await
    }

    /// Clear runtime-owned in-memory caches and sync tracking
    #[tool(
        description = "Clear MCP runtime caches and sync tracking. scope values: all, workspace, semantic_only, search_cache_only, sync_tracking_only. Pass workspace to target one workspace. This does not stop the background sync task; process shutdown cancels tasks through ServerRuntime."
    )]
    async fn clear_runtime(
        &self,
        Parameters(params): Parameters<crate::tools::endpoints::runtime::ClearRuntimeParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::endpoints::runtime::clear_runtime(&self.runtime, params).await
    }

    // ----- Hypergraph tools (Layer 7) -----

    #[tool(
        description = "Build or reuse the immutable HIR graph that graph and audit tools read (all features, lib/bin targets; no test cfg or build-script outputs). Source edits during the build reload a fresh slice; after three changed attempts it fails without replacing CURRENT. force_rebuild makes a fresh instance. Ambiguous paths take target selectors: app[lib]::helper / app[bin]::helper."
    )]
    async fn build_hypergraph(
        &self,
        Parameters(params): Parameters<crate::tools::params::BuildHypergraphParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::build_hypergraph(params).await
    }

    #[tool(
        description = "Lossless machine-readable export of an immutable HIR graph, paged by table. Call without `table` for metadata, coverage and graph_id, then page each listed table with graph_id and next_cursor. A CURRENT change mid-export is an error, never mixed pages. For bulk offline export use the rmc-graph graph_export example (JSONL)."
    )]
    async fn export_graph(
        &self,
        Parameters(params): Parameters<crate::tools::params::GraphExportParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::export_graph(params).await
    }

    #[tool(
        description = "List `use`/extern-crate imports in a module from the persisted hypergraph. For complete module-level dependencies including fully-qualified inline references, use module_dependencies."
    )]
    async fn get_imports(
        &self,
        Parameters(params): Parameters<crate::tools::params::GraphImportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::get_imports(params).await
    }

    #[tool(
        description = "List modules referenced by a module, combining `use`/extern-crate imports with non-import usage edges, including fully-qualified inline paths. Results are grouped by target module and include import/usage counts plus contributing symbols unless `summary=true`."
    )]
    async fn module_dependencies(
        &self,
        Parameters(params): Parameters<crate::tools::params::ModuleDependenciesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::module_dependencies(params).await
    }

    #[tool(
        description = "List items declared in or re-exported from a module that are visible from a given consumer module"
    )]
    async fn get_exports(
        &self,
        Parameters(params): Parameters<crate::tools::params::GraphExportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::get_exports(params).await
    }

    #[tool(
        description = "List re-exports (the subset of get_exports that came via `pub use`) visible from a given consumer module"
    )]
    async fn get_reexports(
        &self,
        Parameters(params): Parameters<crate::tools::params::GraphReexportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::get_reexports(params).await
    }

    #[tool(
        description = "List every explicit `pub use` (or `pub(crate)` / `pub(in path)` / `pub(super)`) declared in a module, regardless of whether it's reachable from any specific consumer. Use this to audit a module's declared re-export surface; for visibility-filtered re-exports use get_reexports instead."
    )]
    async fn get_declared_reexports(
        &self,
        Parameters(params): Parameters<crate::tools::params::GraphDeclaredReexportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::get_declared_reexports(params).await
    }

    #[tool(
        description = "Find every workspace module that imports the given symbol (matched by qualified name)"
    )]
    async fn who_imports(
        &self,
        Parameters(params): Parameters<crate::tools::params::WhoImportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::who_imports(params).await
    }

    #[tool(
        description = "List resolved non-import references to a symbol, with file/byte range, semantic relation (Call, TraitDispatch, FunctionReference, TypeRef, Construct, Read, Write, Use, Implements), category and consumer_item/consumer_function. Imports are separate bindings. Includes aliases, fields and trait implementation bodies. This is configured-loader coverage, not a completeness claim."
    )]
    async fn who_uses(
        &self,
        Parameters(params): Parameters<crate::tools::params::WhoUsesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::who_uses(params).await
    }

    #[tool(
        description = "Aggregation rollup of resolved non-import references, grouped by lexical consumer module with total count and Read/Write/Test/Other category breakdown. Category is independent of semantic relation: these counts are not call counts. Unresolved indirect calls are separate records returned by calls_from."
    )]
    async fn who_uses_summary(
        &self,
        Parameters(params): Parameters<crate::tools::params::WhoUsesSummaryParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::who_uses_summary(params).await
    }

    #[tool(
        description = "Call/TraitDispatch sites targeting a function, from function bodies, with relation, category and source site. Concrete trait calls name the impl; generic/dyn calls name the trait method. Excludes function values, types and imports — use who_uses for every resolved reference. Closures count toward the enclosing fn."
    )]
    async fn who_calls(
        &self,
        Parameters(params): Parameters<crate::tools::params::WhoCallsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::who_calls(params).await
    }

    #[tool(
        description = "Outgoing Call/TraitDispatch sites from a function body (relation, category), plus every unresolved call of that body with range and reason (IndirectCallable, TargetOutsideGraph, Unresolved), not paginated. Excludes type and function-value references and imports. No points-to analysis. Closures count toward the enclosing fn."
    )]
    async fn calls_from(
        &self,
        Parameters(params): Parameters<crate::tools::params::CallsFromParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::calls_from(params).await
    }

    #[tool(
        description = "Tree of outgoing calls from `root`, `depth` default 3, max 8. Each node: fn_qualified_name, crate_name, callees; truncated_at_cycle = already expanded elsewhere in the tree, truncated_at_depth = unvisited callees remain."
    )]
    async fn call_graph(
        &self,
        Parameters(params): Parameters<crate::tools::params::CallGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::call_graph(params).await
    }

    #[tool(
        description = "`who_calls(target)` filtered to call sites whose *caller fn* lives in the named crate. Note: this filters by the caller's crate, not the target's. Useful for asking 'which fns inside crate X call Y?' regardless of which crate Y lives in."
    )]
    async fn callers_in_crate(
        &self,
        Parameters(params): Parameters<crate::tools::params::CallersInCrateParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::callers_in_crate(params).await
    }

    #[tool(
        description = "Reverse BFS from `target`: counts distinct caller fns reachable backward up to `depth` hops. Returns { direct_callers, transitive_callers, depth_reached, truncated_at_depth }. Counts *fns*, not call sites — a fn that calls target 5 times counts as 1 caller. `depth` defaults to 3 and is capped at 8. depth=0 returns zeros; depth=1 is just the direct caller count."
    )]
    async fn recursive_callers_count(
        &self,
        Parameters(params): Parameters<crate::tools::params::RecursiveCallersCountParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::recursive_callers_count(params).await
    }

    #[tool(
        description = "Scan a local crate for `pub` items with no cross-crate importer or reference — candidates for downgrading to `pub(crate)`. Conservative: may miss items used only through public type signatures."
    )]
    async fn dead_pub_in_crate(
        &self,
        Parameters(params): Parameters<crate::tools::params::DeadPubParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::dead_pub_in_crate(params).await
    }

    #[tool(
        description = "Run dead_pub_in_crate over every local crate in the workspace and return a single aggregated report. Each finding includes file path + byte span so callers can navigate directly to the declaration."
    )]
    async fn dead_pub_report(
        &self,
        Parameters(params): Parameters<crate::tools::params::DeadPubReportParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::dead_pub_report(params).await
    }

    #[tool(
        description = "All cross-crate consumer→producer edges in the workspace, with the symbols carrying each edge (sorted by total ref count desc). Local inherent method calls and local trait-declaration dispatch are captured in usage counts as Method items; unresolved indirect calls such as external dyn-trait dispatch or generic Fn calls remain blind spots."
    )]
    async fn crate_edges(
        &self,
        Parameters(params): Parameters<crate::tools::params::CrateEdgesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::crates::crate_edges(params).await
    }

    #[tool(
        description = "Architecture rule check over crate_edges. Each rule: glob `consumer`/`producer` crate patterns, optional consumer_kinds (default lib, bin), except, severity, message. Returns one violation per rule × matching edge with sample symbols and ref counts. Blind to unresolved indirect calls (external dyn-trait, generic Fn), as crate_edges is."
    )]
    async fn forbidden_dependency_check(
        &self,
        Parameters(params): Parameters<crate::tools::params::ForbiddenDependencyCheckParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::crates::forbidden_dependency_check(params).await
    }

    #[tool(
        description = "Enumerate the variants of an enum: returns one row per variant with display_name, qualified_name, and (file, byte span) so callers can navigate to the declaration. `target` is the enum's qualified name (e.g. `my_crate::ErrorKind`). Use this with who_uses(MyEnum::SomeVariant) to investigate per-variant fan-in."
    )]
    async fn enum_variants(
        &self,
        Parameters(params): Parameters<crate::tools::params::EnumVariantsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::enum_variants(params).await
    }

    #[tool(
        description = "Outer attributes and doc-comment lines recorded for the Item at `target`. Returns the trimmed source text of each `#[...]` attribute (e.g. `#[derive(Debug, Clone)]`, `#[must_use]`, `#[non_exhaustive]`, `#[inline]`) and each doc-comment line as `/// ...` (one entry per line). Source order preserved. Empty list when the item has no attributes or its AST source can't be resolved."
    )]
    async fn item_attributes(
        &self,
        Parameters(params): Parameters<crate::tools::params::ItemAttributesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::item_attributes(params).await
    }

    #[tool(
        description = "Items in a crate with an attribute or `///` doc line matching `attribute_pattern`: a bare path (`derive`, `must_use`, `cfg`) or a doc-body prefix (`SAFETY`). Matching is anchored at the attribute start, so a mention inside another attribute does not count. Each row has match_location attr|doc."
    )]
    async fn items_with_attribute(
        &self,
        Parameters(params): Parameters<crate::tools::params::ItemsWithAttributeParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::items_with_attribute(params).await
    }

    #[tool(
        description = "Heuristic: `pub type` aliases in a crate whose module also has `pub use … as <alias>` — possibly a re-export disguised as an alias. Cannot confirm both point at the same target; verify with find_definition."
    )]
    async fn pub_use_pub_type_audit(
        &self,
        Parameters(params): Parameters<crate::tools::params::PubUsePubTypeAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::pub_use_pub_type_audit(params).await
    }

    #[tool(
        description = "Walk every `pub use` re-export of `target` (and every re-export of those re-exports) up to 8 hops with cycle detection. Returns one link per visited binding, breadth-first, with the from-module qualified name, visible_name, and depth. Useful for auditing the public surface of a type — i.e. \"every place `Token` is re-exported and the canonical declaration\"."
    )]
    async fn re_export_chain(
        &self,
        Parameters(params): Parameters<crate::tools::params::ReExportChainParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::re_export_chain(params).await
    }

    #[tool(
        description = "Per local crate: efferent (Ce) and afferent (Ca) crate counts, Martin instability Ce/(Ce+Ca) (0 stable, 1 unstable) and abstractness (traits + pub type aliases) / items; 0.0 on degenerate counts. sort_by: instability | item_count | afferent | efferent | abstractness (descending); top_n caps rows."
    )]
    async fn crate_dependency_metric(
        &self,
        Parameters(params): Parameters<crate::tools::params::CrateDependencyMetricParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::crates::crate_dependency_metric(params).await
    }

    #[tool(
        description = "Workspace-wide name-collision report: cross-crate type collisions, module names that shadow another crate, within-crate type duplicates, and fn names that appear in 4+ crates. Optional `scope`: `all` (default, current behavior), `local` (lib/bin targets only), or `local_no_vendor` (lib/bin targets excluding source under vendor/)."
    )]
    async fn overlaps(
        &self,
        Parameters(params): Parameters<crate::tools::params::OverlapsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::overlaps(params).await
    }

    #[tool(
        description = "Recursive module/item tree of a crate. Without `depth`, a tree over 400 nodes is cut at the deepest level that fits, with truncated_at_depth and counts in the response."
    )]
    async fn module_tree(
        &self,
        Parameters(params): Parameters<crate::tools::params::ModuleTreeParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::module_tree(params).await
    }

    #[tool(
        description = "List crate-owned type items from the hypergraph snapshot. Defaults to Struct, Enum, Union, Trait, and TypeAlias; optional filters support pure `pub` only, associated types, test-module exclusion, and pagination. Requires build_hypergraph to have created a snapshot first."
    )]
    async fn crate_types(
        &self,
        Parameters(params): Parameters<crate::tools::params::CrateTypesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::crate_types(params).await
    }

    #[tool(
        description = "WRITES files: parseable Rust skeletons (function bodies and initializers stripped) under `<directory>/.skeleton/`, mirroring the source layout, plus one aggregate `<package>.rs` per package — compact codebase context. Requires build_hypergraph."
    )]
    async fn crate_skeleton(
        &self,
        Parameters(params): Parameters<crate::tools::params::CrateSkeletonParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::skeleton::crate_skeleton(params).await
    }

    #[tool(
        description = "Workspace-wide counters: nodes by kind, items by ItemKind, bindings by kind, visibility breakdown, pub_crate/total_items encapsulation ratio. In `visibility`, module_private is the canonical private bucket; pub_self/private are legacy aliases (see visibility_notes); restricted_to covers pub(super)/pub(in path)."
    )]
    async fn workspace_stats(
        &self,
        Parameters(params): Parameters<crate::tools::params::WorkspaceStatsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::core::workspace_stats(params).await
    }

    #[tool(
        description = "Recorded signature of a function (free, inherent, trait declaration): is_async, self_param, params (name, type, by_ref, mutability), return_type, generics with declaration-site bounds — where-clause bounds are NOT included. Types rendered by rust-analyzer with allocator/hasher noise trimmed. `signature: null` when the target is not a fn."
    )]
    async fn function_signature(
        &self,
        Parameters(params): Parameters<crate::tools::params::FunctionSignatureParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::function_signature(params).await
    }

    #[tool(
        description = "Local functions in a crate whose signature matches every given filter: has_param_type / returns_type_pattern (case-sensitive substrings of rendered types), self_kind (none | owned | ref | ref_mut). Trait-impl methods excluded. Sorted by qualified name, paged; `summary` drops the signature payload."
    )]
    async fn functions_with_filter(
        &self,
        Parameters(params): Parameters<crate::tools::params::FunctionsWithFilterParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::functions_with_filter(params).await
    }

    #[tool(
        description = "Every `unsafe { }` block in local crates: file, span, line count, enclosing fn, has_safety_comment (`SAFETY` within the 5 preceding lines). Use to find unsafe blocks missing `// SAFETY:` or to review unsafe size. Live syntax walk; the cost is the workspace load."
    )]
    async fn unsafe_audit(
        &self,
        Parameters(params): Parameters<crate::tools::params::UnsafeAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::audits::unsafe_audit(params).await
    }

    #[tool(
        description = "Local `static` items holding global mutable state — `static mut`, LazyLock, OnceLock, OnceCell — classified by HIR type, not text. One finding per matched pattern: qualified_name, pattern, type, file, span. `lazy_static!` is NOT detected; use items_with_attribute or grep for it. Finds hidden singletons (config, clocks, RNGs)."
    )]
    async fn mut_static_audit(
        &self,
        Parameters(params): Parameters<crate::tools::params::MutStaticAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::audits::mut_static_audit(params).await
    }

    #[tool(
        description = "Pure `pub` local items without a `///` doc comment (pub(crate) and private are skipped). Optional crate_name and item_kind (default: documentable kinds, no variants or associated items); test items skipped by default. Each finding: qualified_name, kind, file, span. Reads the hypergraph snapshot, no rust-analyzer load."
    )]
    async fn missing_docs_audit(
        &self,
        Parameters(params): Parameters<crate::tools::params::MissingDocsAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::missing_docs_audit(params).await
    }

    #[tool(
        description = "Local structs/enums/unions (pub only by default) missing any of `required_derives`, e.g. [\"Debug\", \"Clone\"]; path qualifiers are ignored. Each finding: qualified_name, kind, file, span, current_derives, missing_derives. Optional crate_name, item_kind; test items skipped by default. Reads the hypergraph snapshot, no rust-analyzer load."
    )]
    async fn derive_audit(
        &self,
        Parameters(params): Parameters<crate::tools::params::DeriveAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::surface::derive_audit(params).await
    }

    #[tool(
        description = "Functions in recursion cycles (self or mutual), by bounded DFS over the call graph up to max_cycle_length (default 5, max 12); each cycle reported once. Per cycle: fns in order, cycle_length, direct_recursion. crate_name keeps cycles with at least one member in that crate."
    )]
    async fn recursion_check(
        &self,
        Parameters(params): Parameters<crate::tools::params::RecursionCheckParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::audits::recursion_check(params).await
    }

    #[tool(
        description = "Channel constructors in local crates — tokio and std mpsc, crossbeam, flume, bounded and unbounded; aliased imports resolved. Each finding: kind, bounded, literal capacity if any, file, span, enclosing fn. Use to find unbounded channels. Tests skipped by default; the cost is the workspace load."
    )]
    async fn channel_capacity_audit(
        &self,
        Parameters(params): Parameters<crate::tools::params::ChannelCapacityAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::audits::channel_capacity_audit(params).await
    }

    #[tool(
        description = "Pattern hits in local fn bodies: unwrap, expect, panic_macros, unwrap_unchecked, transmute, await_in_guard_scope, self_recursion, unbounded_loop (pick with `patterns`). Each finding: enclosing fn, pattern, file, span, short context. Heuristic: unwrap is by method name, guard scope by text, event loops get flagged. Tests skipped by default. Requires build_hypergraph."
    )]
    async fn fn_body_audit(
        &self,
        Parameters(params): Parameters<crate::tools::params::FnBodyAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::audits::fn_body_audit(params).await
    }

    #[tool(
        description = "Semantic neighbours of an item: takes the source of `target` (qualified name) and runs vector search with it, self-match dropped. Optional threshold, limit (default 10), item_kind. For \"what else looks like X\" — duplicate error types, parser variants. Requires both build_hypergraph and index_codebase."
    )]
    async fn similar_to_item(
        &self,
        Parameters(params): Parameters<crate::tools::params::SimilarToItemParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::similarity::similar_to_item(
            params,
            self.runtime.workspace_locks(),
            Some(self.runtime.search_cache()),
        )
        .await
    }

    #[tool(
        description = "Workspace-wide near-duplicate audit: embeds every item's source and returns similar pairs or single-linkage clusters (output_mode, default clusters) above `threshold` (default: the embedding profile's tuned cutoff). For duplicate detection and refactor planning. First run embeds everything (seconds to minutes), reruns are cached. Requires build_hypergraph, not index_codebase."
    )]
    async fn semantic_overlaps(
        &self,
        Parameters(params): Parameters<crate::tools::params::SemanticOverlapsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::similarity::semantic_overlaps(
            params,
            self.runtime.workspace_locks(),
        )
        .await
    }

    #[tool(
        description = "Task-focused subgraph (nodes, typed edges, hierarchy) of the workspace graph, seeded by `seed_qualified_names` (exact; best for pinpoint navigation) or `task_prompt` (hybrid search; needs index_codebase). Bounded view — max_nodes 80, depth 3 — not a complete export; unresolved seeds go to diagnostics. format: json | mermaid | outline | all. Requires build_hypergraph."
    )]
    async fn build_codemap(
        &self,
        Parameters(crate::tools::params::BuildCodemapParams {
            directory,
            task_prompt,
            embedding_profile,
            seed_qualified_names,
            max_nodes,
            depth,
            max_incoming_per_node,
            embedding_policy,
            format,
            include_snippets,
        }): Parameters<crate::tools::params::BuildCodemapParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph::codemap::handle_build_codemap(
            &directory,
            task_prompt.as_deref(),
            embedding_profile.as_deref(),
            seed_qualified_names.as_deref(),
            max_nodes,
            depth,
            max_incoming_per_node,
            embedding_policy.as_deref(),
            format.as_deref(),
            include_snippets,
            self.runtime.workspace_locks(),
            Some(self.runtime.search_cache()),
        )
        .await
    }
}

/// Tools whose schemas Claude Code loads into the prompt at session start; every
/// other tool stays deferred behind its tool search.
///
/// Why only these: with tool search on, an agent reaches for `grep` before it
/// spends a search call on loading a schema, and these three are what replaces
/// `grep` over code. Each costs a few hundred tokens up front; the whole set of
/// ~60 tools inline cost ~50k per session start, which is why deferral is on.
/// Graph tools (`who_calls`, …) are left out: they answer only after
/// `build_hypergraph`, so they are not a cold-start grep replacement.
///
/// Mechanism: Claude Code reads `_meta["anthropic/alwaysLoad"] = true` on a
/// listed tool as "never defer this one", independent of the server-level
/// `alwaysLoad` in `.mcp.json`. Other clients ignore the key.
const EAGER_TOOLS: &[&str] = &["search", "find_definition", "find_references"];

fn tools_with_load_hints(mut tools: Vec<rmcp::model::Tool>) -> Vec<rmcp::model::Tool> {
    for tool in &mut tools {
        if EAGER_TOOLS.contains(&tool.name.as_ref()) {
            let mut meta = rmcp::model::Meta::new();
            meta.0
                .insert("anthropic/alwaysLoad".into(), serde_json::Value::Bool(true));
            tool.meta = Some(meta);
        }
    }
    tools
}

/// `call_tool` and `list_tools` are written out rather than generated by
/// `#[tool_handler]`: the macro always emits both, and `call_tool` is the one
/// door every tool passes through — the place to log what each call did to
/// the daemon's memory (see `rmc_engine::mem_trace`). `list_tools` is the
/// macro's body verbatim.
impl ServerHandler for SearchToolRouter {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParam,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // The directory is named because one daemon serves every checkout: a
        // call against a workspace nobody else uses loads its own analysis.
        let directory = request
            .arguments
            .as_ref()
            .and_then(|args| args.get("directory"))
            .and_then(|dir| dir.as_str())
            .map(|dir| format!(" {dir}"))
            .unwrap_or_default();
        let mem =
            rmc_engine::mem_trace::MemSpan::start(format!("tool {}{}", request.name, directory));
        let tool = request.name.clone();
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        // rmcp sends the response from inside the task it spawned for this
        // request, so a panic that leaves this future sends none: the client
        // would wait out its timeout. See `panic_boundary`.
        let result =
            crate::panic_boundary::answer_or_error(&tool, self.tool_router.call(tcc)).await;
        mem.finish();
        result
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, McpError> {
        Ok(rmcp::model::ListToolsResult::with_all_items(
            tools_with_load_hints(self.tool_router.list_all()),
        ))
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            // Loaded into every client session at start: keep it to the
            // contracts shared by many tools. Per-tool detail lives in TOOLS.md.
            instructions: Some("Rust code intelligence server: search, definitions, references, renames, dependencies, call graphs, semantic similarity, persisted hypergraph queries, workspace audits, cache/index maintenance. Every path parameter (`directory`, `file_path`, …) must be ABSOLUTE: one daemon serves every working directory and shares no cwd with your session; relative paths are refused. `directory` is the workspace root (the directory with Cargo.toml). List tools page with `limit` (default 50), `offset` and `summary` (drops bulky per-item fields); the list is sorted first, then sliced, and the response carries `total_match_count` — more pages exist while it exceeds `offset + returned`. Graph and audit tools need `build_hypergraph` first. Full per-tool notes: TOOLS.md in the server repository."
                .into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_creation() {
        let _router = SearchToolRouter::new();
        // Just verify it can be created
        assert!(true);
    }

    /// Every name in EAGER_TOOLS is a real tool and carries the hint on the
    /// wire; a renamed tool would otherwise silently fall back to deferred.
    #[test]
    fn eager_tools_exist_and_carry_always_load() {
        let tools = tools_with_load_hints(SearchToolRouter::new().tool_router.list_all());
        for name in EAGER_TOOLS {
            let tool = tools
                .iter()
                .find(|t| t.name == *name)
                .unwrap_or_else(|| panic!("EAGER_TOOLS names unknown tool `{name}`"));
            let wire = serde_json::to_value(tool).unwrap();
            assert_eq!(wire["_meta"]["anthropic/alwaysLoad"], true, "{name}");
        }
        let marked = tools.iter().filter(|t| t.meta.is_some()).count();
        assert_eq!(marked, EAGER_TOOLS.len(), "only EAGER_TOOLS are marked");
    }

    #[test]
    fn test_router_with_sync_manager() {
        use std::sync::Arc;
        let sync_mgr = Arc::new(crate::mcp::SyncManager::new(300));
        let _router = SearchToolRouter::with_sync_manager(sync_mgr);
        assert!(_router.runtime.sync_manager().is_some());
    }

    /// A tool that panics on the async side answers with an error, and the
    /// connection keeps serving. rmcp sends a response from inside the task it
    /// spawned for the request, so without the catch in `call_tool` the panic
    /// took the response with it and the client waited out its own timeout —
    /// here, the 10 s bound below.
    ///
    /// Raw JSON-RPC lines over a duplex pipe against the real `serve`, so what
    /// is checked is what a client reads off the wire.
    #[tokio::test]
    async fn a_panicking_tool_answers_with_an_error_and_the_server_keeps_serving() {
        use rmcp::ServiceExt;
        use rmcp::handler::server::router::tool::ToolRoute;
        use serde_json::{Value, json};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mut router = SearchToolRouter::new();
        router.tool_router.add_route(ToolRoute::new_dyn(
            rmcp::model::Tool::new("panic_probe", "panics", std::sync::Arc::new(serde_json::Map::new())),
            |_| Box::pin(async { panic!("boom in a tool") }),
        ));
        let (server_io, client_io) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(async move {
            let service = router.serve(server_io).await.expect("handshake");
            let _ = service.waiting().await;
        });

        let (reader, mut writer) = tokio::io::split(client_io);
        let mut lines = BufReader::new(reader).lines();
        let mut send = async |message: Value| {
            writer.write_all(format!("{message}\n").as_bytes()).await.unwrap();
        };
        send(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" } } }))
        .await;
        let mut answer = async |id: u64| loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
                .await
                .expect("an answer, not a client timeout")
                .unwrap()
                .expect("the connection is open");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == json!(id) {
                return value;
            }
        };
        answer(1).await;
        send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;

        send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "panic_probe", "arguments": {} } }))
        .await;
        let failed = answer(2).await;
        let message = failed["error"]["message"].as_str().unwrap_or_default().to_owned();
        assert!(
            message.contains("panic_probe") && message.contains("boom in a tool"),
            "the error names the tool and the panic: {failed}"
        );

        // Same connection, next request: the server is still there.
        send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" })).await;
        assert!(answer(3).await["result"]["tools"].is_array());
        server.abort();
    }
}
