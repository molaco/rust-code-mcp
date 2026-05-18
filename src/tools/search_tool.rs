//! Search tool compatibility wrapper
//!
//! This module provides backward compatibility by re-exporting the new modular structure.
//! All functionality has been split into focused modules:
//! - `indexing_tools`: Indexing operations
//! - `query_tools`: Search and query operations
//! - `analysis_tools`: Code analysis operations
//! - `search_tool_router`: MCP tool routing
//!
//! This wrapper ensures existing code using `SearchTool` continues to work.

use rmcp::schemars;

// Re-export the router as SearchTool for backward compatibility
pub use crate::tools::search_tool_router::SearchToolRouter as SearchTool;

// Re-export all parameter types for backward compatibility
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Path to the directory to search")]
    pub directory: String,
    #[schemars(description = "Keyword to search for")]
    pub keyword: String,
    #[schemars(
        description = "Optional embedding profile for vector search. One of: \"local-gpu-small\", \"local-cpu-small\", \"openrouter-qwen3-8b\", \"local-qwen3-4b\", \"local-qwen3-8b\"."
    )]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FileContentParams {
    #[schemars(description = "Path to the file to read")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindDefinitionParams {
    #[schemars(description = "Symbol name to find the definition for (function, struct, trait, const, etc.)")]
    pub symbol_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindReferencesParams {
    #[schemars(description = "Symbol name to find all references for")]
    pub symbol_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RenameSymbolParams {
    #[schemars(description = "Symbol name to rename (must match exactly; ambiguous names are rejected)")]
    pub symbol_name: String,
    #[schemars(description = "New name for the symbol (valid Rust identifier)")]
    pub new_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetDependenciesParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetCallGraphParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
    #[schemars(description = "Optional: specific symbol to get call graph for")]
    pub symbol_name: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AnalyzeComplexityParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSimilarCodeParams {
    #[schemars(description = "Code snippet or query to find similar code")]
    pub query: String,
    #[schemars(description = "Directory containing the codebase")]
    pub directory: String,
    #[schemars(description = "Number of similar results to return (default 5)")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Optional embedding profile for vector search. One of: \"local-gpu-small\", \"local-cpu-small\", \"openrouter-qwen3-8b\", \"local-qwen3-4b\", \"local-qwen3-8b\"."
    )]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BuildHypergraphParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Force a rebuild even if a snapshot for the current fingerprint already exists")]
    pub force_rebuild: Option<bool>,
}

#[derive(Debug, Default, Clone, Copy, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ListPaginationParams {
    #[schemars(description = "Optional cap on returned items after slicing. Default: 50.")]
    #[serde(default)]
    pub limit: Option<usize>,
    #[schemars(description = "Optional offset into the sorted item list, applied before `limit`. Default: 0.")]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Optional summary mode. When true, tools omit bulky per-item payload fields where applicable. Default: false.")]
    #[serde(default)]
    pub summary: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphImportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module qualified name, e.g. `my_crate::sub::module`")]
    pub module: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphExportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module to enumerate exports from (qualified name)")]
    pub module: String,
    #[schemars(description = "Consumer module from whose viewpoint visibility is checked (qualified name)")]
    pub consumer: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphReexportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module to enumerate re-exports from (qualified name)")]
    pub module: String,
    #[schemars(description = "Consumer module from whose viewpoint visibility is checked (qualified name)")]
    pub consumer: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphDeclaredReexportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module to enumerate explicit `pub use` declarations from (qualified name)")]
    pub module: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoImportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose importers you want")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoUsesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose non-import references you want (file:byte-range hits)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoUsesSummaryParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose non-import references you want, aggregated per consumer module with per-category counts")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoCallsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose callers you want (Layer 10 call graph)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CallsFromParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the caller function whose outgoing references you want (Layer 10 call graph)")]
    pub caller: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CallGraphParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the root function to descend from")]
    pub root: String,
    #[schemars(description = "Optional max recursion depth (default 3, capped at 8)")]
    pub depth: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CallersInCrateParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose callers you want")]
    pub target: String,
    #[schemars(description = "Qualified name of the crate to filter callers by (matches the *caller's* crate, not the target's)")]
    pub krate: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RecursiveCallersCountParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose transitive callers you want to count")]
    pub target: String,
    #[schemars(description = "Optional max BFS depth in caller hops (default 3, capped at 8)")]
    pub depth: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeadPubParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the local crate to scan (e.g. `my_crate`). Items declared `pub` with no cross-crate consumers are returned as candidates for downgrading to `pub(crate)`.")]
    pub krate: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeadPubReportParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml). Runs dead_pub_in_crate over every local crate and returns aggregated findings per crate.")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CrateEdgesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct OverlapsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

/// One architectural rule for `forbidden_dependency_check`. Patterns in
/// `consumer`, `producer`, and `except` are glob-style with `*` wildcards.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ForbiddenDependencyRuleParam {
    #[schemars(description = "Glob pattern matched against the consumer crate name (e.g. `domain*`)")]
    pub consumer: String,
    #[schemars(description = "Glob pattern matched against the producer crate name (e.g. `tokio`)")]
    pub producer: String,
    #[schemars(description = "Optional consumer-side glob exception: edges whose consumer matches this pattern are NOT flagged, even if `consumer`/`producer` match")]
    pub except: Option<String>,
    #[schemars(description = "Optional severity tag passed through to violations (e.g. `error` / `warn`)")]
    pub severity: Option<String>,
    #[schemars(description = "Optional human-readable rationale, passed through unchanged")]
    pub message: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ForbiddenDependencyCheckParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Architectural rules to enforce against the workspace's cross-crate edges")]
    pub rules: Vec<ForbiddenDependencyRuleParam>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EnumVariantsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the enum whose variants you want (e.g. `my_crate::module::MyEnum`)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ItemAttributesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the item whose outer attributes (and doc-comment lines) you want, e.g. `my_crate::Foo`")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ItemsWithAttributeParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scan (e.g. `my_crate`); accepts the crate root module name as an alias")]
    pub crate_name: String,
    #[schemars(description = "Substring to match against each item's attribute strings, e.g. `#[must_use]`, `must_use`, `derive(Debug`, or `/// SAFETY:`")]
    pub attribute_pattern: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PubUsePubTypeAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scan (e.g. `my_crate`); accepts the crate root module name as an alias")]
    pub crate_name: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReExportChainParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the canonical declaration whose re-export chain you want to walk (e.g. `my_crate::module::Token`)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CrateDependencyMetricParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional cap on returned rows after sorting. Default: None (all rows).")]
    #[serde(default)]
    pub top_n: Option<usize>,
    #[schemars(description = "Optional sort key applied before `top_n` slicing. One of `instability`, `item_count`, `afferent`, `efferent`, `abstractness` (all descending). Unknown values produce an `invalid_params` error.")]
    #[serde(default)]
    pub sort_by: Option<String>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ModuleTreeParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name (e.g. `my_crate`)")]
    pub krate: String,
    #[schemars(description = "Optional max depth below the crate root (None walks the full tree)")]
    pub depth: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WorkspaceStatsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FunctionSignatureParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the function (e.g. `crate::module::fn_name` or `crate::Type::method`)")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UnsafeAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MutStaticAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct MissingDocsAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Optional list of item kinds to audit (e.g. [\"Function\", \"Struct\", \"Enum\", \"Trait\", \"TypeAlias\", \"Const\", \"Static\"]). Default: all 'documentable' kinds (Function, Struct, Enum, Union, Trait, TypeAlias, Const, Static, Method) — excludes EnumVariant, AssocConst, AssocType which rarely carry standalone docs.")]
    #[serde(default)]
    pub item_kind: Option<Vec<String>>,
    #[schemars(description = "Drop items inside `::tests::` modules. Default true.")]
    #[serde(default)]
    pub skip_test_items: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct DeriveAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Item kind(s) to audit: any subset of [\"Struct\", \"Enum\", \"Union\"]. Default: all three.")]
    #[serde(default)]
    pub item_kind: Option<Vec<String>>,
    #[schemars(description = "Required derive identifiers (e.g. [\"Debug\"] or [\"Debug\", \"Clone\", \"PartialEq\"]). The canonical recommendation is `Debug` for almost every public type.")]
    pub required_derives: Vec<String>,
    #[schemars(description = "Only audit items whose visibility is `pub` (the §8 'Debug almost always' rule applies to the public surface). Default true.")]
    #[serde(default)]
    pub pub_only: Option<bool>,
    #[schemars(description = "Drop items inside `::tests::` modules. Default true.")]
    #[serde(default)]
    pub skip_test_items: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct RecursionCheckParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Maximum cycle length to detect. Default 5 (covers self-loop + indirect recursion through a few hops). Hard cap: 12.")]
    #[serde(default)]
    pub max_cycle_length: Option<usize>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ChannelCapacityAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Drop findings inside `#[cfg(test)]` modules / fns. Default true.")]
    #[serde(default)]
    pub skip_test_fns: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct FnBodyAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Patterns to check. Default: all 8 built-ins. Available: \"unwrap\", \"expect\", \"panic_macros\", \"unwrap_unchecked\", \"transmute\", \"await_in_guard_scope\", \"self_recursion\", \"unbounded_loop\".")]
    #[serde(default)]
    pub patterns: Option<Vec<String>>,
    #[schemars(description = "Drop findings inside `#[cfg(test)]` modules / fns. Default true.")]
    #[serde(default)]
    pub skip_test_fns: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FunctionsWithFilterParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scope the search (accepts the crate name or its root module)")]
    pub krate: String,
    #[schemars(description = "Optional minimum non-self param count")]
    #[serde(default)]
    pub min_param_count: Option<usize>,
    #[schemars(description = "Optional substring pattern that must appear in at least one param's stringified type")]
    #[serde(default)]
    pub has_param_type: Option<String>,
    #[schemars(description = "Optional substring pattern that must appear in the function's stringified return type")]
    #[serde(default)]
    pub returns_type_pattern: Option<String>,
    #[schemars(description = "Optional async filter — true to require `async fn`, false to require non-async")]
    #[serde(default)]
    pub is_async: Option<bool>,
    #[schemars(description = "Optional self-kind filter: \"none\" | \"owned\" | \"ref\" | \"ref_mut\"")]
    #[serde(default)]
    pub self_kind: Option<String>,
    #[schemars(description = "Optional cap on returned matches after slicing. Default: 50. Use together with `offset` to paginate. Compare `total_match_count` to `limit + offset` to detect more pages.")]
    #[serde(default)]
    pub limit: Option<usize>,
    #[schemars(description = "Optional offset into the (sorted) match list, applied before `limit`. Default: 0.")]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Optional summary mode. When `true`, each match drops the full `signature` payload, returning only `target` and `qualified_name`. Default: false.")]
    #[serde(default)]
    pub summary: Option<bool>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct SimilarToItemParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the seed Item (function, struct, enum, etc.)")]
    pub target: String,
    #[schemars(description = "Max number of results (default: 10)")]
    #[serde(default)]
    pub limit: Option<usize>,
    #[schemars(description = "Minimum cosine similarity score (0.0-1.0). Results below are dropped. Default: 0.0")]
    #[serde(default)]
    pub threshold: Option<f32>,
    #[schemars(description = "Restrict results to items of this kind, matching the chunk's symbol_kind (\"Function\", \"Struct\", \"Enum\", \"Trait\", etc.). Case-insensitive. Default: no filter.")]
    #[serde(default)]
    pub item_kind: Option<String>,
    #[schemars(description = "Embedding profile the codebase was indexed with (built-in name or a profile from embedding_profiles.toml). Must match the profile passed to `index_codebase`, since this tool reads that profile's vector index. Default: the built-in default model.")]
    #[serde(default)]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct SemanticOverlapsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Optional item-kind filter (\"Function\" | \"Struct\" | \"Enum\" | \"Trait\" | \"Method\"). Default: all kinds.")]
    #[serde(default)]
    pub item_kind: Option<String>,
    #[schemars(description = "Minimum cosine similarity (0.0-1.0). Omit to use the embedding model's tuned default cutoff (0.85 for the Qwen3 code-embedding model used by default). Cosine-similarity scales are model-specific, so an explicit value is interpreted relative to the active model: drop ~0.05 for crate-scoped scans where chaining is less of a problem; raise to 0.90+ for very strict \"definitely duplicate\" signal.")]
    #[serde(default)]
    pub threshold: Option<f32>,
    #[schemars(description = "Cap on returned pairs in pairs mode, or total emitted cluster members in clusters mode. Default 50.")]
    #[serde(default)]
    pub max_pairs: Option<usize>,
    #[schemars(description = "Optional offset into the sorted result list. In pairs mode this skips pairs; in clusters mode this skips clusters before the member cap is applied. Default 0.")]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Optional summary mode. When true, pair endpoints and cluster members omit file/span payloads and keep only qualified_name + item_kind. Default false.")]
    #[serde(default)]
    pub summary: Option<bool>,
    #[schemars(description = "Drop clusters whose member count exceeds this cap (single-linkage chaining produces large noisy clusters; default 15 trims them while keeping high-signal pair/trio clusters). Set to 0 to disable.")]
    #[serde(default)]
    pub max_cluster_size: Option<usize>,
    #[schemars(description = "Output mode: \"pairs\" (raw similarity edges) or \"clusters\" (single-linkage groups). Default \"clusters\".")]
    #[serde(default)]
    pub output_mode: Option<String>,
    #[schemars(description = "Drop matches whose qualified name contains `::tests::`. Default true.")]
    #[serde(default)]
    pub skip_test_chunks: Option<bool>,
    #[schemars(description = "Drop pairs whose two items share a crate. Default false.")]
    #[serde(default)]
    pub cross_crate_only: Option<bool>,
    #[schemars(description = "Embedding profile to embed items with (built-in name or a profile from embedding_profiles.toml). Selects the model and the similarity scale `threshold` is interpreted on; switching profiles re-embeds via the per-Item cache. Default: the built-in default model (Qwen3-Embedding-0.6B, local GPU).")]
    #[serde(default)]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BuildCodemapParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Natural-language task description. Required unless seed_qualified_names is supplied. Best for exploratory queries against documented APIs — HybridSearch weighs token frequency in doc comments, so verbose-doc public surfaces rank highest. For pinpoint navigation to a specific implementation, prefer seed_qualified_names. Search hits that don't snap to an indexed Item are surfaced in Codemap.diagnostics with per-failure-mode counts (path-norm, line-resolve, kind-filter).")]
    #[serde(default)]
    pub task_prompt: Option<String>,
    #[schemars(description = "Override seeds by qualified name. The hypergraph indexes only `pub` and `pub(crate)` items — module-local private functions and trait-impl methods are not stored as standalone nodes and can't be referenced this way. Names that fail to resolve are surfaced in Codemap.diagnostics rather than erroring out; if the leaf fails but its parent module resolves, the diagnostic notes 'likely private or not indexed'.")]
    #[serde(default)]
    pub seed_qualified_names: Option<Vec<String>>,
    #[schemars(description = "Maximum number of retained nodes. Default 80; capped at 500.")]
    #[serde(default)]
    pub max_nodes: Option<usize>,
    #[schemars(description = "BFS expansion depth from each seed. Default 3; capped at 5.")]
    #[serde(default)]
    pub depth: Option<u8>,
    #[schemars(description = "Per-node incoming-edge cap during BFS expansion. Default 8.")]
    #[serde(default)]
    pub max_incoming_per_node: Option<usize>,
    #[schemars(description = "Embedding-rerank policy: `no_rerank` (default) | `cached_only` | `compute_missing`.")]
    #[serde(default)]
    pub embedding_policy: Option<String>,
    #[schemars(description = "Output format: `json` (default) | `mermaid` | `outline` | `all`.")]
    #[serde(default)]
    pub format: Option<String>,
    #[schemars(description = "Include the first ~5 lines of source per node in the JSON/outline output. Default false.")]
    #[serde(default)]
    pub include_snippets: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_tool_backward_compat() {
        // Verify SearchTool can still be created
        let _tool = SearchTool::new();
        // Just verify construction works
        assert!(true);
    }
}
