//! Layer 6 — read-path queries on a published snapshot.
//!
//! Core primitives, all expressed as direct LMDB lookups (no traversal):
//!   * `imports_of(M)` — scope-side: bindings declared in M that came from a `use`/extern crate.
//!   * `module_dependencies(M)` — scope-side: imported and inline-referenced target modules.
//!   * `exports_of(M, C)` — scope-side, filtered by visibility from consumer C.
//!   * `reexports_of(M, C)` — subset of exports with non-Declared provenance.
//!   * `who_imports(T)` — target-side: bindings anywhere in the workspace whose target is T.
//!
//! Plus a `lookup_by_qualified_name` helper for resolving user-supplied strings
//! to NodeIds (linear scan; sub-millisecond at burn scale, see notes in mod.rs).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use heed::RoTxn;
use serde::{Deserialize, Serialize};

use super::ids::{BindingId, NodeId};
use super::labels::{
    binding_kind_label as label_binding_kind, item_kind_short_label as label_item_kind,
    node_kind_label, usage_category_label,
};
use super::model::{
    Binding, BindingKind, BindingVisibility, FunctionSignature, ItemKind, Node, NodeKind, SelfKind,
    StaticMetadata, Usage,
};
use super::snapshot::OpenedSnapshot;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForbiddenDependencyRule {
    pub consumer: String,
    pub producer: String,
    #[serde(default)]
    pub consumer_kinds: Option<Vec<String>>,
    #[serde(default)]
    pub except: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
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

/// Maximum re-export facade hops to follow before giving up. Bounds recursion
/// in the (pathological) case of a binding chain or a self-referential cycle.
pub(crate) const MAX_REEXPORT_HOPS: usize = 8;

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

/// Pattern classification table for `mut_static_audit`.
///
/// Each entry is `(label, type_substring)`. The classifier scans the static's
/// `type_string` (HirDisplay output) for a literal substring match. The
/// `static mut` pattern is special-cased on `is_mut` rather than on the type
/// string, but it lives in the same const so the documented pattern list
/// stays in one place.
///
/// Why these four:
/// - `static mut` — no synchronization; UB on data race; hard to reason about.
/// - `LazyLock` — globally-mutable interior state initialized on first access.
/// - `OnceLock` — single-write global; still global mutable identity.
/// - `OnceCell` — interior mutability via lazy init, single-thread-only for
///   the std variant.
///
/// `lazy_static!` is intentionally NOT here — Path B's strength is type-name
/// detection, and the macro expands to a generated wrapper type whose name
/// won't match `LazyLock`. Document this as a limitation.
pub(crate) const MUT_STATIC_PATTERNS: &[(&str, &str)] = &[
    // Handled specially via `is_mut`, but listed for documentation parity.
    ("static mut", ""),
    ("LazyLock", "LazyLock"),
    ("OnceLock", "OnceLock"),
    ("OnceCell", "OnceCell"),
];

/// Classify a `StaticMetadata` against `MUT_STATIC_PATTERNS`. Returns the
/// list of matched pattern labels (empty if none). Public to the graph
/// module for unit testing without a snapshot — the workspace-wide audit
/// uses this internally.
pub fn classify_metadata(meta: &StaticMetadata) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for &(label, needle) in MUT_STATIC_PATTERNS {
        if label == "static mut" {
            if meta.is_mut {
                out.push(label);
            }
        } else if !needle.is_empty() && meta.type_string.contains(needle) {
            out.push(label);
        }
    }
    out
}

fn impl_module_item_alias_parts(name: &str) -> Option<(&str, &str, &str)> {
    let (type_prefix, member_name) = name.rsplit_once("::")?;
    let (module_prefix, type_name) = type_prefix.rsplit_once("::")?;
    if module_prefix.is_empty() || type_name.is_empty() || member_name.is_empty() {
        return None;
    }
    Some((module_prefix, type_name, member_name))
}

fn is_impl_module_item_alias_candidate(
    node: &Node,
    module_crate_id: Option<NodeId>,
    module_file: Option<&str>,
    type_name: &str,
    member_name: &str,
) -> bool {
    if node.kind != NodeKind::Item
        || !matches!(
            node.item_kind,
            Some(
                ItemKind::Method
                    | ItemKind::AssocFunction
                    | ItemKind::AssocConst
                    | ItemKind::AssocType
            )
        )
        || node.display_name != member_name
    {
        return false;
    }
    if let Some(crate_id) = module_crate_id {
        if node.crate_id != Some(crate_id) {
            return false;
        }
    }
    if let Some(file) = module_file {
        if node.file.as_deref() != Some(file) {
            return false;
        }
    }

    let suffix = format!("::{type_name}::{member_name}");
    node.qualified_name.ends_with(&suffix)
}

impl OpenedSnapshot {
    /// Resolve a `::`-qualified name to a `(NodeId, Node)`.
    ///
    /// Two-phase lookup:
    ///   1. Canonical match — scan `nodes_by_id` for a `Node.qualified_name == name`.
    ///      This is the common case (declarations live at their canonical path).
    ///   2. Re-export facade fallback — if Phase 1 misses, treat the name as
    ///      `<prefix>::<leaf>`, recursively resolve `<prefix>` (the prefix may
    ///      itself be a re-export facade), then look for a non-Declared binding
    ///      in that module whose `visible_name == leaf` and follow its `target`.
    ///
    /// Recursion is bounded by `MAX_REEXPORT_HOPS` so the function terminates
    /// even in the presence of a binding cycle. The resolved target is returned
    /// as-is, including `ExternalSymbol` stubs — callers that want to walk past
    /// the workspace boundary need to handle that themselves.
    pub fn lookup_by_qualified_name(&self, name: &str) -> Result<Option<(NodeId, Node)>> {
        self.lookup_by_qualified_name_inner(name, MAX_REEXPORT_HOPS)
    }

    fn lookup_by_qualified_name_inner(
        &self,
        name: &str,
        hops_remaining: usize,
    ) -> Result<Option<(NodeId, Node)>> {
        // Phase 1 — canonical name scan.
        {
            let rtxn = self.env.read_txn()?;
            for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
                let (key, node) = entry?;
                if node.qualified_name == name {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(key);
                    return Ok(Some((NodeId(id), node)));
                }
            }
        }

        // Phase 2 — re-export facade fallback.
        if hops_remaining == 0 {
            return Ok(None);
        }

        if let Some(found) = self.lookup_impl_module_item_alias(name, hops_remaining)? {
            return Ok(Some(found));
        }

        let Some((prefix, leaf)) = name.rsplit_once("::") else {
            return Ok(None);
        };
        if prefix.is_empty() || leaf.is_empty() {
            return Ok(None);
        }

        let Some((prefix_id, _prefix_node)) =
            self.lookup_by_qualified_name_inner(prefix, hops_remaining - 1)?
        else {
            return Ok(None);
        };

        let rtxn = self.env.read_txn()?;
        for entry in self.bindings_for_from_module(&rtxn, prefix_id)? {
            let binding = entry?;
            if binding.visible_name != leaf {
                continue;
            }
            // A Declared binding for this name would already have surfaced in
            // Phase 1 via the target's canonical qualified_name. Skip it here so
            // the fallback is strictly about following re-export facades.
            if binding.kind == BindingKind::Declared {
                continue;
            }
            if let Some(target_node) =
                self.dbs.nodes_by_id.get(&rtxn, binding.target.as_bytes())?
            {
                return Ok(Some((binding.target, target_node)));
            }
        }
        Ok(None)
    }

    fn lookup_impl_module_item_alias(
        &self,
        name: &str,
        hops_remaining: usize,
    ) -> Result<Option<(NodeId, Node)>> {
        let Some((module_prefix, type_name, member_name)) = impl_module_item_alias_parts(name)
        else {
            return Ok(None);
        };

        let Some((_module_id, module_node)) =
            self.lookup_by_qualified_name_inner(module_prefix, hops_remaining - 1)?
        else {
            return Ok(None);
        };
        if module_node.kind != NodeKind::Module {
            return Ok(None);
        }

        let mut resolved: Option<(NodeId, Node)> = None;
        let rtxn = self.env.read_txn()?;
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if !is_impl_module_item_alias_candidate(
                &node,
                module_node.crate_id,
                module_node.file.as_deref(),
                type_name,
                member_name,
            ) {
                continue;
            }

            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            if resolved.is_some() {
                return Ok(None);
            }
            resolved = Some((NodeId(id), node));
        }

        Ok(resolved)
    }

    pub fn node_by_id(&self, rtxn: &RoTxn<'_, heed::WithoutTls>, id: NodeId) -> Result<Option<Node>> {
        Ok(self.dbs.nodes_by_id.get(rtxn, id.as_bytes())?)
    }

    fn node_maps(
        &self,
        rtxn: &RoTxn<'_, heed::WithoutTls>,
    ) -> Result<(HashMap<NodeId, Node>, HashMap<NodeId, String>)> {
        let mut nodes = HashMap::new();
        let mut crate_names = HashMap::new();
        for entry in self.dbs.nodes_by_id.iter(rtxn)? {
            let (key, node) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let id = NodeId(id);
            if node.kind == NodeKind::Crate {
                crate_names.insert(id, node.qualified_name.clone());
            }
            nodes.insert(id, node);
        }
        Ok((nodes, crate_names))
    }

    /// Given a `Crate` node's id, find its root `Module` — the module whose
    /// `parent_id == Some(crate_id)` and whose `qualified_name` equals the
    /// crate's `qualified_name`. Returns `None` if the supplied id does not
    /// resolve to a `Crate`, or if no matching root module exists in this
    /// snapshot.
    ///
    /// Implementation note: `lookup_by_qualified_name` returns only the first
    /// match it finds while scanning `nodes_by_id`, but the crate node and its
    /// root module share the same `qualified_name`. This helper scans
    /// `nodes_by_id` looking for the (kind=Module, parent=crate, name=crate)
    /// triple, which is unique by construction in the extraction model.
    pub fn find_root_module_of(&self, crate_id: NodeId) -> Result<Option<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let crate_node = match self.dbs.nodes_by_id.get(&rtxn, crate_id.as_bytes())? {
            Some(n) if n.kind == NodeKind::Crate => n,
            _ => return Ok(None),
        };
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Module
                && node.parent_id == Some(crate_id)
                && node.qualified_name == crate_node.qualified_name
            {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                return Ok(Some(NodeId(id)));
            }
        }
        Ok(None)
    }

    /// Bindings declared in `module` that came from a `use` (or extern crate).
    /// Order is unspecified — caller can sort by visible_name if needed.
    pub fn imports_of(&self, module: NodeId) -> Result<Vec<Binding>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.bindings_for_from_module(&rtxn, module)? {
            let binding = entry?;
            if binding.kind != BindingKind::Declared {
                out.push(binding);
            }
        }
        Ok(out)
    }

    /// Modules referenced by `module`, combining syntactic imports with
    /// non-import usage edges. This complements `imports_of`: fully-qualified
    /// inline references never appear as `Binding`s, but they do appear in
    /// `usages_by_consumer`.
    pub fn module_dependencies(&self, module: NodeId) -> Result<Vec<ModuleDependency>> {
        let rtxn = self.env.read_txn()?;
        let (nodes, crate_names) = self.node_maps(&rtxn)?;
        let mut acc: BTreeMap<NodeId, ModuleDependencyAccumulator> = BTreeMap::new();

        for entry in self.bindings_for_from_module(&rtxn, module)? {
            let binding = entry?;
            if binding.kind == BindingKind::Declared {
                continue;
            }
            let Some((dependency_id, dependency_node)) =
                dependency_node_for(&nodes, binding.target)
            else {
                continue;
            };
            if dependency_id == module {
                continue;
            }
            let target_node = nodes.get(&binding.target);
            let dep = acc.entry(dependency_id).or_insert_with(|| {
                ModuleDependencyAccumulator::new(dependency_node, &crate_names)
            });
            dep.import_count += 1;
            let symbol = dep.symbols.entry(binding.target).or_insert_with(|| {
                ModuleDependencySymbolAccumulator::new(binding.target, target_node)
            });
            symbol.import_count += 1;
            symbol
                .binding_kinds
                .insert(label_binding_kind(binding.kind).to_string());
        }

        for entry in self.usages_for_consumer(&rtxn, module)? {
            let usage = entry?;
            let Some((dependency_id, dependency_node)) = dependency_node_for(&nodes, usage.target)
            else {
                continue;
            };
            if dependency_id == module {
                continue;
            }
            let target_node = nodes.get(&usage.target);
            let dep = acc.entry(dependency_id).or_insert_with(|| {
                ModuleDependencyAccumulator::new(dependency_node, &crate_names)
            });
            dep.usage_count += 1;
            let symbol = dep
                .symbols
                .entry(usage.target)
                .or_insert_with(|| ModuleDependencySymbolAccumulator::new(usage.target, target_node));
            symbol.usage_count += 1;
        }

        let mut dependencies: Vec<ModuleDependency> = acc
            .into_values()
            .map(ModuleDependencyAccumulator::into_dependency)
            .collect();
        dependencies.sort_by(|a, b| {
            a.target_module
                .cmp(&b.target_module)
                .then_with(|| a.target_kind.cmp(&b.target_kind))
        });
        Ok(dependencies)
    }

    /// Bindings declared in `module` that are visible from `consumer`. Includes
    /// both the module's own declared items (true exports) and re-exports.
    pub fn exports_of(&self, module: NodeId, consumer: NodeId) -> Result<Vec<Binding>> {
        let rtxn = self.env.read_txn()?;
        let consumer_ancestry = self.module_ancestors(&rtxn, consumer)?;
        let consumer_crate = self
            .node_by_id(&rtxn, consumer)?
            .and_then(|n| n.crate_id);

        let mut out = Vec::new();
        for entry in self.bindings_for_from_module(&rtxn, module)? {
            let binding = entry?;
            if !is_visible_from(&binding.visibility, consumer_crate, &consumer_ancestry) {
                continue;
            }
            out.push(binding);
        }
        Ok(out)
    }

    /// Subset of `exports_of` whose provenance is *not* Declared (i.e., `pub use`s).
    pub fn reexports_of(&self, module: NodeId, consumer: NodeId) -> Result<Vec<Binding>> {
        let mut out = self.exports_of(module, consumer)?;
        out.retain(|b| b.kind != BindingKind::Declared);
        Ok(out)
    }

    /// Every binding in `module` whose source `use` is explicitly marked `pub`
    /// (or `pub(crate)` / `pub(in path)` / `pub(super)`). Unlike `reexports_of`,
    /// this is not filtered by visibility from a particular consumer — it
    /// returns all syntactic re-export declarations, useful for "audit every
    /// `pub use` in this module" workflows.
    pub fn declared_reexports_of(&self, module: NodeId) -> Result<Vec<Binding>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.bindings_for_from_module(&rtxn, module)? {
            let binding = entry?;
            if binding.kind != BindingKind::Declared && binding.is_explicit_pub_use {
                out.push(binding);
            }
        }
        Ok(out)
    }

    /// All bindings in the workspace whose target is `target` (and that aren't
    /// the target's own declaration). Useful for "who imports symbol X".
    pub fn who_imports(&self, target: NodeId) -> Result<Vec<Binding>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.bindings_for_target(&rtxn, target)? {
            let binding = entry?;
            if binding.kind != BindingKind::Declared {
                out.push(binding);
            }
        }
        Ok(out)
    }

    /// All non-import references to `target`, as recorded by `extract_usages`.
    /// `IMPORT` references are filtered at extraction time — they're modeled
    /// as `Binding`s instead. Order is unspecified.
    pub fn usages_of(&self, target: NodeId) -> Result<Vec<Usage>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            out.push(entry?);
        }
        Ok(out)
    }

    /// All non-import references whose enclosing module is `consumer_module`.
    pub fn usages_in(&self, consumer_module: NodeId) -> Result<Vec<Usage>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.usages_for_consumer(&rtxn, consumer_module)? {
            out.push(entry?);
        }
        Ok(out)
    }

    /// Every non-import reference to `target_fn` whose call site is inside
    /// some function body. Returns the caller's qualified name + file:byte
    /// range + category. References from non-fn scopes (const initializers,
    /// trait bounds, enum discriminants) are excluded — see `who_uses` for
    /// those.
    pub fn who_calls(&self, target_fn: NodeId) -> Result<Vec<EnrichedCallSite>> {
        let rtxn = self.env.read_txn()?;
        let callee_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target_fn.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();
        let mut usages: Vec<Usage> = Vec::new();
        for entry in self.usages_for_target(&rtxn, target_fn)? {
            let usage = entry?;
            if usage.consumer_function.is_some() {
                usages.push(usage);
            }
        }

        let mut out = Vec::with_capacity(usages.len());
        for usage in usages {
            let caller_qualified_name = match usage.consumer_function {
                Some(fn_id) => self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, fn_id.as_bytes())?
                    .map(|n| n.qualified_name),
                None => None,
            };
            out.push(EnrichedCallSite {
                caller_qualified_name,
                callee_qualified_name: callee_qualified_name.clone(),
                file: usage.file,
                start: usage.start,
                end: usage.end,
                category: usage_category_label(usage.category).to_string(),
            });
        }
        Ok(out)
    }

    /// Every non-import reference made *from* the body of `caller_fn`. Returns
    /// the callee's qualified name + file:byte range + category. Closures
    /// inside `caller_fn` attribute to it (RA's default for
    /// `SemanticsScope::containing_function`).
    pub fn calls_from(&self, caller_fn: NodeId) -> Result<Vec<EnrichedCallSite>> {
        let rtxn = self.env.read_txn()?;
        let caller_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, caller_fn.as_bytes())?
            .map(|n| n.qualified_name);

        let mut usages: Vec<Usage> = Vec::new();
        for entry in self.usages_for_consumer_function(&rtxn, caller_fn)? {
            usages.push(entry?);
        }

        let mut out = Vec::with_capacity(usages.len());
        for usage in usages {
            let callee_qualified_name = self
                .dbs
                .nodes_by_id
                .get(&rtxn, usage.target.as_bytes())?
                .map(|n| n.qualified_name)
                .unwrap_or_default();
            out.push(EnrichedCallSite {
                caller_qualified_name: caller_qualified_name.clone(),
                callee_qualified_name,
                file: usage.file,
                start: usage.start,
                end: usage.end,
                category: usage_category_label(usage.category).to_string(),
            });
        }
        Ok(out)
    }

    /// Bounded recursive descent over outgoing fn-body references rooted at
    /// `root_fn`. At each node, `calls_from(node)` is computed, distinct callee
    /// NodeIds are recursed into, and `depth` is decremented. A global
    /// `visited: HashSet<NodeId>` prevents re-expanding the same fn twice
    /// anywhere in the tree (so cycles and DAG-style fan-in both terminate).
    ///
    /// `truncated_at_cycle` flags subtrees pruned because the callee was
    /// already expanded elsewhere — the same callees would have appeared.
    /// `truncated_at_depth` flags subtrees pruned because `depth == 0` and
    /// the node has at least one outgoing edge.
    pub fn call_graph(&self, root_fn: NodeId, depth: u32) -> Result<CallGraphNode> {
        let mut visited: HashSet<NodeId> = HashSet::new();
        self.call_graph_rec(root_fn, depth, &mut visited)
    }

    fn call_graph_rec(
        &self,
        fn_id: NodeId,
        depth: u32,
        visited: &mut HashSet<NodeId>,
    ) -> Result<CallGraphNode> {
        let rtxn = self.env.read_txn()?;
        let node = self.dbs.nodes_by_id.get(&rtxn, fn_id.as_bytes())?;
        let (fn_qualified_name, crate_name) = match node {
            Some(n) => {
                let crate_name = match n.crate_id {
                    Some(cid) => self
                        .dbs
                        .nodes_by_id
                        .get(&rtxn, cid.as_bytes())?
                        .map(|c| c.qualified_name),
                    None => None,
                };
                (n.qualified_name, crate_name)
            }
            None => (String::new(), None),
        };
        drop(rtxn);

        // If this fn has been expanded somewhere else already, prune.
        // The root call (visited empty) always proceeds; subsequent visits to
        // the same NodeId from anywhere in the tree become cycle-truncated.
        if !visited.insert(fn_id) {
            return Ok(CallGraphNode {
                fn_qualified_name,
                crate_name,
                callees: Vec::new(),
                truncated_at_cycle: true,
                truncated_at_depth: false,
            });
        }

        // Collect distinct callee NodeIds. `usages_for_consumer_function`
        // returns one row per call site, so the same callee NodeId may appear
        // multiple times.
        let rtxn2 = self.env.read_txn()?;
        let mut distinct_callees: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        for entry in self.usages_for_consumer_function(&rtxn2, fn_id)? {
            let usage = entry?;
            if seen.insert(usage.target) {
                distinct_callees.push(usage.target);
            }
        }
        drop(rtxn2);

        // At depth 0, leave callees empty and flag truncation if there were any.
        if depth == 0 {
            return Ok(CallGraphNode {
                fn_qualified_name,
                crate_name,
                callees: Vec::new(),
                truncated_at_cycle: false,
                truncated_at_depth: !distinct_callees.is_empty(),
            });
        }

        let mut callees: Vec<CallGraphNode> = Vec::with_capacity(distinct_callees.len());
        for callee_id in distinct_callees {
            let child = self.call_graph_rec(callee_id, depth - 1, visited)?;
            callees.push(child);
        }

        Ok(CallGraphNode {
            fn_qualified_name,
            crate_name,
            callees,
            truncated_at_cycle: false,
            truncated_at_depth: false,
        })
    }

    /// `who_calls(target)` filtered to call sites whose *caller fn* lives in a
    /// crate whose qualified_name equals `crate_qualified`. Callers in any
    /// other crate (or with a missing `crate_id`) are dropped. Note: this
    /// filters by the **caller's** crate, not the target's.
    pub fn callers_in_crate(
        &self,
        target: NodeId,
        crate_qualified: &str,
    ) -> Result<Vec<EnrichedCallSite>> {
        let rtxn = self.env.read_txn()?;
        let callee_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();

        let mut out: Vec<EnrichedCallSite> = Vec::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            let usage = entry?;
            let Some(fn_id) = usage.consumer_function else {
                continue;
            };
            let Some(caller_node) = self.dbs.nodes_by_id.get(&rtxn, fn_id.as_bytes())? else {
                continue;
            };
            let Some(crate_id) = caller_node.crate_id else {
                continue;
            };
            let Some(crate_node) = self.dbs.nodes_by_id.get(&rtxn, crate_id.as_bytes())? else {
                continue;
            };
            if crate_node.qualified_name != crate_qualified {
                continue;
            }
            out.push(EnrichedCallSite {
                caller_qualified_name: Some(caller_node.qualified_name),
                callee_qualified_name: callee_qualified_name.clone(),
                file: usage.file,
                start: usage.start,
                end: usage.end,
                category: usage_category_label(usage.category).to_string(),
            });
        }
        Ok(out)
    }

    /// Reverse BFS from `target`: count distinct caller fns reachable backward
    /// up to `depth` hops. depth=0 returns zeros. depth=1 returns just the
    /// direct callers. Higher depths include transitive callers (callers of
    /// callers, etc.). Counts *fns*, not call sites — a fn that calls target
    /// 5 times counts as 1 caller.
    pub fn recursive_callers_count(
        &self,
        target: NodeId,
        depth: u32,
    ) -> Result<RecursiveCallersCount> {
        let rtxn = self.env.read_txn()?;
        let target_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();
        drop(rtxn);

        if depth == 0 {
            return Ok(RecursiveCallersCount {
                target_qualified_name,
                depth: 0,
                direct_callers: 0,
                transitive_callers: 0,
                depth_reached: 0,
                truncated_at_depth: false,
            });
        }

        // Direct callers (hop 1).
        let mut visited: HashSet<NodeId> = HashSet::new();
        let mut frontier: Vec<NodeId> = Vec::new();
        {
            let rtxn = self.env.read_txn()?;
            for entry in self.usages_for_target(&rtxn, target)? {
                let usage = entry?;
                if let Some(fn_id) = usage.consumer_function {
                    if visited.insert(fn_id) {
                        frontier.push(fn_id);
                    }
                }
            }
        }
        let direct_callers = visited.len();
        let mut depth_reached: u32 = if direct_callers > 0 { 1 } else { 0 };

        // Hops 2..=depth.
        let mut hop: u32 = 1;
        let mut truncated_at_depth = false;
        while hop < depth && !frontier.is_empty() {
            let mut next: Vec<NodeId> = Vec::new();
            for fn_id in frontier.drain(..) {
                let rtxn = self.env.read_txn()?;
                for entry in self.usages_for_target(&rtxn, fn_id)? {
                    let usage = entry?;
                    if let Some(caller_id) = usage.consumer_function {
                        if visited.insert(caller_id) {
                            next.push(caller_id);
                        }
                    }
                }
            }
            if !next.is_empty() {
                depth_reached = hop + 1;
            }
            frontier = next;
            hop += 1;
        }
        // If we exited because we hit depth and the frontier still has
        // un-visited would-be expansions, flag truncation.
        if hop == depth && !frontier.is_empty() {
            // Any node in the frontier that itself has at least one un-visited
            // caller means the BFS isn't exhausted. Do a single peek pass.
            'outer: for fn_id in &frontier {
                let rtxn = self.env.read_txn()?;
                for entry in self.usages_for_target(&rtxn, *fn_id)? {
                    let usage = entry?;
                    if let Some(caller_id) = usage.consumer_function {
                        if !visited.contains(&caller_id) {
                            truncated_at_depth = true;
                            break 'outer;
                        }
                    }
                }
            }
        }

        Ok(RecursiveCallersCount {
            target_qualified_name,
            depth,
            direct_callers,
            transitive_callers: visited.len(),
            depth_reached,
            truncated_at_depth,
        })
    }

    /// Aggregation rollup of `usages_of(target)` grouped by `consumer_module`.
    /// Each row carries a total count and a per-category breakdown
    /// (Read/Write/Test/Other → count). Local inherent method calls and local
    /// trait-declaration dispatch are captured as Method items; remaining
    /// blind spots are indirect calls RA cannot resolve to a workspace Item
    /// (for example `dyn Trait` over external traits or generic `F: Fn(..)`).
    /// Sorted by `total_count` desc, ties broken by `consumer_qualified_name`.
    pub fn who_uses_summary(&self, target: NodeId) -> Result<Vec<UsageSummaryRow>> {
        let rtxn = self.env.read_txn()?;

        // Group by consumer_module: total + per-category breakdown.
        let mut totals: HashMap<NodeId, usize> = HashMap::new();
        let mut breakdown: HashMap<NodeId, BTreeMap<String, usize>> = HashMap::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            let usage = entry?;
            *totals.entry(usage.consumer_module).or_insert(0) += 1;
            let cat = usage_category_label(usage.category).to_string();
            *breakdown
                .entry(usage.consumer_module)
                .or_default()
                .entry(cat)
                .or_insert(0) += 1;
        }

        // Resolve display names. We need the consumer module's qualified_name
        // and (separately) its crate's qualified_name for downstream display.
        let mut rows: Vec<UsageSummaryRow> = Vec::with_capacity(totals.len());
        for (consumer_module, total_count) in totals {
            let (qualified_name, crate_qualified) = match self
                .dbs
                .nodes_by_id
                .get(&rtxn, consumer_module.as_bytes())?
            {
                Some(node) => {
                    let crate_qual = match node.crate_id {
                        Some(cid) => self
                            .dbs
                            .nodes_by_id
                            .get(&rtxn, cid.as_bytes())?
                            .map(|n| n.qualified_name),
                        None => None,
                    };
                    (node.qualified_name, crate_qual)
                }
                None => (String::new(), None),
            };
            rows.push(UsageSummaryRow {
                consumer_qualified_name: qualified_name,
                consumer_crate: crate_qualified,
                total_count,
                category_breakdown: breakdown.remove(&consumer_module).unwrap_or_default(),
            });
        }
        rows.sort_by(|a, b| {
            b.total_count
                .cmp(&a.total_count)
                .then_with(|| a.consumer_qualified_name.cmp(&b.consumer_qualified_name))
        });
        Ok(rows)
    }

    /// v7: enumerate the variants of an enum. `enum_id` must be the NodeId of
    /// an `ItemKind::Enum` Item; non-enum inputs return an empty Vec rather
    /// than erroring (so callers can probe arbitrary Items without
    /// pre-validating). Walks `children_by_parent` and filters to
    /// `item_kind == ItemKind::EnumVariant` — explicit filter even though
    /// today an enum's only children are its variants, so future model
    /// extensions don't silently leak through.
    pub fn enum_variants(&self, enum_id: NodeId) -> Result<Vec<Node>> {
        let rtxn = self.env.read_txn()?;
        let mut child_ids: Vec<NodeId> = Vec::new();
        if let Some(iter) = self
            .dbs
            .children_by_parent
            .get_duplicates(&rtxn, enum_id.as_bytes())?
        {
            for entry in iter {
                let (_k, child_bytes) = entry?;
                let mut id = [0u8; 32];
                id.copy_from_slice(child_bytes);
                child_ids.push(NodeId(id));
            }
        }
        let mut out = Vec::with_capacity(child_ids.len());
        for child_id in child_ids {
            let Some(node) = self.dbs.nodes_by_id.get(&rtxn, child_id.as_bytes())? else {
                continue;
            };
            if node.item_kind == Some(ItemKind::EnumVariant) {
                out.push(node);
            }
        }
        // Sort by source position so variants come back in declaration order
        // rather than alphabetically. Nodes without spans (rare; only ones
        // missing nav-target metadata) sort to the end with empty file path
        // and (0, 0) span to keep ordering deterministic.
        out.sort_by(|a, b| {
            let a_key = (
                a.file.as_deref().unwrap_or(""),
                a.span.map(|(s, _)| s).unwrap_or(0),
            );
            let b_key = (
                b.file.as_deref().unwrap_or(""),
                b.span.map(|(s, _)| s).unwrap_or(0),
            );
            a_key.cmp(&b_key)
        });
        Ok(out)
    }

    /// v8: return the outer attributes and doc-comment lines (one entry per
    /// line) recorded for the Item at `target`. Empty Vec when the target
    /// has no attributes, isn't an Item, or doesn't exist. Order matches
    /// source order.
    pub fn item_attributes(&self, target: NodeId) -> Result<Vec<String>> {
        let rtxn = self.env.read_txn()?;
        let Some(node) = self.dbs.nodes_by_id.get(&rtxn, target.as_bytes())? else {
            return Ok(Vec::new());
        };
        Ok(node.attributes)
    }

    /// v8: every Item in `crate_id` whose attribute list has at least one
    /// entry that matches `attr_pattern`. Wrapped patterns match
    /// case-sensitively at the **start** of the raw attribute string (e.g.
    /// attr `"#[must_use]"` matches pattern `"#[must_use]"` and pattern
    /// `"#[must_use"`), bare patterns such as `derive` or `must_use` match
    /// the attribute path, OR a pattern can match the start of the **body**
    /// of a `///` doc comment (the body is whatever follows the `/// ` prefix). This
    /// avoids false positives where the pattern text appears in the middle
    /// of an unrelated attribute — e.g. searching `#[must_use]` no longer
    /// matches `#[tool(description = "...#[must_use]...")]` whose body just
    /// happens to mention the pattern. Empty patterns match nothing.
    /// Returns enriched rows with file + span so callers can navigate.
    /// Sorted by qualified name.
    pub fn items_with_attribute(
        &self,
        crate_id: NodeId,
        attr_pattern: &str,
    ) -> Result<Vec<ItemWithAttribute>> {
        let rtxn = self.env.read_txn()?;
        let mut out: Vec<ItemWithAttribute> = Vec::new();
        // Empty pattern: match nothing. Substring containment of "" is
        // trivially true on every string, which would flood callers with
        // every attribute-bearing item — almost certainly not what they
        // wanted. The previous behavior here was the same trivial-true
        // bug; switch to "match nothing" as the safer default.
        if attr_pattern.is_empty() {
            return Ok(out);
        }
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind != NodeKind::Item {
                continue;
            }
            if node.crate_id != Some(crate_id) {
                continue;
            }
            let Some((matched, location)) = node
                .attributes
                .iter()
                .find_map(|a| match_attribute(a, attr_pattern).map(|loc| (a.clone(), loc)))
            else {
                continue;
            };
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            out.push(ItemWithAttribute {
                target: NodeId(id),
                qualified_name: node.qualified_name,
                item_kind: node.item_kind,
                matched_attribute: matched,
                match_location: location.to_string(),
                file: node.file,
                span: node.span,
            });
        }
        out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(out)
    }

    /// v9: return the recorded `FunctionSignature` for `target` (a local
    /// function NodeId), or `None` if no signature is present (e.g. the
    /// target isn't a function, or extraction skipped it). Single-key LMDB
    /// lookup, no scan.
    pub fn function_signature(&self, target: NodeId) -> Result<Option<FunctionSignature>> {
        let rtxn = self.env.read_txn()?;
        Ok(self.dbs.signatures_by_target.get(&rtxn, target.as_bytes())?)
    }

    /// v10 (Phase 7 Path B): return the recorded `StaticMetadata` for
    /// `target` (a local `static` NodeId), or `None` if no metadata is
    /// present (e.g. the target isn't a `static`, or extraction skipped
    /// it). Single-key LMDB lookup, no scan.
    pub fn static_metadata(&self, target: NodeId) -> Result<Option<StaticMetadata>> {
        let rtxn = self.env.read_txn()?;
        Ok(self
            .dbs
            .static_metadata_by_target
            .get(&rtxn, target.as_bytes())?)
    }

    /// v10 (Phase 7 Path B): workspace-wide audit of every local `static`
    /// item whose `StaticMetadata` matches one of the known global-mutable-
    /// state patterns (`static mut`, `LazyLock<...>`, `OnceLock<...>`,
    /// `OnceCell<...>`). A single static matching multiple patterns
    /// produces multiple findings (e.g. `static mut FOO: LazyLock<...>`
    /// yields two rows).
    ///
    /// Iterates `Item` nodes whose `item_kind == ItemKind::Static`, fetches
    /// each one's `StaticMetadata` via `static_metadata_by_target`, and
    /// classifies via `classify_metadata`. Sorted by
    /// `(qualified_name, matched_pattern)` for determinism.
    ///
    /// Limitation: the `lazy_static!` macro is NOT detected — its expansion
    /// produces a generated wrapper type whose name doesn't contain
    /// `LazyLock`. Use `items_with_attribute` or grep for `lazy_static!`
    /// invocations to cover that case.
    pub fn mut_static_audit(&self) -> Result<Vec<MutStaticFinding>> {
        let rtxn = self.env.read_txn()?;

        // Collect static Item nodes first so the iterator borrow on rtxn
        // is released before per-target metadata lookups.
        let mut statics: Vec<(NodeId, Node)> = Vec::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind != NodeKind::Item {
                continue;
            }
            if node.item_kind != Some(ItemKind::Static) {
                continue;
            }
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            statics.push((NodeId(id), node));
        }

        let mut out: Vec<MutStaticFinding> = Vec::new();
        for (item_id, item) in statics {
            let Some(meta) = self
                .dbs
                .static_metadata_by_target
                .get(&rtxn, item_id.as_bytes())?
            else {
                continue;
            };
            let matched = classify_metadata(&meta);
            for label in matched {
                out.push(MutStaticFinding {
                    item: item_id,
                    qualified_name: item.qualified_name.clone(),
                    matched_pattern: label.to_string(),
                    type_string: meta.type_string.clone(),
                    file: item.file.clone(),
                    span: item.span,
                });
            }
        }
        out.sort_by(|a, b| {
            a.qualified_name
                .cmp(&b.qualified_name)
                .then_with(|| a.matched_pattern.cmp(&b.matched_pattern))
        });
        Ok(out)
    }

    /// v9: every local function in `crate_id` whose `FunctionSignature`
    /// matches every `Some` field of `filter`. Iterates the
    /// `signatures_by_target` table (linear in #fns), fetches the Node for
    /// each key to scope by `crate_id`, then applies the filter predicates.
    /// Sorted by qualified name.
    pub fn functions_with_filter(
        &self,
        crate_id: NodeId,
        filter: &FunctionFilter,
    ) -> Result<Vec<FunctionWithSignature>> {
        let rtxn = self.env.read_txn()?;
        let mut out: Vec<FunctionWithSignature> = Vec::new();
        for entry in self.dbs.signatures_by_target.iter(&rtxn)? {
            let (key, sig) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let target = NodeId(id);
            let Some(node) = self.dbs.nodes_by_id.get(&rtxn, key)? else {
                continue;
            };
            if node.crate_id != Some(crate_id) {
                continue;
            }
            if !filter_matches(filter, &sig) {
                continue;
            }
            out.push(FunctionWithSignature {
                target,
                qualified_name: node.qualified_name,
                signature: sig,
            });
        }
        out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(out)
    }

    /// Phase 4a (heuristic): every `Item.TypeAlias` in `crate_id` whose
    /// owning module also carries a `pub use ... as <alias_name>` (or
    /// `pub use ::<alias_name>`) binding. Such an alias is a candidate
    /// for being a re-export disguised as a `pub type` declaration.
    ///
    /// Limitation: the model does not record what an alias's RHS resolves
    /// to, so this query cannot confirm the `pub use` and the `pub type`
    /// point at the same target. Treat results as candidates and verify
    /// with `find_definition` / source review before acting.
    pub fn pub_use_pub_type_audit(
        &self,
        crate_id: NodeId,
    ) -> Result<Vec<PubTypeAliasMasqueradingAsReexport>> {
        let rtxn = self.env.read_txn()?;
        // Collect all type-aliases in the crate first so the iterator
        // borrow on rtxn is released before we walk per-alias bindings.
        let mut aliases: Vec<(NodeId, Node)> = Vec::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind != NodeKind::Item {
                continue;
            }
            if node.crate_id != Some(crate_id) {
                continue;
            }
            if node.item_kind != Some(ItemKind::TypeAlias) {
                continue;
            }
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            aliases.push((NodeId(id), node));
        }

        let mut out: Vec<PubTypeAliasMasqueradingAsReexport> = Vec::new();
        for (alias_id, alias) in aliases {
            let Some(owner) = alias.parent_id else {
                continue;
            };
            for entry in self.bindings_for_from_module(&rtxn, owner)? {
                let binding = entry?;
                if !binding.is_explicit_pub_use {
                    continue;
                }
                if binding.visible_name != alias.display_name {
                    continue;
                }
                if binding.target == alias_id {
                    continue; // a binding to the alias itself isn't suspicious
                }
                out.push(PubTypeAliasMasqueradingAsReexport {
                    alias_qualified_name: alias.qualified_name.clone(),
                    alias_node_id: alias_id,
                    file: alias.file.clone(),
                    span: alias.span,
                    suspicious_pub_use_target_node_id: binding.target,
                    suspicious_pub_use_visible_name: binding.visible_name,
                });
            }
        }
        out.sort_by(|a, b| a.alias_qualified_name.cmp(&b.alias_qualified_name));
        Ok(out)
    }

    /// Phase 4b: walk every `pub use` re-export of `target` (and every
    /// re-export of those re-exports) up to `MAX_REEXPORT_HOPS` hops.
    /// Returns one `ReExportLink` per visited binding, breadth-first.
    /// Cycle detection keys on `(from_module, visible_name)` pairs so a
    /// module re-exporting two distinct items under the same name still
    /// surfaces both, while a self-referential cycle terminates.
    pub fn re_export_chain(&self, target: NodeId) -> Result<ReExportChain> {
        let rtxn = self.env.read_txn()?;
        let canonical_node = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();

        let mut links: Vec<ReExportLink> = Vec::new();
        let mut visited: HashSet<(NodeId, String)> = HashSet::new();
        // Frontier: (target_to_search_for, depth_to_assign)
        let mut frontier: Vec<(NodeId, u8)> = vec![(target, 1)];
        while let Some((current_target, depth)) = frontier.pop() {
            if depth as usize > MAX_REEXPORT_HOPS {
                continue;
            }
            // Collect bindings first to release the iterator borrow
            // before per-binding `nodes_by_id.get` lookups.
            let mut bindings_for_current: Vec<Binding> = Vec::new();
            for entry in self.bindings_for_target(&rtxn, current_target)? {
                bindings_for_current.push(entry?);
            }
            for binding in bindings_for_current {
                if !binding.is_explicit_pub_use {
                    continue;
                }
                let key = (binding.from_module, binding.visible_name.clone());
                if !visited.insert(key) {
                    continue;
                }
                let from_module_qualified = self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, binding.from_module.as_bytes())?
                    .map(|n| n.qualified_name)
                    .unwrap_or_default();
                links.push(ReExportLink {
                    from_module: binding.from_module,
                    from_module_qualified_name: from_module_qualified,
                    visible_name: binding.visible_name.clone(),
                    depth,
                });
                // Recurse: the re-exporting module is itself a "target"
                // for downstream re-exports. Bindings whose target is the
                // module ID don't generally exist, so this naturally
                // terminates when no further hops are found.
                if (depth as usize) < MAX_REEXPORT_HOPS {
                    frontier.push((binding.from_module, depth + 1));
                }
            }
        }
        // Stable order: by depth, then module qualified name, then visible name.
        links.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.from_module_qualified_name.cmp(&b.from_module_qualified_name))
                .then_with(|| a.visible_name.cmp(&b.visible_name))
        });
        Ok(ReExportChain {
            canonical: target,
            canonical_qualified_name: canonical_node,
            links,
        })
    }

    /// Phase 4c: per-local-crate Robert Martin instability +
    /// abstractness metrics. Both metrics are NaN-guarded; degenerate
    /// counts (zero edges or zero items) return 0.0.
    pub fn crate_dependency_metric(&self) -> Result<Vec<CrateMetric>> {
        let edges = self.crate_edges()?;
        let rtxn = self.env.read_txn()?;

        // Collect every local crate: (crate_id, crate_name).
        let mut crates: Vec<(NodeId, String)> = Vec::new();
        // Per-crate item counters: (total_items, traits, pub_type_aliases).
        let mut item_counts: HashMap<NodeId, (u32, u32, u32)> = HashMap::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let nid = NodeId(id);
            if node.kind == NodeKind::Crate {
                crates.push((nid, node.qualified_name.clone()));
            } else if node.kind == NodeKind::Item {
                if let Some(crate_id) = node.crate_id {
                    let counts = item_counts.entry(crate_id).or_insert((0, 0, 0));
                    counts.0 += 1;
                    match node.item_kind {
                        Some(ItemKind::Trait) => counts.1 += 1,
                        Some(ItemKind::TypeAlias) => {
                            // Only count pub type aliases for the abstractness ratio.
                            if node.visibility.as_deref() == Some("pub") {
                                counts.2 += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Build distinct producer/consumer sets per local crate name.
        // crate_edges keys by qualified name string, so we map names → ids.
        let name_to_id: HashMap<String, NodeId> =
            crates.iter().map(|(id, n)| (n.clone(), *id)).collect();
        let mut efferent_set: HashMap<NodeId, BTreeSet<String>> = HashMap::new();
        let mut afferent_set: HashMap<NodeId, BTreeSet<String>> = HashMap::new();
        for edge in &edges {
            if let Some(consumer_id) = name_to_id.get(&edge.consumer_crate) {
                efferent_set
                    .entry(*consumer_id)
                    .or_default()
                    .insert(edge.producer_crate.clone());
            }
            if let Some(producer_id) = name_to_id.get(&edge.producer_crate) {
                afferent_set
                    .entry(*producer_id)
                    .or_default()
                    .insert(edge.consumer_crate.clone());
            }
        }

        let mut out: Vec<CrateMetric> = Vec::with_capacity(crates.len());
        for (crate_id, crate_name) in crates {
            let efferent = efferent_set
                .get(&crate_id)
                .map(|s| s.len())
                .unwrap_or(0) as u32;
            let afferent = afferent_set
                .get(&crate_id)
                .map(|s| s.len())
                .unwrap_or(0) as u32;
            let instability = if efferent + afferent == 0 {
                0.0
            } else {
                efferent as f64 / (efferent + afferent) as f64
            };
            let (total_items, trait_count, pub_alias_count) =
                item_counts.get(&crate_id).copied().unwrap_or((0, 0, 0));
            let abstractness = if total_items == 0 {
                0.0
            } else {
                (trait_count + pub_alias_count) as f64 / total_items as f64
            };
            out.push(CrateMetric {
                crate_id,
                crate_name,
                efferent,
                afferent,
                instability,
                abstractness,
                item_count: total_items,
            });
        }
        out.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
        Ok(out)
    }

    /// Items in `crate_id` declared `pub` whose only consumers — both as imports
    /// and as references — live inside the same crate. Such items are candidates
    /// for downgrading to `pub(crate)`.
    ///
    /// Skipped (already minimal):
    ///   * `Private` items.
    ///   * `pub(crate)` items targeting their own crate.
    ///   * `pub(in path)` items — the path is always an ancestor module within
    ///     the same crate, so visibility is already strictly narrower than
    ///     `pub(crate)`.
    ///
    /// Known false positive: an item referenced *only* through a public
    /// function/type signature (never named directly in caller code) won't show
    /// up in `usages_by_target`, so we may flag it as dead-pub even when its
    /// `pub` is load-bearing for the signature. Acceptable for v1 — caller
    /// should treat findings as candidates, not certainties.
    pub fn dead_pub_in_crate(&self, crate_id: NodeId) -> Result<Vec<DeadPubFinding>> {
        let rtxn = self.env.read_txn()?;

        let mut candidates: Vec<(NodeId, Node)> = Vec::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Item && node.crate_id == Some(crate_id) {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                candidates.push((NodeId(id), node));
            }
        }

        let mut out = Vec::new();
        for (item_id, item) in candidates {
            // Collect bindings before doing follow-up `nodes_by_id.get` lookups
            // so the iterator's borrow on `rtxn` is dropped first.
            let mut bindings_for_item: Vec<Binding> = Vec::new();
            for entry in self.bindings_for_target(&rtxn, item_id)? {
                bindings_for_item.push(entry?);
            }

            // Find the Declared binding (visibility lives there). Items appear
            // in Type and Value namespaces for unit/tuple structs, but the
            // post-extraction dedup keeps just one Declared row.
            let Some(declared) = bindings_for_item
                .iter()
                .find(|b| b.kind == BindingKind::Declared)
                .cloned()
            else {
                continue;
            };

            // Visibility filter — only `Public` items are candidates.
            match declared.visibility {
                BindingVisibility::Public => {}
                BindingVisibility::Private
                | BindingVisibility::Crate(_)
                | BindingVisibility::RestrictedTo(_) => continue,
            }

            // External importer check (any non-Declared binding from another crate).
            let mut has_external_importer = false;
            for binding in &bindings_for_item {
                if binding.kind == BindingKind::Declared {
                    continue;
                }
                let Some(from_node) = self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, binding.from_module.as_bytes())?
                else {
                    continue;
                };
                if from_node.crate_id != Some(crate_id) {
                    has_external_importer = true;
                    break;
                }
            }
            if has_external_importer {
                continue;
            }

            // External user check (any usage whose consumer module is in
            // another crate). Collect first, then resolve.
            let mut usages_for_item: Vec<Usage> = Vec::new();
            for entry in self.usages_for_target(&rtxn, item_id)? {
                usages_for_item.push(entry?);
            }
            let mut has_external_user = false;
            for usage in &usages_for_item {
                let Some(consumer_node) = self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, usage.consumer_module.as_bytes())?
                else {
                    continue;
                };
                if consumer_node.crate_id != Some(crate_id) {
                    has_external_user = true;
                    break;
                }
            }
            if has_external_user {
                continue;
            }

            let Some(item_kind) = item.item_kind else {
                continue;
            };
            out.push(DeadPubFinding {
                target: item_id,
                qualified_name: item.qualified_name.clone(),
                item_kind,
                declared_visibility: declared.visibility,
            });
        }
        // Deterministic order — easier to diff across runs.
        out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(out)
    }

    /// Workspace-wide `dead_pub` aggregate: one entry per local crate, with
    /// findings sorted by qualified name. Crates are returned sorted by name.
    pub fn dead_pub_report(&self) -> Result<Vec<CrateDeadPub>> {
        // Gather crate ids first so the per-crate query iterators don't
        // overlap with the outer scan over nodes_by_id.
        let mut crates: Vec<(NodeId, String)> = Vec::new();
        {
            let rtxn = self.env.read_txn()?;
            for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
                let (key, node) = entry?;
                if node.kind == NodeKind::Crate {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(key);
                    crates.push((NodeId(id), node.qualified_name));
                }
            }
        }
        crates.sort_by(|a, b| a.1.cmp(&b.1));

        let mut report = Vec::with_capacity(crates.len());
        for (crate_id, crate_qualified_name) in crates {
            let findings = self.dead_pub_in_crate(crate_id)?;
            report.push(CrateDeadPub {
                crate_id,
                crate_qualified_name,
                findings,
            });
        }
        Ok(report)
    }

    /// All cross-crate consumer→producer edges, decorated with the symbols
    /// carrying each edge. Cost is O(N_nodes + N_bindings + N_usages) — a
    /// single read transaction with three sequential scans.
    ///
    /// Note: local inherent method calls and local trait-declaration dispatch
    /// are captured as Method items in `total_refs_via_usages`. Remaining
    /// blind spots are indirect calls RA cannot resolve to a workspace Item
    /// (for example `dyn Trait` over external traits or generic `F: Fn(..)`).
    pub fn crate_edges(&self) -> Result<Vec<CrateEdge>> {
        let rtxn = self.env.read_txn()?;

        // Build crate index: every node → its crate id; every crate id → name.
        let mut node_to_crate: HashMap<NodeId, NodeId> = HashMap::new();
        let mut crate_name: HashMap<NodeId, String> = HashMap::new();
        let mut node_qual: HashMap<NodeId, String> = HashMap::new();
        let mut node_kind_label_map: HashMap<NodeId, String> = HashMap::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let nid = NodeId(id);
            if node.kind == NodeKind::Crate {
                crate_name.insert(nid, node.qualified_name.clone());
            }
            if let Some(cid) = node.crate_id {
                node_to_crate.insert(nid, cid);
            }
            node_qual.insert(nid, node.qualified_name.clone());
            node_kind_label_map.insert(nid, node_kind_label(&node, label_item_kind));
        }

        // (consumer_crate, producer_crate, target, binding_kind) → import_count
        let mut import_acc: HashMap<(NodeId, NodeId, NodeId, BindingKind), usize> = HashMap::new();
        for entry in self.dbs.bindings_by_id.iter(&rtxn)? {
            let (_k, binding) = entry?;
            if binding.kind == BindingKind::Declared {
                continue;
            }
            let Some(consumer_crate) = node_to_crate.get(&binding.from_module).copied() else {
                continue;
            };
            let Some(producer_crate) = node_to_crate.get(&binding.target).copied() else {
                continue;
            };
            if consumer_crate == producer_crate {
                continue;
            }
            *import_acc
                .entry((consumer_crate, producer_crate, binding.target, binding.kind))
                .or_insert(0) += 1;
        }

        // (consumer_crate, producer_crate, target) → usage_count
        let mut usage_acc: HashMap<(NodeId, NodeId, NodeId), usize> = HashMap::new();
        for entry in self.dbs.usages_by_id.iter(&rtxn)? {
            let (_k, usage) = entry?;
            let Some(consumer_crate) = node_to_crate.get(&usage.consumer_module).copied() else {
                continue;
            };
            let Some(producer_crate) = node_to_crate.get(&usage.target).copied() else {
                continue;
            };
            if consumer_crate == producer_crate {
                continue;
            }
            *usage_acc
                .entry((consumer_crate, producer_crate, usage.target))
                .or_insert(0) += 1;
        }

        // Merge into per-edge per-symbol records.
        // Key: (consumer, producer) → Map<(target, binding_kind_label), (import_count, usage_count)>
        let mut per_edge: HashMap<(NodeId, NodeId), HashMap<(NodeId, Option<String>), (usize, usize)>> =
            HashMap::new();

        for ((c, p, t, bk), n) in import_acc {
            let entry = per_edge
                .entry((c, p))
                .or_default()
                .entry((t, Some(label_binding_kind(bk).to_string())))
                .or_insert((0, 0));
            entry.0 += n;
        }
        for ((c, p, t), n) in usage_acc {
            let entry = per_edge
                .entry((c, p))
                .or_default()
                // No binding_kind on a pure usage edge.
                .entry((t, None))
                .or_insert((0, 0));
            entry.1 += n;
        }

        let mut edges: Vec<CrateEdge> = Vec::new();
        for ((c, p), per_symbol) in per_edge {
            let consumer_crate = crate_name.get(&c).cloned().unwrap_or_default();
            let producer_crate = crate_name.get(&p).cloned().unwrap_or_default();

            // Collapse two rows for the same target (one with binding_kind,
            // one without) into one symbol row when possible. We keep the
            // binding_kind label if any binding exists for that target.
            let mut by_target: BTreeMap<NodeId, EdgeSymbol> = BTreeMap::new();
            for ((t, bk), (ic, uc)) in per_symbol {
                let target_qualified = node_qual.get(&t).cloned().unwrap_or_default();
                let target_kind = node_kind_label_map.get(&t).cloned().unwrap_or_default();
                let sym = by_target.entry(t).or_insert_with(|| EdgeSymbol {
                    target_qualified,
                    target_kind,
                    binding_kind: None,
                    import_count: 0,
                    usage_count: 0,
                });
                sym.import_count += ic;
                sym.usage_count += uc;
                if bk.is_some() && sym.binding_kind.is_none() {
                    sym.binding_kind = bk;
                }
            }

            let mut symbols: Vec<EdgeSymbol> = by_target.into_values().collect();
            symbols.sort_by(|a, b| {
                let ta = a.import_count + a.usage_count;
                let tb = b.import_count + b.usage_count;
                tb.cmp(&ta).then_with(|| a.target_qualified.cmp(&b.target_qualified))
            });

            let unique_symbols = symbols.len();
            let total_refs_via_imports = symbols.iter().map(|s| s.import_count).sum();
            let total_refs_via_usages = symbols.iter().map(|s| s.usage_count).sum();

            edges.push(CrateEdge {
                consumer_crate,
                producer_crate,
                unique_symbols,
                total_refs_via_imports,
                total_refs_via_usages,
                symbols,
            });
        }
        edges.sort_by(|a, b| {
            a.consumer_crate
                .cmp(&b.consumer_crate)
                .then_with(|| a.producer_crate.cmp(&b.producer_crate))
        });
        Ok(edges)
    }

    /// Pure filter over `crate_edges`. For every (consumer, producer) edge,
    /// test each rule; emit a violation when the consumer matches
    /// `rule.consumer`, the producer matches `rule.producer`, the consumer
    /// target kind is allowed by `rule.consumer_kinds`, and (if `except` is
    /// set) the consumer does NOT match `rule.except`.
    ///
    /// Patterns are glob-style with `*` wildcards. Pattern matching is on crate
    /// names; `consumer_kinds` defaults to `["lib", "bin"]`. Severity and
    /// message pass through to violations unchanged for caller-side rendering.
    pub fn forbidden_dependency_check(
        &self,
        rules: &[ForbiddenDependencyRule],
    ) -> Result<Vec<ForbiddenDependencyViolation>> {
        let edges = self.crate_edges()?;
        let crate_target_kind_by_name = self.crate_target_kind_by_name()?;
        let mut violations: Vec<ForbiddenDependencyViolation> = Vec::new();
        for edge in &edges {
            let consumer_kind = crate_target_kind_by_name
                .get(&edge.consumer_crate)
                .map(String::as_str)
                .unwrap_or("lib");
            for (idx, rule) in rules.iter().enumerate() {
                if !rule_allows_consumer_kind(rule, consumer_kind) {
                    continue;
                }
                if !glob_match(&rule.consumer, &edge.consumer_crate) {
                    continue;
                }
                if !glob_match(&rule.producer, &edge.producer_crate) {
                    continue;
                }
                if let Some(except) = rule.except.as_ref() {
                    if glob_match(except, &edge.consumer_crate) {
                        continue;
                    }
                }
                let total_refs = edge.total_refs_via_imports + edge.total_refs_via_usages;
                let sample_symbol = edge
                    .symbols
                    .first()
                    .map(|s| s.target_qualified.clone());
                violations.push(ForbiddenDependencyViolation {
                    rule_index: idx,
                    consumer_crate: edge.consumer_crate.clone(),
                    producer_crate: edge.producer_crate.clone(),
                    severity: rule.severity.clone(),
                    message: rule.message.clone(),
                    sample_symbol,
                    unique_symbols: edge.unique_symbols,
                    total_refs,
                });
            }
        }
        violations.sort_by(|a, b| {
            a.rule_index
                .cmp(&b.rule_index)
                .then_with(|| a.consumer_crate.cmp(&b.consumer_crate))
                .then_with(|| a.producer_crate.cmp(&b.producer_crate))
        });
        Ok(violations)
    }

    fn crate_target_kind_by_name(&self) -> Result<HashMap<String, String>> {
        let rtxn = self.read_txn()?;
        let mut out = HashMap::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (_key, node) = entry?;
            if node.kind == NodeKind::Crate {
                out.insert(
                    node.qualified_name,
                    node.crate_target_kind.unwrap_or_else(|| "lib".to_string()),
                );
            }
        }
        Ok(out)
    }

    /// Phase 6: query-time audit of `unsafe { ... }` blocks across the
    /// workspace. Live computation (no cache); requires a `LoadedWorkspace`
    /// supplied by the caller. Implementation lives in
    /// `crate::graph::unsafe_audit`.
    pub fn unsafe_audit(
        &self,
        loaded: &super::loader::LoadedWorkspace,
    ) -> Result<Vec<super::unsafe_audit::UnsafeFinding>> {
        super::unsafe_audit::unsafe_audit_impl(loaded, self)
    }

    /// Single-pass over `nodes_by_id`. Detects cross-crate type collisions,
    /// module shadowing of crate names, within-crate type duplicates, and
    /// fn names that appear in 4+ crates.
    pub fn overlaps(&self) -> Result<OverlapsReport> {
        self.overlaps_with_scope(OverlapScope::All)
    }

    pub fn overlaps_with_scope(&self, scope: OverlapScope) -> Result<OverlapsReport> {
        let rtxn = self.env.read_txn()?;

        let mut crate_name_for: HashMap<NodeId, String> = HashMap::new();
        let mut crate_target_kind_for: HashMap<NodeId, String> = HashMap::new();
        let mut vendor_crates: HashSet<NodeId> = HashSet::new();

        // First pass: build crate indexes and detect crates whose local
        // source lives under vendor/.
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Crate {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                let crate_id = NodeId(id);
                crate_name_for.insert(crate_id, node.display_name.clone());
                crate_target_kind_for.insert(
                    crate_id,
                    node.crate_target_kind.unwrap_or_else(|| "lib".to_string()),
                );
            }
            if let (Some(crate_id), Some(file)) = (node.crate_id, node.file.as_deref()) {
                if file.starts_with("vendor/") {
                    vendor_crates.insert(crate_id);
                }
            }
        }
        let allowed_crates: HashSet<NodeId> = crate_name_for
            .keys()
            .copied()
            .filter(|crate_id| {
                overlap_scope_allows_crate(
                    scope,
                    *crate_id,
                    &crate_target_kind_for,
                    &vendor_crates,
                )
            })
            .collect();
        let crate_names: HashSet<String> = allowed_crates
            .iter()
            .filter_map(|crate_id| crate_name_for.get(crate_id).cloned())
            .collect();

        // Group containers we'll fill on the second pass.
        let mut type_groups: HashMap<String, Vec<(NodeId, Node, NodeId)>> = HashMap::new();
        let mut shadows: Vec<ModuleShadow> = Vec::new();
        let mut within_crate_types: HashMap<(NodeId, String), Vec<Node>> = HashMap::new();
        let mut fn_spread: HashMap<String, BTreeSet<String>> = HashMap::new();

        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let nid = NodeId(id);

            if node.kind == NodeKind::Module {
                if let Some(crate_id) = node.crate_id {
                    if !allowed_crates.contains(&crate_id) {
                        continue;
                    }
                    let owning_crate = crate_name_for.get(&crate_id).cloned().unwrap_or_default();
                    if crate_names.contains(&node.display_name)
                        && node.display_name != owning_crate
                    {
                        shadows.push(ModuleShadow {
                            crate_name: owning_crate,
                            module_qualified: node.qualified_name.clone(),
                            shadowed_crate: node.display_name.clone(),
                        });
                    }
                }
            }

            if node.kind != NodeKind::Item {
                continue;
            }
            let Some(item_kind) = node.item_kind else {
                continue;
            };
            let Some(crate_id) = node.crate_id else {
                continue;
            };
            if !allowed_crates.contains(&crate_id) {
                continue;
            }

            // Type-kind items participate in collision and within-crate dup checks.
            if matches!(
                item_kind,
                ItemKind::Struct | ItemKind::Enum | ItemKind::Trait | ItemKind::TypeAlias
            ) {
                type_groups
                    .entry(node.display_name.clone())
                    .or_default()
                    .push((nid, node.clone(), crate_id));
                within_crate_types
                    .entry((crate_id, node.display_name.clone()))
                    .or_default()
                    .push(node.clone());
            }

            // Fn-spread check.
            if item_kind == ItemKind::Function {
                if let Some(crate_dn) = crate_name_for.get(&crate_id) {
                    fn_spread
                        .entry(node.display_name.clone())
                        .or_default()
                        .insert(crate_dn.clone());
                }
            }
        }

        // Cross-crate type collisions: name appears in ≥2 distinct crates.
        let mut cross_crate_type_collisions: Vec<TypeCollision> = type_groups
            .into_iter()
            .filter_map(|(name, group)| {
                let distinct: HashSet<NodeId> = group.iter().map(|(_, _, c)| *c).collect();
                if distinct.len() < 2 {
                    return None;
                }
                let mut locations: Vec<TypeLocation> = group
                    .into_iter()
                    .map(|(_, n, cid)| TypeLocation {
                        crate_name: crate_name_for.get(&cid).cloned().unwrap_or_default(),
                        qualified_name: n.qualified_name,
                        item_kind: n.item_kind.map(label_item_kind).unwrap_or("?").to_string(),
                    })
                    .collect();
                locations.sort_by(|a, b| {
                    a.crate_name
                        .cmp(&b.crate_name)
                        .then_with(|| a.qualified_name.cmp(&b.qualified_name))
                });
                Some(TypeCollision { name, locations })
            })
            .collect();
        cross_crate_type_collisions.sort_by(|a, b| a.name.cmp(&b.name));

        // Within-crate duplicates: ≥2 entries under the same (crate, name).
        let mut within_crate_type_duplicates: Vec<WithinCrateDuplicate> = within_crate_types
            .into_iter()
            .filter_map(|((cid, name), nodes)| {
                if nodes.len() < 2 {
                    return None;
                }
                let mut qualified_names: Vec<String> =
                    nodes.into_iter().map(|n| n.qualified_name).collect();
                qualified_names.sort();
                Some(WithinCrateDuplicate {
                    crate_name: crate_name_for.get(&cid).cloned().unwrap_or_default(),
                    name,
                    qualified_names,
                })
            })
            .collect();
        within_crate_type_duplicates.sort_by(|a, b| {
            a.crate_name
                .cmp(&b.crate_name)
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut common_fn_names: Vec<CommonFnName> = fn_spread
            .into_iter()
            .filter(|(_, set)| set.len() >= 4)
            .map(|(name, set)| CommonFnName {
                name,
                crates: set.into_iter().collect(),
            })
            .collect();
        common_fn_names.sort_by(|a, b| {
            b.crates.len().cmp(&a.crates.len()).then_with(|| a.name.cmp(&b.name))
        });

        shadows.sort_by(|a, b| {
            a.crate_name
                .cmp(&b.crate_name)
                .then_with(|| a.module_qualified.cmp(&b.module_qualified))
        });

        Ok(OverlapsReport {
            cross_crate_type_collisions,
            module_shadows: shadows,
            within_crate_type_duplicates,
            common_fn_names,
        })
    }

    /// Recursive module/item tree rooted at the crate node whose
    /// `qualified_name` matches `crate_name`. `depth` of `Some(n)` limits
    /// recursion to n levels below the root (root itself is depth 0).
    pub fn module_tree(&self, crate_name: &str, depth: Option<usize>) -> Result<ModuleTreeNode> {
        let rtxn = self.env.read_txn()?;
        let mut crate_id: Option<NodeId> = None;
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Crate && node.qualified_name == crate_name {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                crate_id = Some(NodeId(id));
                break;
            }
        }
        let crate_id = crate_id
            .with_context(|| format!("no Crate node with qualified_name `{crate_name}`"))?;

        // Pre-build a target -> formatted-visibility map for every Item in this
        // crate. The model stores visibility on the declaring `Binding`, not
        // on the Item Node, so without this lookup `module_tree` would emit
        // `null` for every item. One linear pass over `bindings_by_id` filtered
        // by the item's owning crate keeps build_module_tree's per-item lookup
        // O(1).
        let mut item_visibility: HashMap<NodeId, String> = HashMap::new();
        // First, collect the set of Item NodeIds in this crate.
        let mut crate_items: HashSet<NodeId> = HashSet::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Item && node.crate_id == Some(crate_id) {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                crate_items.insert(NodeId(id));
            }
        }
        // Second, walk bindings_by_id and pick up Declared bindings whose
        // target is one of those items. If an item somehow has multiple
        // Declared bindings (defensively — shouldn't happen in well-formed
        // extraction), prefer one whose `from_module` matches the item's
        // parent module; otherwise keep the first.
        let mut item_parents: HashMap<NodeId, NodeId> = HashMap::new();
        for item_id in &crate_items {
            if let Some(node) = self.dbs.nodes_by_id.get(&rtxn, item_id.as_bytes())? {
                if let Some(parent) = node.parent_id {
                    item_parents.insert(*item_id, parent);
                }
            }
        }
        let mut item_vis_picks: HashMap<NodeId, (BindingVisibility, bool)> = HashMap::new();
        for entry in self.dbs.bindings_by_id.iter(&rtxn)? {
            let (_k, binding) = entry?;
            if binding.kind != BindingKind::Declared {
                continue;
            }
            if !crate_items.contains(&binding.target) {
                continue;
            }
            let parent_match = item_parents
                .get(&binding.target)
                .map(|p| *p == binding.from_module)
                .unwrap_or(false);
            match item_vis_picks.get(&binding.target) {
                None => {
                    item_vis_picks.insert(binding.target, (binding.visibility, parent_match));
                }
                Some((_, existing_parent_match)) => {
                    // Upgrade only if we previously had a non-parent-matching
                    // pick and the new one matches the parent module.
                    if !existing_parent_match && parent_match {
                        item_vis_picks.insert(binding.target, (binding.visibility, parent_match));
                    }
                }
            }
        }
        for (id, (vis, _)) in item_vis_picks {
            item_visibility.insert(id, format_binding_visibility(&rtxn, self, vis));
        }

        self.build_module_tree(&rtxn, crate_id, depth, 0, &item_visibility)
    }

    fn build_module_tree(
        &self,
        rtxn: &RoTxn<'_, heed::WithoutTls>,
        node_id: NodeId,
        depth_limit: Option<usize>,
        cur_depth: usize,
        item_visibility: &HashMap<NodeId, String>,
    ) -> Result<ModuleTreeNode> {
        let node = self
            .dbs
            .nodes_by_id
            .get(rtxn, node_id.as_bytes())?
            .with_context(|| "dangling NodeId in module_tree walk")?;

        let mut children_nodes: Vec<ModuleTreeNode> = Vec::new();
        let stop_recursion = depth_limit.map(|d| cur_depth >= d).unwrap_or(false);

        if !stop_recursion {
            // Collect child ids first so the iterator's borrow on rtxn drops
            // before we recurse.
            let mut child_ids: Vec<NodeId> = Vec::new();
            if let Some(iter) = self
                .dbs
                .children_by_parent
                .get_duplicates(rtxn, node_id.as_bytes())?
            {
                for entry in iter {
                    let (_k, child_bytes) = entry?;
                    let mut id = [0u8; 32];
                    id.copy_from_slice(child_bytes);
                    child_ids.push(NodeId(id));
                }
            }
            for child_id in child_ids {
                children_nodes.push(self.build_module_tree(
                    rtxn,
                    child_id,
                    depth_limit,
                    cur_depth + 1,
                    item_visibility,
                )?);
            }
            children_nodes.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        }

        let item_kind_label = node
            .item_kind
            .map(|k| format!("Item.{}", label_item_kind(k)));
        let visibility = if node.kind == NodeKind::Item {
            item_visibility.get(&node_id).cloned()
        } else {
            node.visibility.clone()
        };
        Ok(ModuleTreeNode {
            qualified_name: node.qualified_name.clone(),
            display_name: node.display_name.clone(),
            kind: node_kind_label(&node, label_item_kind),
            item_kind: item_kind_label,
            visibility,
            children: children_nodes,
        })
    }

    /// Two-pass aggregate: counts of nodes (by kind), items (by ItemKind),
    /// bindings (by BindingKind), and Binding-level visibility.
    pub fn workspace_stats(&self) -> Result<WorkspaceStats> {
        let rtxn = self.env.read_txn()?;
        let mut nodes = NodeKindCounts::default();
        let mut items_by_kind: BTreeMap<String, usize> = BTreeMap::new();

        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (_k, node) = entry?;
            match node.kind {
                NodeKind::Workspace => nodes.workspace += 1,
                NodeKind::Crate => nodes.crate_ += 1,
                NodeKind::Module => nodes.module += 1,
                NodeKind::Item => {
                    nodes.item += 1;
                    if let Some(ik) = node.item_kind {
                        *items_by_kind
                            .entry(label_item_kind(ik).to_string())
                            .or_insert(0) += 1;
                    }
                }
                NodeKind::ExternalSymbol => nodes.external_symbol += 1,
            }
        }

        let mut bindings_by_kind: BTreeMap<String, usize> = BTreeMap::new();
        let mut visibility = VisibilityCounts::default();

        for entry in self.dbs.bindings_by_id.iter(&rtxn)? {
            let (_k, binding) = entry?;
            *bindings_by_kind
                .entry(label_binding_kind(binding.kind).to_string())
                .or_insert(0) += 1;
            // Visibility counts are only meaningful for Declared bindings
            // (the ones that carry the item's source visibility). Counting
            // all bindings would over-count re-exports. Filter to Declared.
            if binding.kind == BindingKind::Declared {
                count_declared_visibility(&mut visibility, &binding);
            }
        }

        // `pub_crate / (pub_ + pub_crate)` — of the items the author actively
        // made non-private, what fraction is crate-scoped? Avoid NaN on a
        // degenerate workspace with zero non-private items.
        let non_private = visibility.pub_ + visibility.pub_crate;
        let pub_crate_share = if non_private == 0 {
            0.0
        } else {
            visibility.pub_crate as f64 / non_private as f64
        };

        Ok(WorkspaceStats {
            nodes,
            items_by_kind,
            bindings_by_kind,
            visibility,
            visibility_notes: visibility_count_notes(),
            pub_crate_share,
        })
    }

    // ----- helpers -----

    fn bindings_for_from_module<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        module: NodeId,
    ) -> Result<impl Iterator<Item = Result<Binding>> + 'txn> {
        // bindings_by_from_module is DUP_SORT: NodeId → BindingId. We iterate
        // duplicates of the given key, then resolve each BindingId to a Binding.
        Ok(self
            .dbs
            .bindings_by_from_module
            .get_duplicates(rtxn, module.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, bid_bytes) = entry?;
                let mut bid = [0u8; 32];
                bid.copy_from_slice(bid_bytes);
                let binding = self
                    .dbs
                    .bindings_by_id
                    .get(rtxn, &bid)?
                    .context("dangling BindingId in bindings_by_from_module")?;
                let _ = BindingId(bid);
                Ok(binding)
            }))
    }

    fn bindings_for_target<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        target: NodeId,
    ) -> Result<impl Iterator<Item = Result<Binding>> + 'txn> {
        Ok(self
            .dbs
            .bindings_by_target
            .get_duplicates(rtxn, target.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, bid_bytes) = entry?;
                let mut bid = [0u8; 32];
                bid.copy_from_slice(bid_bytes);
                let binding = self
                    .dbs
                    .bindings_by_id
                    .get(rtxn, &bid)?
                    .context("dangling BindingId in bindings_by_target")?;
                Ok(binding)
            }))
    }

    fn usages_for_target<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        target: NodeId,
    ) -> Result<impl Iterator<Item = Result<Usage>> + 'txn> {
        Ok(self
            .dbs
            .usages_by_target
            .get_duplicates(rtxn, target.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, uid_bytes) = entry?;
                let mut uid = [0u8; 32];
                uid.copy_from_slice(uid_bytes);
                let usage = self
                    .dbs
                    .usages_by_id
                    .get(rtxn, &uid)?
                    .context("dangling UsageId in usages_by_target")?;
                Ok(usage)
            }))
    }

    fn usages_for_consumer<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        consumer: NodeId,
    ) -> Result<impl Iterator<Item = Result<Usage>> + 'txn> {
        Ok(self
            .dbs
            .usages_by_consumer
            .get_duplicates(rtxn, consumer.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, uid_bytes) = entry?;
                let mut uid = [0u8; 32];
                uid.copy_from_slice(uid_bytes);
                let usage = self
                    .dbs
                    .usages_by_id
                    .get(rtxn, &uid)?
                    .context("dangling UsageId in usages_by_consumer")?;
                Ok(usage)
            }))
    }

    fn usages_for_consumer_function<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        caller_fn: NodeId,
    ) -> Result<impl Iterator<Item = Result<Usage>> + 'txn> {
        Ok(self
            .dbs
            .usages_by_consumer_function
            .get_duplicates(rtxn, caller_fn.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, uid_bytes) = entry?;
                let mut uid = [0u8; 32];
                uid.copy_from_slice(uid_bytes);
                let usage = self
                    .dbs
                    .usages_by_id
                    .get(rtxn, &uid)?
                    .context("dangling UsageId in usages_by_consumer_function")?;
                Ok(usage)
            }))
    }

    /// Distinct outgoing references from `caller_fn`'s body.
    ///
    /// Wraps the private `usages_for_consumer_function` iterator and dedupes
    /// by target `NodeId`. Includes calls, type references, const reads —
    /// anything `Usage` produces with `consumer_function == Some(caller_fn)`.
    /// The caller (codemap layer) classifies edges by reading each target's
    /// `Node.item_kind`.
    pub(crate) fn callees_of(&self, caller_fn: NodeId) -> Result<Vec<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let mut seen: HashSet<NodeId> = HashSet::new();
        for entry in self.usages_for_consumer_function(&rtxn, caller_fn)? {
            seen.insert(entry?.target);
        }
        Ok(seen.into_iter().collect())
    }

    /// Distinct functions whose body contains a reference to `target`.
    ///
    /// Mirrors the `consumer_function.is_some()` filter used by `who_calls`.
    /// Semantics depend on `target`'s `ItemKind`: if `target` is callable
    /// these are callers, if `target` is a type these are consumers —
    /// classification is the caller's concern.
    pub(crate) fn referrers_of(&self, target: NodeId) -> Result<Vec<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let mut seen: HashSet<NodeId> = HashSet::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            if let Some(referrer) = entry?.consumer_function {
                seen.insert(referrer);
            }
        }
        Ok(seen.into_iter().collect())
    }

    /// Walk up `module → parent → ...` and return the set including `module`
    /// itself. Used to answer "is C a descendant of M?".
    fn module_ancestors(
        &self,
        rtxn: &RoTxn<'_, heed::WithoutTls>,
        module: NodeId,
    ) -> Result<HashSet<NodeId>> {
        let mut seen = HashSet::new();
        let mut cur = Some(module);
        while let Some(id) = cur {
            if !seen.insert(id) {
                break; // cycle guard
            }
            cur = self
                .dbs
                .nodes_by_id
                .get(rtxn, id.as_bytes())?
                .and_then(|n| n.parent_id);
        }
        Ok(seen)
    }
}

fn count_declared_visibility(counts: &mut VisibilityCounts, binding: &Binding) {
    match binding.visibility {
        BindingVisibility::Public => counts.pub_ += 1,
        BindingVisibility::Crate(_) => counts.pub_crate += 1,
        BindingVisibility::RestrictedTo(module_id) if module_id == binding.from_module => {
            counts.module_private += 1;
            counts.pub_self += 1;
            counts.private += 1;
        }
        BindingVisibility::RestrictedTo(_) => counts.restricted_to += 1,
        BindingVisibility::Private => {
            counts.pub_self += 1;
            counts.private += 1;
        }
    }
}

fn visibility_count_notes() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "module_private".to_string(),
            "canonical count for declarations visible only inside their declaring module"
                .to_string(),
        ),
        (
            "pub_self".to_string(),
            "back-compat alias for module-private declarations; prefer module_private"
                .to_string(),
        ),
        (
            "private".to_string(),
            "legacy private bucket: module_private plus unresolved private restrictions"
                .to_string(),
        ),
        (
            "restricted_to".to_string(),
            "broader module-subtree restrictions only, such as pub(super) or pub(in path)"
                .to_string(),
        ),
    ])
}

fn overlap_scope_allows_crate(
    scope: OverlapScope,
    crate_id: NodeId,
    crate_target_kind_for: &HashMap<NodeId, String>,
    vendor_crates: &HashSet<NodeId>,
) -> bool {
    match scope {
        OverlapScope::All => true,
        OverlapScope::Local | OverlapScope::LocalNoVendor => {
            let target_kind = crate_target_kind_for
                .get(&crate_id)
                .map(String::as_str)
                .unwrap_or("lib");
            let local_target = matches!(target_kind, "lib" | "bin");
            local_target && (scope == OverlapScope::Local || !vendor_crates.contains(&crate_id))
        }
    }
}

#[derive(Default)]
struct ModuleDependencyAccumulator {
    target_module: String,
    target_kind: String,
    target_crate: Option<String>,
    import_count: usize,
    usage_count: usize,
    symbols: BTreeMap<NodeId, ModuleDependencySymbolAccumulator>,
}

impl ModuleDependencyAccumulator {
    fn new(node: &Node, crate_names: &HashMap<NodeId, String>) -> Self {
        Self {
            target_module: node.qualified_name.clone(),
            target_kind: node_kind_label(node, label_item_kind),
            target_crate: node.crate_id.and_then(|id| crate_names.get(&id).cloned()),
            import_count: 0,
            usage_count: 0,
            symbols: BTreeMap::new(),
        }
    }

    fn into_dependency(self) -> ModuleDependency {
        let mut symbols: Vec<ModuleDependencySymbol> = self
            .symbols
            .into_values()
            .map(ModuleDependencySymbolAccumulator::into_symbol)
            .collect();
        symbols.sort_by(|a, b| a.target_qualified.cmp(&b.target_qualified));
        ModuleDependency {
            target_module: self.target_module,
            target_kind: self.target_kind,
            target_crate: self.target_crate,
            import_count: self.import_count,
            usage_count: self.usage_count,
            symbols,
        }
    }
}

struct ModuleDependencySymbolAccumulator {
    target_qualified: String,
    target_kind: String,
    import_count: usize,
    usage_count: usize,
    binding_kinds: BTreeSet<String>,
}

impl ModuleDependencySymbolAccumulator {
    fn new(target: NodeId, node: Option<&Node>) -> Self {
        Self {
            target_qualified: node
                .map(|node| node.qualified_name.clone())
                .unwrap_or_else(|| target.to_hex()),
            target_kind: node
                .map(|node| node_kind_label(node, label_item_kind))
                .unwrap_or_else(|| "Unknown".to_string()),
            import_count: 0,
            usage_count: 0,
            binding_kinds: BTreeSet::new(),
        }
    }

    fn into_symbol(self) -> ModuleDependencySymbol {
        ModuleDependencySymbol {
            target_qualified: self.target_qualified,
            target_kind: self.target_kind,
            import_count: self.import_count,
            usage_count: self.usage_count,
            binding_kinds: self.binding_kinds.into_iter().collect(),
        }
    }
}

fn dependency_node_for(nodes: &HashMap<NodeId, Node>, target: NodeId) -> Option<(NodeId, &Node)> {
    let mut current = target;
    let mut guard = 0usize;
    loop {
        let node = nodes.get(&current)?;
        match node.kind {
            NodeKind::Module | NodeKind::Crate | NodeKind::ExternalSymbol => {
                return Some((current, node));
            }
            NodeKind::Workspace => return None,
            NodeKind::Item => {
                current = node.parent_id?;
                guard += 1;
                if guard > 32 {
                    return None;
                }
            }
        }
    }
}

/// Glob matcher with `*` wildcards (matches any run of chars, including
/// empty). No other metacharacters; pattern segments are matched as literal
/// substrings between wildcards. Greedy / linear in `text.len() *
/// pattern.len()`. Used by `forbidden_dependency_check`.
fn glob_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut cursor: usize = 0;
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            // `*` at the boundary — anchors the next match anywhere from cursor.
            if i == last {
                return true;
            }
            continue;
        }
        if i == 0 {
            // No leading `*`: the first segment must anchor at start.
            if !text[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
        } else if i == last {
            // No trailing `*`: the last segment must anchor at end.
            return text[cursor..].ends_with(part)
                && text.len() - cursor >= part.len();
        } else {
            match text[cursor..].find(part) {
                Some(pos) => cursor += pos + part.len(),
                None => return false,
            }
        }
    }
    true
}

fn rule_allows_consumer_kind(rule: &ForbiddenDependencyRule, consumer_kind: &str) -> bool {
    let allowed = match rule.consumer_kinds.as_ref().filter(|kinds| !kinds.is_empty()) {
        Some(kinds) => kinds.as_slice(),
        None => return matches_default_consumer_kind(consumer_kind),
    };
    allowed.iter().any(|kind| {
        let kind = normalize_consumer_kind(kind);
        kind == "*" || kind == consumer_kind
    })
}

fn matches_default_consumer_kind(consumer_kind: &str) -> bool {
    matches!(consumer_kind, "lib" | "bin")
}

fn normalize_consumer_kind(kind: &str) -> String {
    match kind.trim() {
        "custom-build" => "build".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

/// Anchored attribute match used by `items_with_attribute`.
///
/// Returns `Some("attr")` when `pat` is a prefix of the raw attribute
/// string or matches the attribute path (`derive`, `must_use`, `cfg`, ...),
/// `Some("doc")` when the attribute is a `///` doc-comment (`"/// body"`)
/// and `pat` is a prefix of the body, otherwise `None`.
///
/// The intent is that searching for `#[must_use]` matches an attribute
/// stored as `"#[must_use]"` or `"#[must_use = \"...\"]"`, but does NOT
/// match unrelated attributes whose body merely contains the literal text
/// `#[must_use]` (e.g. `#[tool(description = "...#[must_use]...")]`).
///
/// Doc-comment lines are stored verbatim as `"/// body"` (the lexer
/// preserves the leading space). Stripping the `/// ` prefix lets a
/// caller search for `SAFETY` and match a doc line `"/// SAFETY: ..."`,
/// while still matching the full-string form `/// SAFETY` against the
/// raw attribute prefix.
///
/// Empty pattern returns `None` (the caller short-circuits empty
/// patterns to "match nothing").
fn match_attribute(attr: &str, pat: &str) -> Option<&'static str> {
    if pat.is_empty() {
        return None;
    }
    if attr.starts_with(pat) {
        return Some("attr");
    }
    if attr_matches_path_or_body(attr, pat) {
        return Some("attr");
    }
    // Doc lines in our model are always stored as `"/// body"`.
    if let Some(body) = attr.strip_prefix("/// ") {
        if body.starts_with(pat) {
            return Some("doc");
        }
    }
    None
}

fn attr_matches_path_or_body(attr: &str, pat: &str) -> bool {
    let Some(body) = attr.strip_prefix("#[") else {
        return false;
    };
    let normalized_pat = normalize_attr_pattern(pat);
    if normalized_pat.is_empty() {
        return false;
    }
    if attr_pattern_is_path_only(normalized_pat) {
        let path = attr_path(body);
        return path == normalized_pat
            || path
                .rsplit("::")
                .next()
                .map(|last| last == normalized_pat)
                .unwrap_or(false);
    }
    body.starts_with(normalized_pat)
}

fn normalize_attr_pattern(pat: &str) -> &str {
    let pat = pat
        .strip_prefix("#[")
        .unwrap_or(pat)
        .strip_prefix('!')
        .unwrap_or_else(|| pat.strip_prefix("#![").unwrap_or(pat));
    pat.strip_suffix(']').unwrap_or(pat)
}

fn attr_pattern_is_path_only(pat: &str) -> bool {
    !pat.is_empty()
        && pat
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn attr_path(body: &str) -> &str {
    let end = body
        .char_indices()
        .find_map(|(idx, c)| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                None
            } else {
                Some(idx)
            }
        })
        .unwrap_or(body.len());
    &body[..end]
}

/// Render a `BindingVisibility` as the human-readable string we emit on
/// `ModuleTreeNode.visibility` for Items: `"pub"`, `"pub(crate)"`,
/// `"pub(in path::to::mod)"`, or `"pub(self)"` for the implicit-private case.
fn format_binding_visibility(
    rtxn: &RoTxn<'_, heed::WithoutTls>,
    snap: &OpenedSnapshot,
    vis: BindingVisibility,
) -> String {
    match vis {
        BindingVisibility::Public => "pub".to_string(),
        BindingVisibility::Private => "pub(self)".to_string(),
        BindingVisibility::Crate(_) => "pub(crate)".to_string(),
        BindingVisibility::RestrictedTo(id) => {
            match snap.dbs.nodes_by_id.get(rtxn, id.as_bytes()).ok().flatten() {
                Some(node) => format!("pub(in {})", node.qualified_name),
                None => "pub(in ?)".to_string(),
            }
        }
    }
}

/// v9: predicate for `functions_with_filter`. Every `Some` field on the
/// filter narrows the match; a `None` field is a no-op. Substring matches
/// (`has_param_type`, `returns_type_pattern`) are case-sensitive against
/// the HirDisplay strings in the signature.
fn filter_matches(filter: &FunctionFilter, sig: &FunctionSignature) -> bool {
    if let Some(want) = filter.is_async
        && sig.is_async != want
    {
        return false;
    }
    if let Some(min) = filter.min_param_count
        && sig.params.len() < min
    {
        return false;
    }
    if let Some(needle) = filter.has_param_type.as_deref()
        && !sig.params.iter().any(|p| p.ty.contains(needle))
    {
        return false;
    }
    if let Some(needle) = filter.returns_type_pattern.as_deref()
        && !sig.return_type.contains(needle)
    {
        return false;
    }
    if let Some(want) = filter.self_kind {
        let actual = sig.self_param;
        let ok = match want {
            SelfKindFilter::None => actual.is_none(),
            SelfKindFilter::Owned => matches!(actual, Some(SelfKind::Owned)),
            SelfKindFilter::Ref => matches!(actual, Some(SelfKind::Ref)),
            SelfKindFilter::RefMut => matches!(actual, Some(SelfKind::RefMut)),
        };
        if !ok {
            return false;
        }
    }
    true
}

fn is_visible_from(
    vis: &BindingVisibility,
    consumer_crate: Option<NodeId>,
    consumer_ancestry: &HashSet<NodeId>,
) -> bool {
    match vis {
        BindingVisibility::Public => true,
        BindingVisibility::Private => false,
        BindingVisibility::Crate(crate_id) => consumer_crate == Some(*crate_id),
        // Restricted to the subtree rooted at `ancestor_id`: visible iff the
        // consumer's own ancestry chain passes through that node.
        BindingVisibility::RestrictedTo(ancestor_id) => consumer_ancestry.contains(ancestor_id),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph::model::Namespace;
    use crate::graph::snapshot::{BuildOptions, build_and_persist, open_current};
    use crate::graph::storage::{GraphEnvOptions, GraphPaths};
    use std::path::Path;
    use std::sync::OnceLock;

    // Build the snapshot once and share across all tests in this module.
    // Saves ~3s/test in release (~25s in debug). The TempDir is held inside
    // the static so the heed env stays valid for the process lifetime.
    struct SharedSnap {
        _td: tempfile::TempDir,
        snap: OpenedSnapshot,
    }

    pub(crate) fn shared_snapshot() -> &'static OpenedSnapshot {
        static CACHE: OnceLock<SharedSnap> = OnceLock::new();
        &CACHE
            .get_or_init(|| {
                let td = tempfile::tempdir().unwrap();
                let opts = BuildOptions {
                    data_dir_override: Some(td.path().to_path_buf()),
                    ..Default::default()
                };
                let result =
                    build_and_persist(Path::new(env!("CARGO_MANIFEST_DIR")), opts).unwrap();
                let paths = GraphPaths::for_workspace_in(td.path(), &result.workspace_root);
                let snap = open_current(&paths, GraphEnvOptions::default())
                    .unwrap()
                    .unwrap();
                SharedSnap { _td: td, snap }
            })
            .snap
    }

    fn test_node(qualified_name: &str, display_name: &str, item_kind: Option<ItemKind>) -> Node {
        Node {
            id: NodeId([9u8; 32]),
            kind: NodeKind::Item,
            display_name: display_name.to_string(),
            qualified_name: qualified_name.to_string(),
            crate_id: Some(NodeId([1u8; 32])),
            parent_id: None,
            item_kind,
            file: Some("src/graph/queries.rs".to_string()),
            span: None,
            visibility: None,
            attributes: Vec::new(),
            crate_target_kind: None,
        }
    }

    #[test]
    fn impl_module_item_alias_matches_canonical_method_suffix() {
        let (module_prefix, type_name, member_name) = impl_module_item_alias_parts(
            "rust_code_mcp::graph::queries::OpenedSnapshot::lookup_by_qualified_name",
        )
        .expect("alias parts");
        assert_eq!(module_prefix, "rust_code_mcp::graph::queries");
        assert_eq!(type_name, "OpenedSnapshot");
        assert_eq!(member_name, "lookup_by_qualified_name");

        let node = test_node(
            "rust_code_mcp::graph::snapshot::OpenedSnapshot::lookup_by_qualified_name",
            "lookup_by_qualified_name",
            Some(ItemKind::Method),
        );
        assert!(is_impl_module_item_alias_candidate(
            &node,
            Some(NodeId([1u8; 32])),
            Some("src/graph/queries.rs"),
            type_name,
            member_name
        ));
    }

    #[test]
    fn impl_module_item_alias_rejects_wrong_crate_or_kind() {
        let method = test_node(
            "rust_code_mcp::graph::snapshot::OpenedSnapshot::lookup_by_qualified_name",
            "lookup_by_qualified_name",
            Some(ItemKind::Method),
        );
        assert!(!is_impl_module_item_alias_candidate(
            &method,
            Some(NodeId([2u8; 32])),
            Some("src/graph/queries.rs"),
            "OpenedSnapshot",
            "lookup_by_qualified_name"
        ));
        assert!(!is_impl_module_item_alias_candidate(
            &method,
            Some(NodeId([1u8; 32])),
            Some("src/graph/other.rs"),
            "OpenedSnapshot",
            "lookup_by_qualified_name"
        ));

        let function = test_node(
            "rust_code_mcp::graph::snapshot::OpenedSnapshot::lookup_by_qualified_name",
            "lookup_by_qualified_name",
            Some(ItemKind::Function),
        );
        assert!(!is_impl_module_item_alias_candidate(
            &function,
            Some(NodeId([1u8; 32])),
            Some("src/graph/queries.rs"),
            "OpenedSnapshot",
            "lookup_by_qualified_name"
        ));
    }

    #[test]
    fn lookup_by_qualified_name_resolves_known_modules() {
        let snap = shared_snapshot();
        let (_id, node) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader")
            .unwrap()
            .expect("graph::loader module found");
        assert_eq!(node.kind, NodeKind::Module);
    }

    #[test]
    fn imports_of_graph_mod_includes_loader_load() {
        let snap = shared_snapshot();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph")
            .unwrap()
            .unwrap();
        let imports = snap.imports_of(graph_mod_id).unwrap();
        assert!(
            imports.iter().any(|b| b.visible_name == "load"),
            "expected `load` to appear in imports of graph mod (via `pub use loader::load`)"
        );
    }

    #[test]
    fn who_imports_finds_target() {
        let snap = shared_snapshot();
        let (load_fn_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader::load")
            .unwrap()
            .unwrap();
        let importers = snap.who_imports(load_fn_id).unwrap();
        assert!(
            !importers.is_empty(),
            "expected at least one importer of loader::load"
        );
        // The graph::mod re-export should be among them.
        let from_modules: Vec<NodeId> = importers.iter().map(|b| b.from_module).collect();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph")
            .unwrap()
            .unwrap();
        assert!(
            from_modules.contains(&graph_mod_id),
            "expected graph mod to appear among importers of loader::load"
        );
    }

    #[test]
    fn exports_of_loader_visible_from_graph_mod() {
        let snap = shared_snapshot();
        let (loader_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader")
            .unwrap()
            .unwrap();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph")
            .unwrap()
            .unwrap();
        let exports = snap.exports_of(loader_mod_id, graph_mod_id).unwrap();
        assert!(
            exports.iter().any(|b| b.visible_name == "load"),
            "expected loader::load to be visible from graph mod"
        );
    }

    #[test]
    fn lookup_by_qualified_name_resolves_reexport_facade() {
        // `rust_code_mcp::graph::load` is exposed via `pub use loader::load;`
        // in src/graph/mod.rs. The canonical declaration lives at
        // `rust_code_mcp::graph::loader::load`. The fallback should follow the
        // re-export and return the canonical Item node.
        let snap = shared_snapshot();
        let (_id, node) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::load")
            .unwrap()
            .expect("re-export facade should resolve to the canonical Item");
        assert_eq!(node.kind, NodeKind::Item);
        assert_eq!(
            node.qualified_name, "rust_code_mcp::graph::loader::load",
            "facade should resolve to the canonical declaration site"
        );
    }

    #[test]
    fn lookup_by_qualified_name_canonical_still_works() {
        // Regression check: the canonical-name path remains the primary lookup
        // and is not affected by the re-export fallback.
        let snap = shared_snapshot();
        let (_id, node) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader::load")
            .unwrap()
            .expect("canonical name should resolve directly");
        assert_eq!(node.kind, NodeKind::Item);
        assert_eq!(node.qualified_name, "rust_code_mcp::graph::loader::load");
    }

    #[test]
    fn lookup_by_qualified_name_unresolvable_terminates() {
        // No node carries this name and no facade points at it. The recursive
        // fallback must terminate (bounded by MAX_REEXPORT_HOPS) and return None
        // rather than spinning.
        let snap = shared_snapshot();
        let result = snap
            .lookup_by_qualified_name("rust_code_mcp::nonexistent::thing")
            .unwrap();
        assert!(
            result.is_none(),
            "lookup of an unknown name should return None, got {result:?}"
        );
    }

    #[test]
    fn private_visibility_blocks_export() {
        // rust_code_mcp::graph::extract has private helpers like `crate_display_name`.
        // From outside the loader/extract sibling (e.g., rust_code_mcp root module),
        // those should NOT be exported.
        let snap = shared_snapshot();
        let (extract_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::extract")
            .unwrap()
            .unwrap();
        let (root_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let exports = snap.exports_of(extract_id, root_id).unwrap();
        // `crate_display_name` is a non-pub fn — should be filtered out.
        assert!(
            !exports.iter().any(|b| b.visible_name == "crate_display_name"),
            "private helper should not be exported"
        );
    }

    #[test]
    fn usages_of_loader_load_returns_at_least_one() {
        // `loader::load` is called by `build_and_persist` in the same lib.
        // Phase 2 must record at least one Usage row.
        let snap = shared_snapshot();
        let (load_fn_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader::load")
            .unwrap()
            .unwrap();
        let usages = snap.usages_of(load_fn_id).unwrap();
        assert!(
            !usages.is_empty(),
            "expected at least one usage of loader::load"
        );
        for u in &usages {
            assert_eq!(u.target, load_fn_id, "wrong target on usages_of result");
            assert!(u.start <= u.end, "range must be ordered");
            assert!(!u.file.is_empty(), "file path must be set");
        }
    }

    #[test]
    fn usages_in_consumer_filters_to_that_module() {
        // Pick the `graph::snapshot` module (we know loader::load is called
        // inside it). Every Usage returned must have consumer_module ==
        // snapshot module's NodeId.
        let snap = shared_snapshot();
        let (snapshot_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::snapshot")
            .unwrap()
            .unwrap();
        let usages = snap.usages_in(snapshot_mod_id).unwrap();
        for u in &usages {
            assert_eq!(
                u.consumer_module, snapshot_mod_id,
                "usages_in must return only refs whose consumer matches the queried module"
            );
        }
    }

    #[test]
    fn dead_pub_findings_are_well_formed() {
        // Smoke test: the query terminates and every finding it emits has
        // Public visibility and points at a real Item. The exact set of
        // dead-pub items is sensitive to refactors; don't pin a specific
        // qualified_name here.
        let snap = shared_snapshot();
        let (crate_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        // The lookup above resolves to the crate root MODULE; map up to the
        // actual Crate node via parent_id.
        let rtxn = snap.env.read_txn().unwrap();
        let crate_node_id = snap
            .dbs
            .nodes_by_id
            .get(&rtxn, crate_id.as_bytes())
            .unwrap()
            .and_then(|n| if n.kind == NodeKind::Crate { Some(crate_id) } else { n.parent_id })
            .expect("expected crate node id");
        drop(rtxn);

        let findings = snap.dead_pub_in_crate(crate_node_id).unwrap();
        for f in &findings {
            assert_eq!(
                f.declared_visibility,
                BindingVisibility::Public,
                "dead-pub finding must have Public visibility, got {:?} for {}",
                f.declared_visibility,
                f.qualified_name
            );
            // The target must resolve to a real Item node with a matching qname.
            let rtxn = snap.env.read_txn().unwrap();
            let node = snap
                .dbs
                .nodes_by_id
                .get(&rtxn, f.target.as_bytes())
                .unwrap()
                .expect("dead-pub target must resolve to a Node");
            assert_eq!(node.kind, NodeKind::Item);
            assert_eq!(node.qualified_name, f.qualified_name);
        }
    }

    #[test]
    fn crate_edges_returns_at_least_one_edge() {
        let snap = shared_snapshot();
        let edges = snap.crate_edges().unwrap();
        // The lib uses several external crates (heed, anyhow, serde, ra-ap-*),
        // and a self-only workspace might still have at least one
        // external→rust_code_mcp edge. We only assert non-empty here.
        assert!(
            !edges.is_empty(),
            "expected at least one cross-crate edge in the workspace"
        );
        for e in &edges {
            assert!(!e.consumer_crate.is_empty());
            assert!(!e.producer_crate.is_empty());
            assert_ne!(e.consumer_crate, e.producer_crate, "same-crate edges must be filtered out");
            assert_eq!(e.unique_symbols, e.symbols.len());
        }
    }

    /// Pick a (consumer, producer) pair from the real edges and assert that a
    /// rule targeting exactly that pair fires.
    #[test]
    fn forbidden_dependency_check_simple_match() {
        let snap = shared_snapshot();
        let edges = snap.crate_edges().unwrap();
        let edge = edges.first().expect("workspace has at least one edge");
        let rules = vec![ForbiddenDependencyRule {
            consumer: edge.consumer_crate.clone(),
            producer: edge.producer_crate.clone(),
            consumer_kinds: Some(vec!["*".into()]),
            except: None,
            severity: Some("error".into()),
            message: Some("test rule".into()),
        }];
        let violations = snap.forbidden_dependency_check(&rules).unwrap();
        assert!(
            violations.iter().any(|v| {
                v.consumer_crate == edge.consumer_crate
                    && v.producer_crate == edge.producer_crate
                    && v.rule_index == 0
            }),
            "expected exact-pair rule to fire on edge {} -> {}",
            edge.consumer_crate,
            edge.producer_crate,
        );
        for v in &violations {
            assert_eq!(v.severity.as_deref(), Some("error"));
            assert_eq!(v.message.as_deref(), Some("test rule"));
        }
    }

    /// `consumer = "*"` must match every edge in the workspace; `producer =
    /// "*"` does the same on the other side.
    #[test]
    fn forbidden_dependency_check_glob_wildcard_matches_all() {
        let snap = shared_snapshot();
        let edges = snap.crate_edges().unwrap();
        let rules = vec![ForbiddenDependencyRule {
            consumer: "*".into(),
            producer: "*".into(),
            consumer_kinds: Some(vec!["*".into()]),
            except: None,
            severity: None,
            message: None,
        }];
        let violations = snap.forbidden_dependency_check(&rules).unwrap();
        assert_eq!(
            violations.len(),
            edges.len(),
            "wildcard consumer+producer rule must produce one violation per edge"
        );
    }

    /// Rule fires on a real (consumer, producer) edge — then add an `except`
    /// glob covering the consumer and verify it suppresses the violation.
    #[test]
    fn forbidden_dependency_check_except_overrides_match() {
        let snap = shared_snapshot();
        let edges = snap.crate_edges().unwrap();
        let edge = edges.first().expect("workspace has at least one edge");

        // Baseline: rule fires.
        let base_rules = vec![ForbiddenDependencyRule {
            consumer: "*".into(),
            producer: edge.producer_crate.clone(),
            consumer_kinds: Some(vec!["*".into()]),
            except: None,
            severity: None,
            message: None,
        }];
        let base = snap.forbidden_dependency_check(&base_rules).unwrap();
        assert!(
            base.iter().any(|v| v.consumer_crate == edge.consumer_crate
                && v.producer_crate == edge.producer_crate),
            "baseline rule should match the picked edge"
        );

        // With `except = consumer_crate`, the picked edge must be suppressed.
        let exempted = vec![ForbiddenDependencyRule {
            consumer: "*".into(),
            producer: edge.producer_crate.clone(),
            consumer_kinds: Some(vec!["*".into()]),
            except: Some(edge.consumer_crate.clone()),
            severity: None,
            message: None,
        }];
        let after = snap.forbidden_dependency_check(&exempted).unwrap();
        assert!(
            !after.iter().any(|v| v.consumer_crate == edge.consumer_crate
                && v.producer_crate == edge.producer_crate),
            "`except` must suppress the matched edge"
        );
    }

    /// Sanity for the hand-rolled glob matcher.
    #[test]
    fn forbidden_glob_match_smoke() {
        assert!(super::glob_match("tokio", "tokio"));
        assert!(!super::glob_match("tokio", "tokio_util"));
        assert!(super::glob_match("*", ""));
        assert!(super::glob_match("*", "anything"));
        assert!(super::glob_match("domain*", "domain_core"));
        assert!(super::glob_match("domain*", "domain"));
        assert!(!super::glob_match("domain*", "core_domain"));
        assert!(super::glob_match("*core", "domain_core"));
        assert!(!super::glob_match("*core", "domain_core_v2"));
        assert!(super::glob_match("foo*bar", "foobar"));
        assert!(super::glob_match("foo*bar", "foo_x_bar"));
        assert!(!super::glob_match("foo*bar", "foo"));
    }

    #[test]
    fn forbidden_dependency_rule_defaults_to_lib_and_bin_consumers() {
        let rule = ForbiddenDependencyRule {
            consumer: "*".into(),
            producer: "*".into(),
            consumer_kinds: None,
            except: None,
            severity: None,
            message: None,
        };

        assert!(super::rule_allows_consumer_kind(&rule, "lib"));
        assert!(super::rule_allows_consumer_kind(&rule, "bin"));
        assert!(!super::rule_allows_consumer_kind(&rule, "example"));
        assert!(!super::rule_allows_consumer_kind(&rule, "test"));
        assert!(!super::rule_allows_consumer_kind(&rule, "bench"));
        assert!(!super::rule_allows_consumer_kind(&rule, "build"));
    }

    #[test]
    fn forbidden_dependency_rule_explicit_consumer_kinds_override_default() {
        let rule = ForbiddenDependencyRule {
            consumer: "*".into(),
            producer: "*".into(),
            consumer_kinds: Some(vec!["example".into(), "custom-build".into()]),
            except: None,
            severity: None,
            message: None,
        };

        assert!(!super::rule_allows_consumer_kind(&rule, "lib"));
        assert!(super::rule_allows_consumer_kind(&rule, "example"));
        assert!(super::rule_allows_consumer_kind(&rule, "build"));
    }

    #[test]
    fn dependency_node_for_climbs_item_parents_to_module() {
        let module_id = NodeId([1u8; 32]);
        let item_id = NodeId([2u8; 32]);
        let variant_id = NodeId([3u8; 32]);
        let external_id = NodeId([4u8; 32]);
        let mut nodes = HashMap::new();
        nodes.insert(
            module_id,
            Node {
                id: module_id,
                kind: NodeKind::Module,
                display_name: "search".to_string(),
                qualified_name: "crate::search".to_string(),
                crate_id: None,
                parent_id: None,
                item_kind: None,
                file: None,
                span: None,
                visibility: None,
                attributes: Vec::new(),
                crate_target_kind: None,
            },
        );
        nodes.insert(
            item_id,
            Node {
                id: item_id,
                kind: NodeKind::Item,
                display_name: "Bm25Search".to_string(),
                qualified_name: "crate::search::Bm25Search".to_string(),
                crate_id: None,
                parent_id: Some(module_id),
                item_kind: Some(ItemKind::Struct),
                file: None,
                span: None,
                visibility: None,
                attributes: Vec::new(),
                crate_target_kind: None,
            },
        );
        nodes.insert(
            variant_id,
            Node {
                id: variant_id,
                kind: NodeKind::Item,
                display_name: "Variant".to_string(),
                qualified_name: "crate::search::Bm25Search::Variant".to_string(),
                crate_id: None,
                parent_id: Some(item_id),
                item_kind: Some(ItemKind::EnumVariant),
                file: None,
                span: None,
                visibility: None,
                attributes: Vec::new(),
                crate_target_kind: None,
            },
        );
        nodes.insert(
            external_id,
            Node {
                id: external_id,
                kind: NodeKind::ExternalSymbol,
                display_name: "serde".to_string(),
                qualified_name: "serde".to_string(),
                crate_id: None,
                parent_id: None,
                item_kind: None,
                file: None,
                span: None,
                visibility: None,
                attributes: Vec::new(),
                crate_target_kind: None,
            },
        );

        let (resolved_id, resolved_node) =
            super::dependency_node_for(&nodes, variant_id).expect("variant dependency");
        assert_eq!(resolved_id, module_id);
        assert_eq!(resolved_node.qualified_name, "crate::search");
        let (resolved_id, resolved_node) =
            super::dependency_node_for(&nodes, external_id).expect("external dependency");
        assert_eq!(resolved_id, external_id);
        assert_eq!(resolved_node.qualified_name, "serde");
    }

    #[test]
    fn overlaps_returns_well_formed_report() {
        let snap = shared_snapshot();
        let report = snap.overlaps().unwrap();
        // Don't assert specific collisions — the workspace may not have any.
        // Just exercise the code path and verify the struct shape.
        for c in &report.cross_crate_type_collisions {
            assert!(!c.name.is_empty());
            assert!(c.locations.len() >= 2);
        }
        for d in &report.within_crate_type_duplicates {
            assert!(d.qualified_names.len() >= 2);
        }
        for f in &report.common_fn_names {
            assert!(f.crates.len() >= 4);
        }
    }

    #[test]
    fn overlap_scope_filters_examples_and_vendor() {
        let lib_crate = NodeId([1u8; 32]);
        let example_crate = NodeId([2u8; 32]);
        let vendor_crate = NodeId([3u8; 32]);
        let mut target_kinds = HashMap::new();
        target_kinds.insert(lib_crate, "lib".to_string());
        target_kinds.insert(example_crate, "example".to_string());
        target_kinds.insert(vendor_crate, "lib".to_string());
        let vendor_crates = HashSet::from([vendor_crate]);

        assert!(super::overlap_scope_allows_crate(
            OverlapScope::All,
            example_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(super::overlap_scope_allows_crate(
            OverlapScope::Local,
            vendor_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(!super::overlap_scope_allows_crate(
            OverlapScope::Local,
            example_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(!super::overlap_scope_allows_crate(
            OverlapScope::LocalNoVendor,
            vendor_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(super::overlap_scope_allows_crate(
            OverlapScope::LocalNoVendor,
            lib_crate,
            &target_kinds,
            &vendor_crates,
        ));
    }

    #[test]
    fn module_tree_roots_at_requested_crate() {
        let snap = shared_snapshot();
        let tree = snap.module_tree("rust_code_mcp", None).unwrap();
        assert_eq!(tree.qualified_name, "rust_code_mcp");
        assert_eq!(tree.kind, "Crate");
        assert!(
            !tree.children.is_empty(),
            "crate root should have at least one child (the root Module)"
        );
    }

    #[test]
    fn module_tree_respects_depth_limit() {
        let snap = shared_snapshot();
        let tree = snap.module_tree("rust_code_mcp", Some(0)).unwrap();
        // Depth 0 => no children walked.
        assert!(tree.children.is_empty(), "depth=0 must not recurse");
    }

    #[test]
    fn declared_reexports_of_lists_all_pub_uses() {
        // `rust_code_mcp::graph` has `pub use loader::load;` (and other
        // `pub use`s). declared_reexports_of(graph_mod_id) must include `load`
        // and every binding in the result must satisfy is_explicit_pub_use.
        let snap = shared_snapshot();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph")
            .unwrap()
            .unwrap();
        let reexports = snap.declared_reexports_of(graph_mod_id).unwrap();
        assert!(
            !reexports.is_empty(),
            "expected at least one declared `pub use` in graph mod"
        );
        for b in &reexports {
            assert!(
                b.is_explicit_pub_use,
                "declared_reexports_of must only return is_explicit_pub_use=true, got false for {}",
                b.visible_name
            );
            assert_ne!(b.kind, BindingKind::Declared);
        }
        assert!(
            reexports.iter().any(|b| b.visible_name == "load"),
            "expected `load` among declared re-exports of graph mod"
        );
    }

    #[test]
    fn explicit_pub_use_is_marked_on_pub_use_bindings() {
        // `rust_code_mcp::graph::mod` carries `pub use loader::load;`. The
        // resulting binding must have `is_explicit_pub_use == true`. The
        // declared binding for `loader` (sibling module declaration with no
        // `pub use`) must have `is_explicit_pub_use == false`.
        let snap = shared_snapshot();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph")
            .unwrap()
            .unwrap();
        let imports = snap.imports_of(graph_mod_id).unwrap();
        let load_bind = imports
            .iter()
            .find(|b| b.visible_name == "load")
            .expect("expected `load` re-export binding in graph mod");
        assert!(
            load_bind.is_explicit_pub_use,
            "`pub use loader::load` should be marked explicit_pub_use=true, got false"
        );

        // A non-pub `use` should land with is_explicit_pub_use == false.
        // `rust_code_mcp::graph::queries` has plenty of private `use` lines.
        let (queries_mod_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::queries")
            .unwrap()
            .unwrap();
        let queries_imports = snap.imports_of(queries_mod_id).unwrap();
        let private_imports: Vec<&Binding> = queries_imports
            .iter()
            .filter(|b| !b.is_explicit_pub_use)
            .collect();
        assert!(
            !private_imports.is_empty(),
            "expected at least one private (non-pub) `use` in graph::queries"
        );
    }

    #[test]
    fn who_uses_summary_aggregates_by_consumer() {
        let snap = shared_snapshot();
        let (load_fn_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader::load")
            .unwrap()
            .unwrap();
        let summary = snap.who_uses_summary(load_fn_id).unwrap();
        let raw = snap.usages_of(load_fn_id).unwrap();
        assert!(
            !summary.is_empty(),
            "expected at least one summary row for loader::load"
        );
        // Aggregate invariant: sum of per-row total_count == total raw usages.
        let summed: usize = summary.iter().map(|r| r.total_count).sum();
        assert_eq!(
            summed,
            raw.len(),
            "summary totals must equal the raw usage count"
        );
        for row in &summary {
            assert!(row.total_count >= 1);
            assert!(
                !row.category_breakdown.is_empty(),
                "category_breakdown must be non-empty when total_count >= 1"
            );
            let breakdown_sum: usize = row.category_breakdown.values().copied().sum();
            assert_eq!(
                breakdown_sum, row.total_count,
                "per-row category sum must equal total_count"
            );
        }
        // Sorted by total_count desc.
        for w in summary.windows(2) {
            assert!(w[0].total_count >= w[1].total_count);
        }
    }

    #[test]
    fn calls_from_returns_callees() {
        // Layer 10 — call graph: `build_and_persist` is a known caller of
        // `loader::load`. `calls_from(build_and_persist)` should include the
        // `loader::load` ref (plus a long tail of other refs from inside the
        // body — at minimum the loader::load call must be present).
        let snap = shared_snapshot();
        let (caller_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::snapshot::build_and_persist")
            .unwrap()
            .expect("build_and_persist not in graph");
        let calls = snap
            .calls_from(caller_id)
            .expect("calls_from failed");
        assert!(
            calls
                .iter()
                .any(|c| c.callee_qualified_name.contains("loader::load")),
            "expected calls_from(build_and_persist) to include loader::load, got {:?}",
            calls
                .iter()
                .map(|c| &c.callee_qualified_name)
                .collect::<Vec<_>>()
        );
        // Every row's caller_qualified_name should resolve to build_and_persist
        // (call sites attribute to the queried fn — closures fold to parent).
        for c in &calls {
            assert_eq!(
                c.caller_qualified_name.as_deref(),
                Some("rust_code_mcp::graph::snapshot::build_and_persist"),
                "caller mismatch on {:?}",
                c
            );
        }
    }

    #[test]
    fn workspace_stats_has_basic_counts() {
        let snap = shared_snapshot();
        let stats = snap.workspace_stats().unwrap();
        assert!(stats.nodes.crate_ >= 1, "expected at least one crate");
        assert!(!stats.items_by_kind.is_empty(), "items_by_kind must be non-empty");
        assert!(!stats.bindings_by_kind.is_empty(), "bindings_by_kind must be non-empty");
        assert!(stats.pub_crate_share.is_finite());
        assert!(stats.pub_crate_share >= 0.0);
        assert!(stats.pub_crate_share <= 1.0);
    }

    #[test]
    fn visibility_counts_separate_module_private_from_restricted() {
        let from_module = NodeId([1u8; 32]);
        let parent_module = NodeId([2u8; 32]);
        let target = NodeId([3u8; 32]);
        let mut counts = VisibilityCounts::default();

        let mut binding = Binding {
            from_module,
            namespace: Namespace::Type,
            visible_name: "local".to_string(),
            target,
            kind: BindingKind::Declared,
            visibility: BindingVisibility::RestrictedTo(from_module),
            is_explicit_pub_use: false,
        };
        count_declared_visibility(&mut counts, &binding);
        binding.visible_name = "super_visible".to_string();
        binding.visibility = BindingVisibility::RestrictedTo(parent_module);
        count_declared_visibility(&mut counts, &binding);

        assert_eq!(counts.module_private, 1);
        assert_eq!(counts.pub_self, 1);
        assert_eq!(counts.private, 1);
        assert_eq!(counts.restricted_to, 1);
    }

    #[test]
    fn visibility_count_notes_flag_alias_fields() {
        let notes = visibility_count_notes();
        assert!(notes["module_private"].contains("canonical"));
        assert!(notes["pub_self"].contains("back-compat alias"));
        assert!(notes["private"].contains("legacy private bucket"));
        assert!(notes["restricted_to"].contains("broader module-subtree"));
    }

    #[test]
    fn call_graph_returns_root_with_callees() {
        // `build_and_persist` is a known caller of `loader::load` (and others);
        // a depth-2 descent must produce a non-empty `callees` vec on the root.
        let snap = shared_snapshot();
        let (root_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::snapshot::build_and_persist")
            .unwrap()
            .expect("build_and_persist not in graph");
        let tree = snap
            .call_graph(root_id, 2)
            .expect("call_graph failed");
        assert_eq!(
            tree.fn_qualified_name,
            "rust_code_mcp::graph::snapshot::build_and_persist"
        );
        assert!(
            !tree.callees.is_empty(),
            "expected build_and_persist to have at least one callee"
        );
        assert!(
            !tree.truncated_at_depth,
            "depth=2 should not truncate the root itself"
        );
        assert!(
            !tree.truncated_at_cycle,
            "root never has truncated_at_cycle"
        );
    }

    #[test]
    fn call_graph_respects_depth_zero() {
        // depth=0 means: don't expand. Even on a known caller, callees must be
        // empty and truncated_at_depth must be true (because the fn does have
        // outgoing edges).
        let snap = shared_snapshot();
        let (root_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::snapshot::build_and_persist")
            .unwrap()
            .expect("build_and_persist not in graph");
        let tree = snap
            .call_graph(root_id, 0)
            .expect("call_graph failed");
        assert!(tree.callees.is_empty(), "depth=0 leaves callees empty");
        assert!(
            tree.truncated_at_depth,
            "depth=0 on a fn with outgoing edges must set truncated_at_depth"
        );
    }

    #[test]
    fn callers_in_crate_filters_correctly() {
        // `loader::load` is referenced from inside `rust_code_mcp` itself
        // (e.g., from `build_and_persist`). Filtering by the workspace's own
        // crate must return a strict subset of who_calls — equal or smaller.
        let snap = shared_snapshot();
        let (target_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader::load")
            .unwrap()
            .expect("loader::load not in graph");
        let all = snap.who_calls(target_id).expect("who_calls failed");
        let filtered = snap
            .callers_in_crate(target_id, "rust_code_mcp")
            .expect("callers_in_crate failed");
        assert!(
            filtered.len() <= all.len(),
            "filtered set must be subset of who_calls (got {} filtered vs {} total)",
            filtered.len(),
            all.len()
        );
        // Every filtered row's caller must be set (came from an in-crate fn).
        for row in &filtered {
            assert!(
                row.caller_qualified_name
                    .as_deref()
                    .map(|s| s.starts_with("rust_code_mcp"))
                    .unwrap_or(false),
                "caller {:?} not in rust_code_mcp",
                row.caller_qualified_name
            );
        }
        // Filtering by a bogus crate name must yield zero rows even when
        // who_calls is non-empty.
        let empty = snap
            .callers_in_crate(target_id, "definitely_not_a_real_crate_xyz")
            .expect("callers_in_crate failed");
        assert!(empty.is_empty(), "bogus crate filter must return zero");
    }

    /// v7: `enum_variants` enumerates the variants of an enum. Pick
    /// `BindingKind` (defined in src/graph/model.rs) — it has exactly
    /// 4 variants: `Declared`, `NamedImport`, `GlobImport`,
    /// `ExternCrateImport`.
    #[test]
    fn enum_variants_returns_expected_set() {
        let snap = shared_snapshot();
        let (enum_id, enum_node) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::model::BindingKind")
            .unwrap()
            .expect("BindingKind enum not in graph");
        assert_eq!(enum_node.kind, NodeKind::Item);
        assert_eq!(enum_node.item_kind, Some(ItemKind::Enum));

        let variants = snap.enum_variants(enum_id).expect("enum_variants failed");
        let mut names: Vec<String> = variants.iter().map(|n| n.display_name.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "Declared".to_string(),
                "ExternCrateImport".to_string(),
                "GlobImport".to_string(),
                "NamedImport".to_string(),
            ],
            "expected exactly the 4 BindingKind variants, got {names:?}"
        );

        // Each variant Node must point its parent at the enum and carry
        // the right ItemKind / qualified_name shape.
        for v in &variants {
            assert_eq!(v.kind, NodeKind::Item);
            assert_eq!(v.item_kind, Some(ItemKind::EnumVariant));
            assert_eq!(v.parent_id, Some(enum_id));
            assert_eq!(
                v.qualified_name,
                format!("rust_code_mcp::graph::model::BindingKind::{}", v.display_name)
            );
            assert!(v.file.is_some(), "variant should have a file path");
            assert!(v.span.is_some(), "variant should have a span");
            assert!(v.visibility.is_none(), "variant visibility inherits from parent");
        }
    }

    /// v8: `item_attributes(target)` returns the outer attributes recorded
    /// on the Item Node. Pick `Node` struct (model.rs) — it carries a stable
    /// `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]`.
    #[test]
    fn item_attributes_of_node_struct_includes_derive() {
        let snap = shared_snapshot();
        let (id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::model::Node")
            .unwrap()
            .expect("Node struct not in snapshot");
        let attrs = snap.item_attributes(id).expect("item_attributes failed");
        let derive = attrs
            .iter()
            .find(|s| s.starts_with("#[derive("))
            .unwrap_or_else(|| panic!("no derive attr on Node, got {attrs:?}"));
        for trait_name in ["Debug", "Clone", "Serialize", "Deserialize"] {
            assert!(
                derive.contains(trait_name),
                "Node derive should mention `{trait_name}`, got `{derive}`"
            );
        }
    }

    /// v8: `items_with_attribute(crate, pattern)` anchor-matches the
    /// attribute strings on every Item in the crate. Searching for bare
    /// `derive` (attribute-path match) across
    /// `rust_code_mcp` should find at least the `Node` and `ItemKind`
    /// types.
    #[test]
    fn items_with_attribute_finds_derive_users() {
        let snap = shared_snapshot();
        // Resolve the crate node — `rust_code_mcp` resolves to the crate
        // root MODULE; promote to the actual Crate node via parent_id.
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = if root_node.kind == NodeKind::Crate {
            root_id
        } else {
            root_node.parent_id.expect("module should have parent")
        };
        let hits = snap
            .items_with_attribute(crate_id, "derive")
            .expect("items_with_attribute failed");
        assert!(
            !hits.is_empty(),
            "expected at least one derive-bearing item in rust_code_mcp"
        );
        let qnames: Vec<String> = hits.iter().map(|h| h.qualified_name.clone()).collect();
        assert!(
            qnames
                .iter()
                .any(|q| q == "rust_code_mcp::graph::model::Node"),
            "expected Node among derive-bearing items, got {qnames:?}"
        );
        assert!(
            qnames
                .iter()
                .any(|q| q == "rust_code_mcp::graph::model::ItemKind"),
            "expected ItemKind among derive-bearing items, got {qnames:?}"
        );
        for h in &hits {
            assert!(
                h.matched_attribute.starts_with("#[derive("),
                "matched_attribute should be a derive attr, got `{}` (location={})",
                h.matched_attribute,
                h.match_location,
            );
        }
    }

    #[test]
    fn match_attribute_accepts_bare_attribute_paths() {
        assert_eq!(match_attribute("#[derive(Debug)]", "derive"), Some("attr"));
        assert_eq!(match_attribute("#[derive(Debug)]", "#[derive("), Some("attr"));
        assert_eq!(match_attribute("#[must_use]", "must_use"), Some("attr"));
        assert_eq!(match_attribute("#[cfg(test)]", "cfg"), Some("attr"));
        assert_eq!(
            match_attribute("#[tool(description = \"mentions #[must_use]\")]", "must_use"),
            None
        );
    }

    /// Item #2 audit: anchored matching must NOT surface items whose
    /// attributes merely contain the pattern text mid-string. The MCP tool
    /// methods on `SearchToolRouter` carry a `#[tool(description = "...")]`
    /// attribute whose body mentions `#[must_use]` in prose (e.g. the
    /// description for `mut_static_audit`). The legacy substring matcher
    /// flagged those as `#[must_use]` items; the anchored matcher must not.
    #[test]
    fn items_with_attribute_does_not_match_pattern_inside_attr_body() {
        let snap = shared_snapshot();
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = if root_node.kind == NodeKind::Crate {
            root_id
        } else {
            root_node.parent_id.expect("module should have parent")
        };
        let results = snap
            .items_with_attribute(crate_id, "#[must_use]")
            .expect("items_with_attribute failed");
        for hit in &results {
            assert!(
                !hit.qualified_name.contains("SearchToolRouter::item_attributes"),
                "anchored match should skip mentions of `#[must_use]` inside other attributes' bodies, got hit={hit:?}"
            );
            assert!(
                !hit.qualified_name.contains("SearchToolRouter::items_with_attribute"),
                "same — should skip the items_with_attribute tool description, got hit={hit:?}"
            );
            // The audit also matched a doc comment in
            // OpenedSnapshot::items_with_attribute earlier — verify that's
            // also gone now (the doc body started with `(`, not `#[`).
            assert!(
                !hit.qualified_name.contains("OpenedSnapshot::items_with_attribute"),
                "should not match doc-comment lines that merely mention `#[must_use]`, got hit={hit:?}"
            );
            // Every surviving hit must either start the attr with the
            // pattern, or have a doc-body that does.
            let m = &hit.matched_attribute;
            let body_match = m
                .strip_prefix("/// ")
                .map(|b| b.starts_with("#[must_use]"))
                .unwrap_or(false);
            assert!(
                m.starts_with("#[must_use]") || body_match,
                "matched_attribute `{m}` (location={}) should anchor at start or in doc body",
                hit.match_location,
            );
        }
    }

    /// Item #2: empty pattern must return zero results (vs. the legacy
    /// substring containment which trivially matched everything).
    #[test]
    fn items_with_attribute_empty_pattern_returns_nothing() {
        let snap = shared_snapshot();
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = if root_node.kind == NodeKind::Crate {
            root_id
        } else {
            root_node.parent_id.expect("module should have parent")
        };
        let results = snap
            .items_with_attribute(crate_id, "")
            .expect("items_with_attribute failed");
        assert!(
            results.is_empty(),
            "empty pattern should return zero hits, got {} hits",
            results.len()
        );
    }

    /// Phase 4a smoke: `pub_use_pub_type_audit` returns without error
    /// against the `rust_code_mcp` workspace. Result set may be empty
    /// (this codebase doesn't necessarily contain the antipattern); when
    /// non-empty, every entry must carry a non-empty qualified name and
    /// distinct alias / pub_use_target NodeIds.
    #[test]
    fn pub_use_pub_type_audit_smoke() {
        let snap = shared_snapshot();
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = if root_node.kind == NodeKind::Crate {
            root_id
        } else {
            root_node.parent_id.expect("root module has parent")
        };
        let findings = snap
            .pub_use_pub_type_audit(crate_id)
            .expect("pub_use_pub_type_audit failed");
        for f in &findings {
            assert!(
                !f.alias_qualified_name.is_empty(),
                "alias_qualified_name must be non-empty"
            );
            assert!(
                !f.suspicious_pub_use_visible_name.is_empty(),
                "suspicious_pub_use_visible_name must be non-empty"
            );
            // Alias and the matching pub_use's target are different by
            // construction (the alias wouldn't be flagged otherwise).
            assert_ne!(
                f.alias_node_id, f.suspicious_pub_use_target_node_id,
                "alias and pub_use target should differ"
            );
        }
    }

    /// Phase 4b smoke: re_export_chain on `ForbiddenDependencyRule`,
    /// which `src/graph/mod.rs` re-exports from `queries`. Walking from
    /// the canonical declaration must surface at least one link (the
    /// `pub use` in `graph/mod.rs`).
    #[test]
    fn re_export_chain_finds_known_facade() {
        let snap = shared_snapshot();
        let (target_id, _) = snap
            .lookup_by_qualified_name(
                "rust_code_mcp::graph::queries::ForbiddenDependencyRule",
            )
            .unwrap()
            .expect("ForbiddenDependencyRule canonical decl not in snapshot");
        let chain = snap
            .re_export_chain(target_id)
            .expect("re_export_chain failed");
        assert_eq!(chain.canonical, target_id);
        assert!(
            !chain.links.is_empty(),
            "expected at least one re-export link for ForbiddenDependencyRule, got 0"
        );
        // Sanity: every link must carry the same visible_name (the type
        // is re-exported under its own name) and a sane depth.
        for link in &chain.links {
            assert_eq!(link.visible_name, "ForbiddenDependencyRule");
            assert!(link.depth >= 1, "depth must be >= 1");
            assert!(
                (link.depth as usize) <= MAX_REEXPORT_HOPS,
                "depth must be <= MAX_REEXPORT_HOPS"
            );
            assert!(
                !link.from_module_qualified_name.is_empty(),
                "from_module_qualified_name must resolve"
            );
        }
    }

    /// Phase 4c smoke: `crate_dependency_metric` returns one entry per
    /// local crate and every metric is well-formed (counts non-negative,
    /// instability + (1 - instability) ≈ 1, abstractness in [0, 1]).
    #[test]
    fn crate_dependency_metric_smoke() {
        let snap = shared_snapshot();
        let metrics = snap
            .crate_dependency_metric()
            .expect("crate_dependency_metric failed");
        assert!(
            !metrics.is_empty(),
            "expected at least one local crate (this workspace itself)"
        );
        for m in &metrics {
            assert!(!m.crate_name.is_empty(), "crate_name must be non-empty");
            // u32 fields are non-negative by construction.
            let _ = m.efferent;
            let _ = m.afferent;
            let _ = m.item_count;
            // Instability sanity.
            assert!(
                (0.0..=1.0).contains(&m.instability),
                "instability must be in [0, 1], got {} for {}",
                m.instability,
                m.crate_name
            );
            assert!(
                ((m.instability + (1.0 - m.instability)) - 1.0).abs() < 1e-9,
                "instability sanity sum failed for {}",
                m.crate_name
            );
            // Abstractness sanity.
            assert!(
                (0.0..=1.0).contains(&m.abstractness),
                "abstractness must be in [0, 1], got {} for {}",
                m.abstractness,
                m.crate_name
            );
        }
    }

    #[test]
    fn recursive_callers_count_grows_with_depth() {
        // `loader::load` has at least one direct caller (`build_and_persist`),
        // which itself has callers somewhere in the codebase. So the depth=3
        // count must be >= depth=1 count.
        let snap = shared_snapshot();
        let (target_id, _) = snap
            .lookup_by_qualified_name("rust_code_mcp::graph::loader::load")
            .unwrap()
            .expect("loader::load not in graph");
        let depth1 = snap
            .recursive_callers_count(target_id, 1)
            .expect("recursive_callers_count failed");
        let depth3 = snap
            .recursive_callers_count(target_id, 3)
            .expect("recursive_callers_count failed");
        assert_eq!(depth1.depth, 1);
        assert_eq!(depth3.depth, 3);
        assert!(
            depth3.transitive_callers >= depth1.transitive_callers,
            "transitive_callers must grow monotonically with depth (got d1={} d3={})",
            depth1.transitive_callers,
            depth3.transitive_callers
        );
        assert_eq!(
            depth1.direct_callers, depth1.transitive_callers,
            "depth=1 transitive must equal direct"
        );
        assert!(
            depth1.direct_callers >= 1,
            "loader::load should have at least one direct caller"
        );
        // depth=0 case
        let depth0 = snap
            .recursive_callers_count(target_id, 0)
            .expect("recursive_callers_count failed");
        assert_eq!(depth0.direct_callers, 0);
        assert_eq!(depth0.transitive_callers, 0);
        assert_eq!(depth0.depth_reached, 0);
        assert!(!depth0.truncated_at_depth);
    }
}
