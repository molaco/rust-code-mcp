//! Result types for `graph::query` methods.
//!
//! Moved verbatim from the pre-refactor `graph::queries` module in PR 08.
//! Re-exported through `graph::queries` so external consumers
//! (`crate::graph::queries::Foo`, `crate::graph::Foo`) continue to resolve.

use std::collections::BTreeMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::super::ids::NodeId;
use super::super::model::{BindingVisibility, FunctionSignature, ItemKind};

/// One result of `dead_pub_in_crate`: a `pub` item with no cross-crate
/// importers or references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadPubFinding {
    pub target: NodeId,
    pub qualified_name: String,
    pub item_kind: ItemKind,
    pub declared_visibility: BindingVisibility,
}

/// Per-crate aggregate emitted by `dead_pub_report`: every `pub`-but-unused
/// item in a single local crate, sorted by qualified name. Crates with no
/// findings are still included (caller often wants to render zero-result
/// rows for completeness).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateDeadPub {
    pub crate_id: NodeId,
    pub crate_qualified_name: String,
    pub findings: Vec<DeadPubFinding>,
}

/// One target module (or external symbol when no local module can be resolved)
/// referenced by a source module. Import counts come from `use` / `extern crate`
/// bindings; usage counts come from non-import references, including fully
/// qualified inline paths such as `crate::search::bm25::Bm25Search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDependency {
    pub target_module: String,
    pub target_kind: String,
    pub target_crate: Option<String>,
    pub import_count: usize,
    pub usage_count: usize,
    pub symbols: Vec<ModuleDependencySymbol>,
}

/// Per-symbol contribution to a `ModuleDependency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDependencySymbol {
    pub target_qualified: String,
    pub target_kind: String,
    pub import_count: usize,
    pub usage_count: usize,
    pub binding_kinds: Vec<String>,
}

/// One row of `crate_edges`: every cross-crate consumer→producer edge with the
/// concrete symbols carrying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateEdge {
    pub consumer_crate: String,
    pub producer_crate: String,
    pub unique_symbols: usize,
    pub total_refs_via_imports: usize,
    pub total_refs_via_usages: usize,
    pub symbols: Vec<EdgeSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeSymbol {
    pub target_qualified: String,
    pub target_kind: String,
    pub binding_kind: Option<String>,
    pub import_count: usize,
    pub usage_count: usize,
}

/// One architectural rule for `forbidden_dependency_check`. Patterns in
/// `consumer`, `producer`, and `except` are glob-style with `*` wildcards
/// (matched against crate names). `consumer_kinds` filters Cargo target kinds
/// on the consumer side and defaults to `["lib", "bin"]`, which excludes
/// examples, tests, benches, and build scripts from architecture checks unless
/// the caller opts them in. `severity` and `message` are passed through
/// unchanged for caller-side rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ForbiddenDependencyRule {
    #[schemars(description = "Glob pattern matched against the consumer crate name (e.g. `domain*`)")]
    pub consumer: String,
    #[schemars(description = "Glob pattern matched against the producer crate name (e.g. `tokio`)")]
    pub producer: String,
    #[serde(default)]
    #[schemars(description = "Optional consumer Cargo target kinds to inspect. Defaults to [`lib`, `bin`]; use values like `example`, `test`, `bench`, or `build` to opt those targets in")]
    pub consumer_kinds: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(description = "Optional consumer-side glob exception: edges whose consumer matches this pattern are NOT flagged, even if `consumer`/`producer` match")]
    pub except: Option<String>,
    #[serde(default)]
    #[schemars(description = "Optional severity tag passed through to violations (e.g. `error` / `warn`)")]
    pub severity: Option<String>,
    #[serde(default)]
    #[schemars(description = "Optional human-readable rationale, passed through unchanged")]
    pub message: Option<String>,
}

/// One row of `forbidden_dependency_check`: a cross-crate edge that matched
/// a `ForbiddenDependencyRule`. `sample_symbol` is the highest-ref-count
/// symbol carrying the offending edge (rendered as a qualified name) — handy
/// for caller-side "click to navigate" UX.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForbiddenDependencyViolation {
    pub rule_index: usize,
    pub consumer_crate: String,
    pub producer_crate: String,
    pub severity: Option<String>,
    pub message: Option<String>,
    pub sample_symbol: Option<String>,
    pub unique_symbols: usize,
    pub total_refs: usize,
}

/// Result of `overlaps`: name collisions, module shadows, and within-crate
/// duplicates that often signal accidental complexity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlapsReport {
    pub cross_crate_type_collisions: Vec<TypeCollision>,
    pub module_shadows: Vec<ModuleShadow>,
    pub within_crate_type_duplicates: Vec<WithinCrateDuplicate>,
    pub common_fn_names: Vec<CommonFnName>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlapScope {
    All,
    Local,
    LocalNoVendor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeCollision {
    pub name: String,
    pub locations: Vec<TypeLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeLocation {
    pub crate_name: String,
    pub qualified_name: String,
    pub item_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleShadow {
    pub crate_name: String,
    pub module_qualified: String,
    pub shadowed_crate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithinCrateDuplicate {
    pub crate_name: String,
    pub name: String,
    pub qualified_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommonFnName {
    pub name: String,
    pub crates: Vec<String>,
}

/// One row of `who_calls(target_fn)` or `calls_from(caller_fn)`. Both queries
/// return the same shape — the only difference is whether the row's "anchor"
/// is the caller (`who_calls`) or the callee (`calls_from`); each side
/// carries the *other* end's qualified name plus the file:byte-range hit and
/// reference category. References from non-fn scopes (const initializers,
/// trait bounds, enum discriminants) never appear because both queries scan
/// only `Usage` rows where `consumer_function.is_some()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedCallSite {
    pub caller_qualified_name: Option<String>,
    pub callee_qualified_name: String,
    pub file: String,
    pub start: u32,
    pub end: u32,
    pub category: String, // "Read" | "Write" | "Test" | "Other"
}

/// Result of `call_graph(root_fn, depth)` — bounded recursive descent through
/// outgoing fn-body references. `callees` is empty at leaves, depth-limit, or
/// cycle-truncated branches; the two boolean flags disambiguate which.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallGraphNode {
    pub fn_qualified_name: String,
    pub crate_name: Option<String>,
    pub callees: Vec<CallGraphNode>,
    /// `true` if this fn was already expanded earlier in the traversal — its
    /// callees are visible elsewhere in the graph and were skipped here to
    /// avoid infinite recursion / redundant subtrees.
    pub truncated_at_cycle: bool,
    /// `true` if depth ran out at this node and `calls_from(this)` would
    /// otherwise have produced callees.
    pub truncated_at_depth: bool,
}

/// Result of `recursive_callers_count(target, depth)` — reverse BFS over
/// fn-level call sites, counting distinct caller fns reachable backward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecursiveCallersCount {
    pub target_qualified_name: String,
    pub depth: u32,
    pub direct_callers: usize,
    pub transitive_callers: usize,
    pub depth_reached: u32,
    pub truncated_at_depth: bool,
}

/// One row of `who_uses_summary`: aggregation of `usages_of(target)` results,
/// grouped by `(consumer_module, target)`. Each row carries a per-category
/// breakdown so callers can see whether the consumer reads / writes / tests
/// the target. Sorted by `total_count` desc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummaryRow {
    pub consumer_qualified_name: String,
    pub consumer_crate: Option<String>,
    pub total_count: usize,
    pub category_breakdown: BTreeMap<String, usize>,
}

/// Recursive node tree returned by `module_tree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleTreeNode {
    pub qualified_name: String,
    pub display_name: String,
    pub kind: String,
    pub item_kind: Option<String>,
    pub visibility: Option<String>,
    pub children: Vec<ModuleTreeNode>,
}

/// Result of `workspace_stats`: counters across the whole snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceStats {
    pub nodes: NodeKindCounts,
    pub items_by_kind: BTreeMap<String, usize>,
    pub bindings_by_kind: BTreeMap<String, usize>,
    pub visibility: VisibilityCounts,
    /// Human-readable field roles for `visibility`. Kept separate from the
    /// numeric counters so existing clients can ignore it while humans can see
    /// which fields are canonical and which ones are compatibility aliases.
    #[serde(default)]
    pub visibility_notes: BTreeMap<String, String>,
    /// Of the items the author actively made non-private, what fraction is
    /// crate-scoped? Computed as `pub_crate / (pub_ + pub_crate)`. Returns
    /// `0.0` when both counts are zero (degenerate workspace with no
    /// non-private items). Higher values indicate tighter encapsulation:
    /// the author preferred `pub(crate)` over `pub` where possible.
    pub pub_crate_share: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NodeKindCounts {
    pub workspace: usize,
    pub crate_: usize,
    pub module: usize,
    pub item: usize,
    pub external_symbol: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VisibilityCounts {
    pub pub_: usize,
    pub pub_crate: usize,
    /// Declarations visible only inside their declaring module. This is the
    /// precise bucket for implicit private items and explicit `pub(self)`.
    #[serde(default)]
    pub module_private: usize,
    /// Back-compat alias for `module_private` plus unresolved private
    /// restrictions. Prefer `module_private` for the precise resolved bucket.
    pub pub_self: usize,
    /// Declarations restricted to a module subtree broader than the declaring
    /// module, e.g. `pub(super)` or `pub(in path)`.
    pub restricted_to: usize,
    /// Declarations not visible beyond their declaring module, plus any
    /// restricted visibility whose module could not be resolved.
    pub private: usize,
}

/// One row of `items_with_attribute(crate, pattern)`: every Item in the
/// requested crate whose `attributes` list has at least one entry that
/// anchor-matches the supplied pattern. `matched_attribute` is the first
/// matching entry on the Item — useful for caller-side rendering even when
/// the Item carries multiple attributes. `match_location` is `"attr"` when
/// the pattern matched at the start of the attribute string, or `"doc"` when
/// it matched at the start of the body of a `///` doc comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWithAttribute {
    pub target: NodeId,
    pub qualified_name: String,
    pub item_kind: Option<ItemKind>,
    pub matched_attribute: String,
    pub match_location: String,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
}

/// v9 (Phase 5): filter spec for `functions_with_filter(crate, filter)`.
/// Every `Some` field tightens the search; `None` fields are unconstrained.
/// Substring matches are case-sensitive against the HirDisplay strings
/// recorded in the `FunctionSignature`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunctionFilter {
    pub min_param_count: Option<usize>,
    pub has_param_type: Option<String>,
    pub returns_type_pattern: Option<String>,
    pub is_async: Option<bool>,
    pub self_kind: Option<SelfKindFilter>,
}

/// v9: which self-kind a fn must carry to match `FunctionFilter::self_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelfKindFilter {
    /// Matches fns with NO `self` parameter (free fns, assoc fns without self).
    None,
    Owned,
    Ref,
    RefMut,
}

/// v9: one row of `functions_with_filter(crate, filter)` — the matched
/// function's NodeId, qualified name, and full signature record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionWithSignature {
    pub target: NodeId,
    pub qualified_name: String,
    pub signature: FunctionSignature,
}

/// One row of `pub_use_pub_type_audit(crate)`: a `pub type` alias whose
/// owning module also declares a `pub use ... as <alias_name>` (or a
/// `pub use ::<alias_name>` re-export). Indicates the alias may be acting
/// as a bare re-export rather than a true type abbreviation.
///
/// **Heuristic**: this query does NOT verify that the alias's RHS resolves
/// to the same target as the matching `pub use`'s binding (the model
/// doesn't carry a TypeAlias's target). Treat results as candidates for
/// human review — confirm with `find_definition` before acting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PubTypeAliasMasqueradingAsReexport {
    pub alias_qualified_name: String,
    pub alias_node_id: NodeId,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    pub suspicious_pub_use_target_node_id: NodeId,
    pub suspicious_pub_use_visible_name: String,
}

/// One link in the chain returned by `re_export_chain(target)`. Each link
/// names the module hosting a `pub use` binding that re-exports the target
/// (or, transitively, an upstream re-export of the target). `depth` starts
/// at 1 for direct re-exports of `canonical` and increases as the walk
/// follows successive re-exports through downstream modules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReExportLink {
    pub from_module: NodeId,
    pub from_module_qualified_name: String,
    pub visible_name: String,
    pub depth: u8,
}

/// Result of `re_export_chain(target)`: a flattened list of every
/// reachable `pub use` re-export of `target`, walked breadth-first up to
/// `MAX_REEXPORT_HOPS` hops with cycle detection on
/// `(from_module, visible_name)` pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReExportChain {
    pub canonical: NodeId,
    pub canonical_qualified_name: String,
    pub links: Vec<ReExportLink>,
}

/// One row of `crate_dependency_metric()`: per-local-crate Robert Martin
/// instability metric plus an abstractness ratio. Both metrics are
/// NaN-guarded — degenerate counts return 0.0.
///
/// * `efferent` (Ce): distinct producer crates this crate depends on.
/// * `afferent` (Ca): distinct consumer crates that depend on this crate.
/// * `instability = Ce / (Ce + Ca)` — 0.0 = max stable, 1.0 = max unstable.
/// * `abstractness = (traits + pub_type_aliases) / total_items` — share
///   of items in this crate that are abstract surface.
/// * `item_count`: total `NodeKind::Item` nodes whose `crate_id` is this
///   crate. Includes private items, methods, variants, etc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrateMetric {
    pub crate_id: NodeId,
    pub crate_name: String,
    pub efferent: u32,
    pub afferent: u32,
    pub instability: f64,
    pub abstractness: f64,
    pub item_count: u32,
}

/// One row of `mut_static_audit()`: a local `static` item whose recorded
/// `StaticMetadata` matches one of the known global-mutable-state patterns.
/// A single static matching multiple patterns produces one finding per
/// pattern (e.g. `static mut FOO: LazyLock<...>` yields two).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutStaticFinding {
    pub item: NodeId,
    pub qualified_name: String,
    /// One of `MUT_STATIC_PATTERNS[i].0` — the human-readable pattern label.
    pub matched_pattern: String,
    /// The static's `HirDisplay` type string, surfaced for human inspection.
    pub type_string: String,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
}
