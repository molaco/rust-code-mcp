//! Graph-query parameter structs.

use rmcp::schemars;

use rmc_graph::graph::ForbiddenDependencyRule;

use super::ListPaginationParams;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GraphExportParams {
    pub directory: String,
    #[schemars(description = "Omit for metadata. Otherwise nodes, bindings, usages, contains, unresolved_calls, signatures, statics or stub_owners.")]
    pub table: Option<String>,
    #[schemars(description = "Required for every table page: graph_id returned by metadata. A changed CURRENT is an error; restart export.")]
    pub graph_id: Option<String>,
    #[schemars(description = "Opaque JSON string of next_cursor from the preceding page; omit for first page of each table.")]
    pub cursor: Option<String>,
    #[schemars(description = "Rows per page, 1..=10000; default 100. No summary or truncation.")]
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GraphImportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module qualified name, e.g. `my_crate::sub::module`")]
    pub module: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ModuleDependenciesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module qualified name, e.g. `my_crate::sub::module`")]
    pub module: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GraphExportsParams {
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
pub(crate) struct GraphReexportsParams {
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
pub(crate) struct GraphDeclaredReexportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module to enumerate explicit `pub use` declarations from (qualified name)")]
    pub module: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct WhoImportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose importers you want")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct WhoUsesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose non-import references you want (file:byte-range hits)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct WhoUsesSummaryParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose non-import references you want, aggregated per consumer module with per-category counts")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct WhoCallsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose callers you want (Layer 10 call graph)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct CallsFromParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the caller function whose outgoing references you want (Layer 10 call graph)")]
    pub caller: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct CallGraphParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the root function to descend from")]
    pub root: String,
    #[schemars(description = "Optional max recursion depth (default 3, capped at 8)")]
    pub depth: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct CallersInCrateParams {
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
pub(crate) struct RecursiveCallersCountParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose transitive callers you want to count")]
    pub target: String,
    #[schemars(description = "Optional max BFS depth in caller hops (default 3, capped at 8)")]
    pub depth: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct DeadPubParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Local crate to scan, e.g. `my_crate`.")]
    pub krate: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct DeadPubReportParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml). Runs dead_pub_in_crate over every local crate and returns aggregated findings per crate.")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct CrateEdgesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct OverlapsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "all (default) | local (lib/bin targets) | local_no_vendor (also excludes vendor/).")]
    pub scope: Option<String>,
}

pub(crate) type ForbiddenDependencyRuleParam = ForbiddenDependencyRule;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ForbiddenDependencyCheckParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Architectural rules to enforce against the workspace's cross-crate edges")]
    pub rules: Vec<ForbiddenDependencyRuleParam>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct EnumVariantsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the enum whose variants you want (e.g. `my_crate::module::MyEnum`)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ItemAttributesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the item whose outer attributes (and doc-comment lines) you want, e.g. `my_crate::Foo`")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ItemsWithAttributeParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scan (e.g. `my_crate`); accepts the crate root module name as an alias")]
    pub crate_name: String,
    #[schemars(description = "Attribute path (`derive`, `must_use`, `cfg`), wrapped form (`#[derive(`) or doc-body prefix (`SAFETY:`).")]
    pub attribute_pattern: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct PubUsePubTypeAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scan (e.g. `my_crate`); accepts the crate root module name as an alias")]
    pub crate_name: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ReExportChainParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the canonical declaration whose re-export chain you want to walk (e.g. `my_crate::module::Token`)")]
    pub target: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct CrateDependencyMetricParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional cap on returned rows after sorting. Default: None (all rows).")]
    #[serde(default)]
    pub top_n: Option<usize>,
    #[schemars(description = "instability | item_count | afferent | efferent | abstractness, descending, before top_n.")]
    #[serde(default)]
    pub sort_by: Option<String>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ModuleTreeParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name (e.g. `my_crate`)")]
    pub krate: String,
    #[schemars(description = "Max depth below the crate root, served whole. Omitted: full tree up to 400 nodes, else cut at the deepest level that fits (reported).")]
    pub depth: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct CrateTypesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scope the search (accepts the crate name or its root module)")]
    pub krate: String,
    #[schemars(description = "Optional type item kinds to include. Defaults to Struct, Enum, Union, Trait, TypeAlias.")]
    #[serde(default)]
    pub item_kind: Option<Vec<String>>,
    #[schemars(description = "Only include pure `pub` type items. Default false.")]
    #[serde(default)]
    pub pub_only: Option<bool>,
    #[schemars(description = "Include associated type items. Default false.")]
    #[serde(default)]
    pub include_associated_types: Option<bool>,
    #[schemars(description = "Drop items inside `::tests::` modules. Default true.")]
    #[serde(default)]
    pub skip_test_items: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct WorkspaceStatsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct FunctionSignatureParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the function (e.g. `crate::module::fn_name` or `crate::Type::method`)")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct FunctionsWithFilterParams {
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
    #[schemars(description = "Page size; default 50.")]
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
pub(crate) struct SimilarToItemParams {
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
    #[schemars(description = "Only this symbol kind (Function, Struct, Enum, Trait, …), case-insensitive.")]
    #[serde(default)]
    pub item_kind: Option<String>,
    #[schemars(description = "Must match the profile given to index_codebase; default local-cpu-small.")]
    #[serde(default)]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct SemanticOverlapsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Optional item-kind filter (\"Function\" | \"Struct\" | \"Enum\" | \"Trait\" | \"Method\"). Default: all kinds.")]
    #[serde(default)]
    pub item_kind: Option<String>,
    #[schemars(description = "Minimum cosine similarity on the active model's scale; default is its tuned cutoff (0.80 local-cpu-small, 0.85 Qwen3). 0.90+ for strict duplicates.")]
    #[serde(default)]
    pub threshold: Option<f32>,
    #[schemars(description = "Cap on returned pairs, or on cluster members in clusters mode; output only, not work. Default 50.")]
    #[serde(default)]
    pub max_pairs: Option<usize>,
    #[schemars(description = "Pairs (or clusters) to skip; default 0.")]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Keep only qualified_name + item_kind per member.")]
    #[serde(default)]
    pub summary: Option<bool>,
    #[schemars(description = "Drop clusters larger than this (chaining noise); default 15, 0 disables.")]
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
    #[schemars(description = "Embedding profile (built-in or from embedding_profiles.toml); sets the scale of `threshold`. Default local-cpu-small.")]
    #[serde(default)]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct BuildCodemapParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Natural-language task; required unless seed_qualified_names is given. Favours well-documented public APIs; misses go to diagnostics.")]
    #[serde(default)]
    pub task_prompt: Option<String>,
    #[schemars(description = "Embedding profile for task_prompt seed lookup; default local-cpu-small.")]
    #[serde(default)]
    pub embedding_profile: Option<String>,
    #[schemars(description = "Seeds by qualified name, any visibility; not fn-local items, trait-impl methods or #[cfg(test)]. Unresolved names go to diagnostics.")]
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct CrateSkeletonParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional local crate names to render. Default: all selected local lib/bin crates.")]
    #[serde(default)]
    pub crates: Option<Vec<String>>,
    #[schemars(description = "Visibility buckets to include: `pub`, `pub(crate)`, `restricted`, `private`, or `all`. Default: `pub`, `pub(crate)`.")]
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[schemars(description = "Preserve item doc comments from the graph snapshot. Default true.")]
    #[serde(default)]
    pub include_docs: Option<bool>,
    #[schemars(description = "Preserve item attributes from the graph snapshot. Default true.")]
    #[serde(default)]
    pub include_attrs: Option<bool>,
    #[schemars(description = "Render synthetic inherent impl facades for retained associated items. Default true.")]
    #[serde(default)]
    pub include_impls: Option<bool>,
    #[schemars(description = "Drop test items by v1 heuristics (`::tests::`, item-level `#[test]`, item-level `#[cfg(test)]`). Default true.")]
    #[serde(default)]
    pub skip_test_items: Option<bool>,
    #[schemars(description = "Exclude vendor crates from local crate selection. Default true.")]
    #[serde(default)]
    pub exclude_vendor: Option<bool>,
    #[schemars(description = "Remove the existing `<directory>/.skeleton` generated tree before writing. Default true.")]
    #[serde(default)]
    pub clean: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}
