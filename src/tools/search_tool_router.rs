//! Search tool router module
//!
//! This module provides the MCP tool routing layer for all search and analysis tools.
//! It acts as a facade, delegating to specialized modules while handling MCP protocol
//! concerns like parameter validation and response formatting.
//!
//! ## Overview
//!
//! The `SearchToolRouter` is the main entry point for MCP clients. It:
//! - Routes tool calls to specialized implementations
//! - Handles parameter validation and error formatting
//! - Manages optional sync manager for background indexing
//! - Exposes 10 tools via MCP protocol
//!
//! ## Architecture
//!
//! ```text
//! MCP Client
//!     ↓
//! SearchToolRouter (this module)
//!     ├─→ query_tools      (search, get_similar_code, read_file_content)
//!     ├─→ analysis_tools   (find_*, get_*, analyze_complexity)
//!     ├─→ index_tool       (index_codebase)
//!     └─→ health_tool      (health_check)
//! ```
//!
//! ## Exposed MCP Tools
//!
//! 1. **read_file_content** - Read file contents
//! 2. **search** - Hybrid keyword + semantic search
//! 3. **get_similar_code** - Semantic similarity search
//! 4. **find_definition** - Locate symbol definitions
//! 5. **find_references** - Find all symbol references
//! 6. **get_dependencies** - Analyze file imports
//! 7. **get_call_graph** - Function call relationships
//! 8. **analyze_complexity** - Code complexity metrics
//! 9. **health_check** - System health status
//! 10. **index_codebase** - Manual indexing with change detection
//! 11. **clear_cache** - Clear corrupted cache/index files
//!
//! ## Refactoring Notes
//!
//! This module is the result of Phase 1 refactoring, which split the monolithic
//! `search_tool.rs` (1000 LOC, 242 cyclomatic complexity) into focused modules:
//! - Reduced router to ~200 LOC with routing-only logic
//! - Extracted business logic to specialized modules
//! - Achieved 90% complexity reduction
//! - Maintained full backward compatibility
//!
//! ## Examples
//!
//! ### Basic Usage
//! ```rust,no_run
//! use rust_code_mcp::tools::search_tool_router::SearchToolRouter;
//!
//! // Create router without sync manager
//! let router = SearchToolRouter::new();
//! ```
//!
//! ### With Background Sync
//! ```rust,no_run
//! use rust_code_mcp::tools::search_tool_router::SearchToolRouter;
//! use rust_code_mcp::mcp::SyncManager;
//! use std::sync::Arc;
//!
//! // Create router with background sync
//! let sync_mgr = Arc::new(SyncManager::new(300  // 5-minute sync interval
//! ));
//! let router = SearchToolRouter::with_sync_manager(sync_mgr);
//! ```

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

// Re-export parameter types from search_tool for compatibility
pub use crate::tools::search_tool::{
    AnalyzeComplexityParams, FileContentParams, FindDefinitionParams, FindReferencesParams,
    GetCallGraphParams, GetDependenciesParams, GetSimilarCodeParams, RenameSymbolParams,
    SearchParams,
};

/// Main tool router struct
#[derive(Clone)]
pub struct SearchToolRouter {
    tool_router: ToolRouter<Self>,
    /// Optional sync manager for automatic directory tracking
    sync_manager: Option<std::sync::Arc<crate::mcp::SyncManager>>,
}

impl SearchToolRouter {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            sync_manager: None,
        }
    }

    /// Create a new SearchToolRouter with background sync manager
    pub fn with_sync_manager(sync_manager: std::sync::Arc<crate::mcp::SyncManager>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            sync_manager: Some(sync_manager),
        }
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
        crate::tools::query_tools::read_file_content(&file_path).await
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
        crate::tools::query_tools::search(
            &directory,
            &keyword,
            embedding_profile.as_deref(),
            self.sync_manager.as_ref(),
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
        crate::tools::analysis_tools::find_definition_with_options(
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
        crate::tools::analysis_tools::find_references_with_options(
            &symbol_name,
            &directory,
            exact.unwrap_or(false),
        )
        .await
    }

    /// Preview a rename of a symbol across the project (read-only, no files modified)
    #[tool(
        description = "Preview renaming a Rust symbol project-wide using rust-analyzer. Returns the set of edits and file moves WITHOUT modifying any files."
    )]
    async fn rename_symbol(
        &self,
        Parameters(RenameSymbolParams {
            symbol_name,
            new_name,
            directory,
        }): Parameters<RenameSymbolParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::rename_symbol(&symbol_name, &new_name, &directory).await
    }

    /// Get dependencies for a file (imports and files that depend on it)
    #[tool(description = "Get import dependencies for a Rust source file")]
    async fn get_dependencies(
        &self,
        Parameters(GetDependenciesParams { file_path }): Parameters<GetDependenciesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::get_dependencies(&file_path).await
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
        crate::tools::analysis_tools::get_call_graph(&file_path, symbol_name.as_deref()).await
    }

    /// Analyze code complexity metrics for a file
    #[tool(
        description = "Analyze code complexity metrics (LOC, cyclomatic complexity, function count)"
    )]
    async fn analyze_complexity(
        &self,
        Parameters(AnalyzeComplexityParams { file_path }): Parameters<AnalyzeComplexityParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::analyze_complexity(&file_path).await
    }

    /// Check system health status
    #[tool(
        description = "Check the health status of the code search system (BM25, Vector store, Merkle tree)"
    )]
    async fn health_check(
        &self,
        Parameters(params): Parameters<crate::tools::health_tool::HealthCheckParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::health_tool::health_check(Parameters(params)).await
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
        crate::tools::query_tools::get_similar_code(
            &query,
            &directory,
            limit,
            embedding_profile.as_deref(),
        )
        .await
    }

    /// Manually index a codebase directory with automatic change detection
    #[tool(
        description = "Manually index a codebase directory (incremental indexing with Merkle tree change detection)"
    )]
    async fn index_codebase(
        &self,
        Parameters(params): Parameters<crate::tools::index_tool::IndexCodebaseParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::index_tool::index_codebase(params, self.sync_manager.as_ref()).await
    }

    /// Clear corrupted cache, index, and vector store files
    #[tool(description = "Clear corrupted cache files to fix 'Failed to open MetadataCache' errors. Clears metadata cache, tantivy index, and vector store. Pass include_hypergraph=true to ALSO wipe the persisted hypergraph snapshot at <data_dir>/graphs/<workspace_hash>/ — forces the next build_hypergraph call to do a full re-index. The response lists exactly which directories were cleared.")]
    async fn clear_cache(
        &self,
        Parameters(params): Parameters<crate::tools::clear_cache_tool::ClearCacheParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::clear_cache_tool::clear_cache(params).await
    }

    // ----- Hypergraph tools (Layer 7) -----

    #[tool(
        description = "Build or reuse a persisted workspace hypergraph snapshot (HIR-driven, no_deps=true)"
    )]
    async fn build_hypergraph(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::BuildHypergraphParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::build_hypergraph(params).await
    }

    #[tool(
        description = "List `use`/extern-crate imports in a module from the persisted hypergraph"
    )]
    async fn get_imports(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::GraphImportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::get_imports(params).await
    }

    #[tool(
        description = "List items declared in or re-exported from a module that are visible from a given consumer module"
    )]
    async fn get_exports(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::GraphExportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::get_exports(params).await
    }

    #[tool(
        description = "List re-exports (the subset of get_exports that came via `pub use`) visible from a given consumer module"
    )]
    async fn get_reexports(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::GraphReexportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::get_reexports(params).await
    }

    #[tool(
        description = "List every explicit `pub use` (or `pub(crate)` / `pub(in path)` / `pub(super)`) declared in a module, regardless of whether it's reachable from any specific consumer. Use this to audit a module's declared re-export surface; for visibility-filtered re-exports use get_reexports instead."
    )]
    async fn get_declared_reexports(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::GraphDeclaredReexportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::get_declared_reexports(params).await
    }

    #[tool(
        description = "Find every workspace module that imports the given symbol (matched by qualified name)"
    )]
    async fn who_imports(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::WhoImportsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::who_imports(params).await
    }

    #[tool(
        description = "List every non-import reference to the given symbol (file path + byte range + read/write/test category). Complements who_imports, which only enumerates `use` edges."
    )]
    async fn who_uses(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::WhoUsesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::who_uses(params).await
    }

    #[tool(
        description = "Aggregation rollup of who_uses: every non-import reference to the given symbol, grouped by consumer module, with total count + per-category breakdown (Read/Write/Test/Other). Same caveat as who_uses: cross-crate method calls and trait dispatch are NOT included (Layer 4 limitation)."
    )]
    async fn who_uses_summary(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::WhoUsesSummaryParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::who_uses_summary(params).await
    }

    #[tool(
        description = "Layer 10 call graph: every non-import reference to the target function whose call site sits inside another function body. Returns (caller_qualified_name, file, byte range, category) per call site. References in const initializers, type aliases, and other non-function scopes are excluded — use `who_uses` to see all reference sites including those. Calls from closures attribute to the enclosing fn."
    )]
    async fn who_calls(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::WhoCallsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::who_calls(params).await
    }

    #[tool(
        description = "Layer 10 call graph: every non-import reference made from the body of the caller function. Returns (callee_qualified_name, file, byte range, category) per outgoing reference. References in const initializers, type aliases, and other non-function scopes are excluded — use `who_uses` to see all reference sites including those. Calls from closures attribute to the enclosing fn."
    )]
    async fn calls_from(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::CallsFromParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::calls_from(params).await
    }

    #[tool(
        description = "Bounded recursive descent over outgoing call edges from `root`. Returns a tree of CallGraphNode { fn_qualified_name, crate_name, callees, truncated_at_cycle, truncated_at_depth }. `depth` defaults to 3 and is capped at 8 (deeper trees rarely fit usefully in a single response and may explode). `truncated_at_cycle = true` means the fn was already expanded earlier in the traversal — its callees are visible elsewhere in the tree. `truncated_at_depth = true` means depth ran out at this node and there were unvisited callees."
    )]
    async fn call_graph(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::CallGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::call_graph(params).await
    }

    #[tool(
        description = "`who_calls(target)` filtered to call sites whose *caller fn* lives in the named crate. Note: this filters by the caller's crate, not the target's. Useful for asking 'which fns inside crate X call Y?' regardless of which crate Y lives in."
    )]
    async fn callers_in_crate(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::CallersInCrateParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::callers_in_crate(params).await
    }

    #[tool(
        description = "Reverse BFS from `target`: counts distinct caller fns reachable backward up to `depth` hops. Returns { direct_callers, transitive_callers, depth_reached, truncated_at_depth }. Counts *fns*, not call sites — a fn that calls target 5 times counts as 1 caller. `depth` defaults to 3 and is capped at 8. depth=0 returns zeros; depth=1 is just the direct caller count."
    )]
    async fn recursive_callers_count(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::RecursiveCallersCountParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::recursive_callers_count(params).await
    }

    #[tool(
        description = "Scan a local crate for `pub` items with no cross-crate importer or reference — candidates for downgrading to `pub(crate)`. Conservative: may miss items used only through public type signatures."
    )]
    async fn dead_pub_in_crate(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::DeadPubParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::dead_pub_in_crate(params).await
    }

    #[tool(
        description = "Run dead_pub_in_crate over every local crate in the workspace and return a single aggregated report. Each finding includes file path + byte span so callers can navigate directly to the declaration."
    )]
    async fn dead_pub_report(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::DeadPubReportParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::dead_pub_report(params).await
    }

    #[tool(
        description = "All cross-crate consumer→producer edges in the workspace, with the symbols carrying each edge (sorted by total ref count desc). NOTE: cross-crate method calls and trait method dispatch are NOT captured in usage counts — Layer 4 doesn't extract impl-block items as Item nodes, so usage_count reflects only references to module-level items."
    )]
    async fn crate_edges(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::CrateEdgesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::crate_edges(params).await
    }

    #[tool(
        description = "Architectural-rule check: pure filter over crate_edges. Each rule has glob-style `consumer` and `producer` patterns (with `*` wildcards) matched against crate names, plus optional `consumer_kinds` (defaults to [`lib`, `bin`]), `except` (consumer-side override), `severity`, and `message`. Returns one violation per (rule × matching edge), each with sample_symbol/unique_symbols/total_refs for the offending edge. Same caveat as crate_edges: cross-crate method calls / trait dispatch are NOT counted."
    )]
    async fn forbidden_dependency_check(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::ForbiddenDependencyCheckParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::forbidden_dependency_check(params).await
    }

    #[tool(
        description = "Enumerate the variants of an enum: returns one row per variant with display_name, qualified_name, and (file, byte span) so callers can navigate to the declaration. `target` is the enum's qualified name (e.g. `my_crate::ErrorKind`). Use this with who_uses(MyEnum::SomeVariant) to investigate per-variant fan-in."
    )]
    async fn enum_variants(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::EnumVariantsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::enum_variants(params).await
    }

    #[tool(
        description = "Outer attributes and doc-comment lines recorded for the Item at `target`. Returns the trimmed source text of each `#[...]` attribute (e.g. `#[derive(Debug, Clone)]`, `#[must_use]`, `#[non_exhaustive]`, `#[inline]`) and each doc-comment line as `/// ...` (one entry per line). Source order preserved. Empty list when the item has no attributes or its AST source can't be resolved."
    )]
    async fn item_attributes(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::ItemAttributesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::item_attributes(params).await
    }

    #[tool(
        description = "Find every Item in the named crate whose attribute list has at least one entry matching `attribute_pattern`. Bare attribute paths such as `derive`, `must_use`, and `cfg` match `#[derive(...)]` / `#[must_use]`; wrapped prefix forms like `#[derive(` still work. Doc-comment patterns match against the **body** of a `///` line, so `SAFETY` matches `/// SAFETY: ...`. Anchoring avoids false positives where the pattern text appears mid-attribute — e.g. searching `must_use` no longer matches `#[tool(description = \"...#[must_use]...\")]` whose body merely mentions it. Each result row carries `match_location: \"attr\"` or `\"doc\"` so callers can filter visually. Empty pattern returns no results."
    )]
    async fn items_with_attribute(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::ItemsWithAttributeParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::items_with_attribute(params).await
    }

    #[tool(
        description = "Heuristic audit: every `pub type` alias in the named crate whose owning module also carries a `pub use ... as <alias_name>` (or `pub use ::<alias_name>`) binding. Indicates the alias may be acting as a re-export disguised as a `pub type` declaration. The model does NOT record what an alias's RHS resolves to, so this query cannot confirm the `pub use` and `pub type` point at the same target — verify with `find_definition` before acting."
    )]
    async fn pub_use_pub_type_audit(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::PubUsePubTypeAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::pub_use_pub_type_audit(params).await
    }

    #[tool(
        description = "Walk every `pub use` re-export of `target` (and every re-export of those re-exports) up to 8 hops with cycle detection. Returns one link per visited binding, breadth-first, with the from-module qualified name, visible_name, and depth. Useful for auditing the public surface of a type — i.e. \"every place `Token` is re-exported and the canonical declaration\"."
    )]
    async fn re_export_chain(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::ReExportChainParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::re_export_chain(params).await
    }

    #[tool(
        description = "Per-local-crate Robert Martin instability metric plus an abstractness ratio. `efferent` (Ce) = distinct outgoing producer crates; `afferent` (Ca) = distinct incoming consumer crates; `instability = Ce / (Ce + Ca)` (0 = max stable, 1 = max unstable). `abstractness = (traits + pub_type_aliases) / total_items`. Both metrics are NaN-guarded — degenerate counts return 0.0. `crate_id` is rendered as a 64-char hex string. Single-number health metric for refactor decisions. Optional knobs: `sort_by` accepts `instability`, `item_count`, `afferent`, `efferent`, `abstractness` (all descending; unknown values produce an `invalid_params` error); `top_n` caps returned rows after sorting (default: all rows)."
    )]
    async fn crate_dependency_metric(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::CrateDependencyMetricParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::crate_dependency_metric(params).await
    }

    #[tool(
        description = "Workspace-wide name-collision report: cross-crate type collisions, module names that shadow another crate, within-crate type duplicates, and fn names that appear in 4+ crates."
    )]
    async fn overlaps(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::OverlapsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::overlaps(params).await
    }

    #[tool(
        description = "Recursive module/item tree dump rooted at the specified crate. `depth` limits recursion below the root."
    )]
    async fn module_tree(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::ModuleTreeParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::module_tree(params).await
    }

    #[tool(
        description = "Workspace-wide counters: nodes by kind, items by ItemKind, bindings by BindingKind, declared-binding visibility breakdown, and pub_crate/total_items encapsulation ratio."
    )]
    async fn workspace_stats(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::WorkspaceStatsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::workspace_stats(params).await
    }

    #[tool(
        description = "Phase 5 (v9): return the recorded FunctionSignature for a function (free fn, inherent assoc fn, trait declaration fn). Carries is_async, self_param (Owned/Ref/RefMut, or null for free fns), params (name + stringified type + by_ref + mutability), return_type, and generic type parameters with their declaration-site trait bounds. Type strings come from RA's HirDisplay rendered against the function's owning crate; anonymous lifetimes ('_) are suppressed by default. Allocator and hasher type parameters (`, Global>`, `, RandomState>`, `, BuildHasherDefault<...>>`) and `LazyLock`/`OnceLock` init-fn pointer parameters are stripped from rendered types for readability. Returns `signature: null` when the target isn't a fn or extraction skipped it. Note: trait_bounds reflects the parameter's declaration-site bounds only — where-clause bounds added later are NOT included (RA limitation)."
    )]
    async fn function_signature(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::FunctionSignatureParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::function_signature(params).await
    }

    #[tool(
        description = "Phase 5 (v9): every local function in the named crate whose recorded FunctionSignature matches every Some field of the filter. Substring matches (`has_param_type`, `returns_type_pattern`) are case-sensitive against HirDisplay strings. `self_kind` accepts \"none\" | \"owned\" | \"ref\" | \"ref_mut\" — \"none\" matches free fns and assoc fns without self. Allocator and hasher type parameters (`, Global>`, `, RandomState>`, `, BuildHasherDefault<...>>`) and `LazyLock`/`OnceLock` init-fn pointer parameters are stripped from rendered types for readability (same trim as `function_signature`). Sorted by qualified name. Trait-impl method bodies are NOT included (mirrors the impls.rs exclusion). Pagination/summary knobs: `limit` (default 50) caps returned matches, `offset` (default 0) skips matches, and `summary` (default false) drops the per-match `signature` payload (returns just `target`/`qualified_name`) — useful when the full payload exceeds the MCP token budget. The response always carries `total_match_count` (unfiltered total before slicing); compare to `offset + match_count` to detect that more pages exist."
    )]
    async fn functions_with_filter(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::FunctionsWithFilterParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::functions_with_filter(params).await
    }

    #[tool(
        description = "Phase 6: query-time audit of every `unsafe { ... }` block in the workspace's local crates. Walks each `.rs` file's syntax tree (no semantic analysis beyond enclosing-fn lookup), returning per-block: workspace-relative file path, byte span of the unsafe expression (curlies included), source line count, enclosing function (NodeId rendered as a 64-char hex string + qualified name when resolvable, null for unsafe blocks in const initializers / trait bounds / closures-without-fn-parent), and a `has_safety_comment` heuristic flag (true when `SAFETY` appears as a substring in any of the 5 source lines preceding the `unsafe` keyword). Live computation; nothing cached — per-invocation cost is dominated by the workspace load (~2-3s). Sorted by (file, span). Use this to find unsafe blocks missing `// SAFETY:` comments, audit unsafe-block size distribution, or build an unsafe-code inventory for safety-critical review."
    )]
    async fn unsafe_audit(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::UnsafeAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::unsafe_audit(params).await
    }

    #[tool(
        description = "Phase 7 Path B (v10): type-aware audit of every local `static` item that matches a known global-mutable-state pattern. Reads the static's HIR type via HirDisplay (no source-text regex) and classifies against `static mut`, `LazyLock<...>`, `OnceLock<...>`, `OnceCell<...>`. A single static matching multiple patterns produces one finding per pattern. Each finding carries item NodeId (rendered as a 64-char hex string), qualified_name, matched_pattern, type_string (for human inspection), file, and byte span. The `type_string` is post-processed via the same HirDisplay trim as `function_signature` — e.g. `LazyLock<Mutex<Foo>, fn() -> Mutex<Foo>>` becomes `LazyLock<Mutex<Foo>>` (init-fn pointer dropped). Sorted by (qualified_name, matched_pattern). Limitation: the `lazy_static!` macro is NOT detected — its expansion produces a generated wrapper type whose name doesn't contain `LazyLock`. Use `items_with_attribute` or grep to cover that case. Use this to find hidden singleton config / auth / clocks / RNGs and to enforce 'avoid global mutable state' guidelines."
    )]
    async fn mut_static_audit(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::MutStaticAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::mut_static_audit(params).await
    }

    #[tool(
        description = "Phase 8: query-time audit of every pure-`pub` local Item that carries no `///` doc-comment in its extracted attributes. Pure read-side query on the v11 snapshot's `node.attributes` (populated at build time) plus the declaring `Binding`'s visibility — no AST walk, no fresh RA load. Filters: optional `crate_name` (qualified name; accepts a Crate or its root Module), optional `item_kind` (list — defaults to all 'documentable' kinds: Function, Struct, Enum, Union, Trait, TypeAlias, Const, Static, Method; excludes EnumVariant / AssocConst / AssocType which rarely carry standalone docs), and `skip_test_items` (default true — drops items whose qualified name contains `::tests::`). Only pure `pub` Items are flagged; `pub(crate)` / `pub(in path)` / private are skipped per §10 ('pub(crate) is internal API'). Each finding carries target NodeId (64-char hex), qualified_name, item_kind, visibility (always `\"pub\"`), file, and byte span. Sorted by (file, span). Use this to find public items missing rustdoc and to enforce §16 documentation discipline."
    )]
    async fn missing_docs_audit(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::MissingDocsAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::missing_docs_audit(params).await
    }

    #[tool(
        description = "Phase 8: query-time audit of every pure-`pub` local Struct / Enum / Union that is missing one or more required derive macros. Pure read-side query on the v11 snapshot's `node.attributes` (Phase 1 populated at build time) plus the declaring `Binding`'s visibility — no AST walk, no fresh RA load. Filters: optional `crate_name` (qualified name; accepts a Crate or its root Module), optional `item_kind` (list — any subset of [\"Struct\", \"Enum\", \"Union\"]; default all three), `pub_only` (default true — only audits the public surface per §8 'Debug almost always'), and `skip_test_items` (default true — drops items whose qualified name contains `::tests::`). `required_derives` is mandatory: a non-empty list (e.g. [\"Debug\"] or [\"Debug\", \"Clone\", \"PartialEq\"]). The derive parser strips path qualifiers — `serde::Serialize` and `::std::fmt::Debug` both match `Serialize` / `Debug` in `required_derives`. Each finding carries target NodeId (64-char hex), qualified_name, item_kind, visibility, file, byte span, the item's `current_derives`, and the `missing_derives` (set difference). Sorted by (file, span). Use this to enforce §8 standard-derive coverage."
    )]
    async fn derive_audit(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::DeriveAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::derive_audit(params).await
    }

    #[tool(
        description = "Phase 8: pure read-side audit listing every fn participating in a recursion cycle (self-recursion or mutual recursion). Walks the Layer 10 call graph data from `signatures_by_target` (every fn) + `usages_by_consumer_function` (caller fn → outgoing call sites), running a bounded DFS up to `max_cycle_length` from each fn. Cycles are canonicalized (rotated so the lowest-id NodeId comes first) so the same cycle viewed from different starting nodes counts once. Filters: optional `crate_name` (qualified name; accepts a Crate or its root Module — a cycle is included if at least one of its members is in the requested crate, which is a deliberately looser filter than 'all members in crate'); `max_cycle_length` defaults 5 and is clamped to [1, 12]. Each cycle reports `fns` (qualified names in cycle order, lowest-id first), `cycle_length`, `direct_recursion` (true iff length == 1), and `starting_node_id` (64-char hex of the lowest-id member). Sorted by `(cycle_length asc, qualified_name)`. Use this to enforce §22 'no recursion in critical paths'."
    )]
    async fn recursion_check(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::RecursionCheckParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::recursion_check(params).await
    }

    #[tool(
        description = "Phase 8: query-time AST-walk audit of every channel-construction call site in the workspace's local crates. Loads the workspace through rust-analyzer (~2-3s, dominates per-call cost), iterates every local module's source file, walks the syntax tree for `CallExpr` nodes, and resolves each call's path through `Semantics::resolve_path` so aliased imports such as `use tokio::sync::mpsc; mpsc::channel(N)` still match the canonical entry. Matches the hardcoded v1 path table: `tokio::sync::mpsc::channel` (bounded), `tokio::sync::mpsc::unbounded_channel`, `std::sync::mpsc::channel` (legacy unbounded — flag), `std::sync::mpsc::sync_channel` (bounded), `crossbeam_channel::bounded`, `crossbeam_channel::unbounded`, `flume::bounded`, `flume::unbounded`. Per finding: workspace-relative crate name, `kind` (one of the 8 labels above), `bounded` flag, `capacity` (Some(N) for a literal int arg with `_` separators allowed, None for a const / variable / arithmetic expression / unbounded constructor), file, byte span of the call expression, and enclosing fn (NodeId rendered as 64-char hex + qualified name when resolvable; null for calls in const initializers / closures-without-fn-parent). Filters: optional `crate_name` (qualified name; accepts a Crate or its root Module), `skip_test_fns` (default true — drops findings whose enclosing fn / module carries `#[cfg(test)]`). Sorted by (file, span). Use this to inventory channel construction across the workspace, enforce §12 'use bounded channels', and surface unbounded-channel call sites for review."
    )]
    async fn channel_capacity_audit(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::ChannelCapacityAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::channel_capacity_audit(params).await
    }

    #[tool(
        description = "Phase 8: query-time AST-walk audit of every fn body in the workspace's local crates against eight built-in pattern matchers covering rust-guidelines §3, §9, §12, §19, §22. Loads the workspace through rust-analyzer (~2-3s, dominates per-call cost), iterates every local module's source file, walks each `fn`'s body, and emits one finding per pattern hit. Patterns: 1) `unwrap` — any `MethodCallExpr` named `unwrap` (§9 'Avoid `unwrap()` in production paths'); 2) `expect` — same but `expect` (§9); 3) `panic_macros` — `panic!` / `unreachable!` / `todo!` / `unimplemented!` invocations (§9 'Use `panic!` for bugs only'); 4) `unwrap_unchecked` — `unwrap_unchecked` / `unwrap_err_unchecked` (§19); 5) `transmute` — `CallExpr` resolving to `std::mem::transmute` or `core::mem::transmute` via `Semantics::resolve_path` (§19); 6) `await_in_guard_scope` — `.await` inside a block where a preceding `LetStmt`'s initializer or type contains a guard hint (`MutexGuard`, `RwLockReadGuard`, `RwLockWriteGuard`, bare `Guard`, `Ref<` / `RefMut<`, or `.lock()` / `.read()` / `.write()` call) (§12 'Never hold a lock across `.await`'); 7) `self_recursion` — call resolving (via `Semantics::resolve_path` for `CallExpr` and `Semantics::resolve_method_call` for `MethodCallExpr`) to the enclosing fn's canonical path (§22 'No recursion in critical paths'); 8) `unbounded_loop` — bare `loop {}` keyword form whose body has no `BreakExpr` / `ReturnExpr` / `?` `TryExpr` at any depth (§22 'Give loops clear upper bounds'). Per finding: enclosing fn (NodeId hex + qualified name when resolvable), pattern label, workspace-relative file, byte span, and a 1-3 line trimmed `context` snippet. Filters: optional `crate_name` (Crate / root Module qualified name), `patterns` (subset of the 8 labels — empty/null defaults to all 8), `skip_test_fns` (default true — drops findings inside `#[cfg(test)]` modules / fns). Sorted by `(file, span, pattern)`. Heuristic notes: `await_in_guard_scope` is a review trigger and accepts false positives (string-match on let-stmt text); `unbounded_loop` will flag legitimate event loops — disable via `patterns` if needed; `unwrap` matches any method named `unwrap`, not just `Result::unwrap` / `Option::unwrap`. Use this to enforce body-level guideline coverage. Requires `build_hypergraph` to have run (snapshot needed for enclosing-fn NodeId lookup)."
    )]
    async fn fn_body_audit(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::FnBodyAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::fn_body_audit(params).await
    }

    #[tool(
        description = "Find semantic neighbors of a hypergraph Item using vector embeddings. Resolves `target` (qualified name) via the persisted hypergraph, reads its source from the file at the recorded byte span, then runs vector_only_search using that source as the query. Returns ranked matches above `threshold` (default 0.0), capped at `limit` (default 10), optionally filtered by `item_kind` (case-insensitive match against the chunk's symbol_kind). Self-match (the seed's own chunk, detected by file path + line-range overlap with the seed's byte span) is dropped automatically. Useful for finding \"what looks like X?\" — e.g. duplicate error types, parser variants, or builder patterns. NOTE: requires both `build_hypergraph` AND `index_codebase` to have been called for the workspace."
    )]
    async fn similar_to_item(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::SimilarToItemParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::similar_to_item(params).await
    }

    #[tool(
        description = "Workspace-wide semantic-overlap audit. Enumerates Items (optionally scoped to a crate / item_kind), embeds each one's source, and builds a similarity graph above `threshold` (defaults to the embedding model's tuned cutoff — 0.85 for the default Qwen3 model), returning either deduplicated pairs (output_mode=\"pairs\") or single-linkage clusters of transitively-similar items (output_mode=\"clusters\", default). Self-matches and cross-test noise are filtered (skip_test_chunks default true). Pagination/output controls: `max_pairs` caps returned pairs in pairs mode or total emitted cluster members in clusters mode, `offset` skips pairs/clusters, and `summary=true` omits per-member file/span payloads. The response carries `total_pair_count` and `total_cluster_count` before pagination. v1.1: per-Item embedding cache + in-memory cosine — first scan pays the full embedding cost; subsequent scans on unchanged code are nearly free (cache lives in the snapshot's LMDB env at the `embeddings_by_target` sub-DB; `build_hypergraph --force_rebuild` clears it). Use this for offline duplicate-detection / refactor planning. NOTE: requires `build_hypergraph` to have been called for the workspace; the vector store / `index_codebase` is no longer required for this tool. Latency is seconds-to-minutes on first run, sub-second on cache-warm reruns."
    )]
    async fn semantic_overlaps(
        &self,
        Parameters(params): Parameters<crate::tools::search_tool::SemanticOverlapsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::semantic_overlaps(params).await
    }

    #[tool(description = "Build a task-conditioned subgraph (codemap) of the indexed workspace.

Returns nodes/edges/hierarchy focused on the prompt. Edges come from the
HIR-driven hypergraph: direct calls and non-import uses. Local trait
dispatch (`x.method()` where `method` is declared in a workspace-local
trait) IS captured — the call resolves back to the trait declaration's
Item. NOT captured: callers reaching an impl method through `dyn Trait`
over external traits, generic `F: Fn(..)` indirect calls, and resolution
to specific impl-method NodeIds via fully-qualified `<T as Trait>::m()`.
These blind spots are inherent to the underlying Usage extraction.

Tunable defaults: max_nodes=80 (cap 500), depth=3 (cap 5),
max_incoming_per_node=8, embedding_policy='no_rerank', format='json'.

Seed source: pass `task_prompt` for HybridSearch-driven seeds (requires
`index_codebase` to have populated the vector store / tantivy index), OR
`seed_qualified_names` for direct lookup. At least one of the two must
be supplied.

Choosing between them: `task_prompt` is best for exploratory queries
against documented APIs; the underlying hybrid search (BM25 + vector
embeddings against doc comments) favors public surfaces with rich
docstrings, and many search hits fail to snap to an indexed Item span
(reported in `Codemap.diagnostics`). For pinpoint navigation to a
specific implementation, prefer `seed_qualified_names`. Note that the
hypergraph indexes only `pub` and `pub(crate)` items — module-local
private functions can't be referenced by qualified name. Unresolved
names go to `Codemap.diagnostics`; if the leaf fails but its parent
module resolves, the diagnostic notes 'likely private or not indexed'
to distinguish that from a typo.

Output: `format='json'` returns the full `Codemap` JSON (nodes, edges,
hierarchy, stats, diagnostics). `format='mermaid'` returns a
`flowchart LR` rendering with seeds highlighted via a `:::seed` class.
`format='outline'` returns a flat indented text outline sorted by
qualified name. `format='all'` wraps the three under a single JSON
object. Requires `build_hypergraph` to have been called for the
workspace.")]
    async fn build_codemap(
        &self,
        Parameters(crate::tools::search_tool::BuildCodemapParams {
            directory,
            task_prompt,
            seed_qualified_names,
            max_nodes,
            depth,
            max_incoming_per_node,
            embedding_policy,
            format,
            include_snippets,
        }): Parameters<crate::tools::search_tool::BuildCodemapParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::graph_tools::handle_build_codemap(
            &directory,
            task_prompt.as_deref(),
            seed_qualified_names.as_deref(),
            max_nodes,
            depth,
            max_incoming_per_node,
            embedding_policy.as_deref(),
            format.as_deref(),
            include_snippets,
        )
        .await
    }
}

#[tool_handler]
impl ServerHandler for SearchToolRouter {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("Rust code intelligence server for searching code, reading files, resolving symbols and references, previewing renames, inspecting dependencies and call graphs, semantic similarity, persisted hypergraph queries, workspace audits, and cache/index maintenance. List-shaped graph and audit tools accept `limit` (default 50), `offset`, and `summary`; responses include `total_match_count` plus returned-count metadata. See each tool's description for parameters and limitations; run `build_hypergraph` before graph-backed tools that require it."
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

    #[test]
    fn test_router_with_sync_manager() {
        use std::sync::Arc;
        let sync_mgr = Arc::new(crate::mcp::SyncManager::new(300));
        let _router = SearchToolRouter::with_sync_manager(sync_mgr);
        assert!(_router.sync_manager.is_some());
    }
}
