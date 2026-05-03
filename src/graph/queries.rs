//! Layer 6 — read-path queries on a published snapshot.
//!
//! Four primitives, all expressed as direct LMDB lookups (no traversal):
//!   * `imports_of(M)` — scope-side: bindings declared in M that came from a `use`/extern crate.
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
use super::model::{Binding, BindingKind, BindingVisibility, ItemKind, Node, NodeKind, Usage, UsageCategory};
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

/// Result of `overlaps`: name collisions, module shadows, and within-crate
/// duplicates that often signal accidental complexity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlapsReport {
    pub cross_crate_type_collisions: Vec<TypeCollision>,
    pub module_shadows: Vec<ModuleShadow>,
    pub within_crate_type_duplicates: Vec<WithinCrateDuplicate>,
    pub common_fn_names: Vec<CommonFnName>,
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
    pub pub_self: usize,
    pub restricted_to: usize,
    pub private: usize,
}

/// Maximum re-export facade hops to follow before giving up. Bounds recursion
/// in the (pathological) case of a binding chain or a self-referential cycle.
const MAX_REEXPORT_HOPS: usize = 8;

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

    pub fn node_by_id(&self, rtxn: &RoTxn<'_, heed::WithoutTls>, id: NodeId) -> Result<Option<Node>> {
        Ok(self.dbs.nodes_by_id.get(rtxn, id.as_bytes())?)
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
    /// (Read/Write/Test/Other → count). Same Layer 4 caveat as `usages_of`:
    /// cross-crate **method calls** and **trait method dispatch** are NOT
    /// included — Layer 4 doesn't extract impl-block items as Item nodes.
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
    /// Note: cross-crate **method calls** and **trait method dispatch** are
    /// NOT captured in `total_refs_via_usages` — Layer 4 doesn't extract
    /// impl-block items as Item nodes, so usage counts only reflect
    /// references to module-level items.
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
            node_kind_label_map.insert(nid, label_node_kind(&node));
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

    /// Single-pass over `nodes_by_id`. Detects cross-crate type collisions,
    /// module shadowing of crate names, within-crate type duplicates, and
    /// fn names that appear in 4+ crates.
    pub fn overlaps(&self) -> Result<OverlapsReport> {
        let rtxn = self.env.read_txn()?;

        let mut crate_name_for: HashMap<NodeId, String> = HashMap::new();
        let mut crate_names: HashSet<String> = HashSet::new();

        // First pass: build crate-id → display_name index.
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Crate {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                crate_name_for.insert(NodeId(id), node.display_name.clone());
                crate_names.insert(node.display_name.clone());
            }
        }

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
            kind: label_node_kind(&node),
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
                match binding.visibility {
                    BindingVisibility::Public => visibility.pub_ += 1,
                    BindingVisibility::Crate(_) => visibility.pub_crate += 1,
                    BindingVisibility::RestrictedTo(_) => visibility.restricted_to += 1,
                    BindingVisibility::Private => visibility.private += 1,
                }
            }
        }
        // pub_self = items declared without any pub keyword. The Binding model
        // collapses that into Private. Mirror it explicitly for consumers that
        // expect the name to be present.
        visibility.pub_self = visibility.private;

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

fn label_node_kind(node: &Node) -> String {
    match node.kind {
        NodeKind::Workspace => "Workspace".to_string(),
        NodeKind::Crate => "Crate".to_string(),
        NodeKind::Module => "Module".to_string(),
        NodeKind::Item => match node.item_kind {
            Some(k) => format!("Item.{}", label_item_kind(k)),
            None => "Item".to_string(),
        },
        NodeKind::ExternalSymbol => "ExternalSymbol".to_string(),
    }
}

fn label_item_kind(k: ItemKind) -> &'static str {
    match k {
        ItemKind::Function => "Fn",
        ItemKind::Struct => "Struct",
        ItemKind::Enum => "Enum",
        ItemKind::Union => "Union",
        ItemKind::Trait => "Trait",
        ItemKind::TypeAlias => "TypeAlias",
        ItemKind::Const => "Const",
        ItemKind::Static => "Static",
        ItemKind::AssocFunction => "AssocFn",
        ItemKind::AssocConst => "AssocConst",
        ItemKind::AssocType => "AssocType",
        ItemKind::Method => "Method",
    }
}

fn label_binding_kind(k: BindingKind) -> &'static str {
    match k {
        BindingKind::Declared => "Declared",
        BindingKind::NamedImport => "NamedImport",
        BindingKind::GlobImport => "GlobImport",
        BindingKind::ExternCrateImport => "ExternCrateImport",
    }
}

fn usage_category_label(c: UsageCategory) -> &'static str {
    match c {
        UsageCategory::Read => "Read",
        UsageCategory::Write => "Write",
        UsageCategory::Test => "Test",
        UsageCategory::Other => "Other",
    }
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
mod tests {
    use super::*;
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

    fn shared_snapshot() -> &'static OpenedSnapshot {
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

    #[test]
    fn lookup_by_qualified_name_resolves_known_modules() {
        let snap = shared_snapshot();
        let (_id, node) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::loader")
            .unwrap()
            .expect("graph::loader module found");
        assert_eq!(node.kind, NodeKind::Module);
    }

    #[test]
    fn imports_of_graph_mod_includes_loader_load() {
        let snap = shared_snapshot();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph")
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
            .lookup_by_qualified_name("file_search_mcp::graph::loader::load")
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
            .lookup_by_qualified_name("file_search_mcp::graph")
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
            .lookup_by_qualified_name("file_search_mcp::graph::loader")
            .unwrap()
            .unwrap();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph")
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
        // `file_search_mcp::graph::load` is exposed via `pub use loader::load;`
        // in src/graph/mod.rs. The canonical declaration lives at
        // `file_search_mcp::graph::loader::load`. The fallback should follow the
        // re-export and return the canonical Item node.
        let snap = shared_snapshot();
        let (_id, node) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::load")
            .unwrap()
            .expect("re-export facade should resolve to the canonical Item");
        assert_eq!(node.kind, NodeKind::Item);
        assert_eq!(
            node.qualified_name, "file_search_mcp::graph::loader::load",
            "facade should resolve to the canonical declaration site"
        );
    }

    #[test]
    fn lookup_by_qualified_name_canonical_still_works() {
        // Regression check: the canonical-name path remains the primary lookup
        // and is not affected by the re-export fallback.
        let snap = shared_snapshot();
        let (_id, node) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::loader::load")
            .unwrap()
            .expect("canonical name should resolve directly");
        assert_eq!(node.kind, NodeKind::Item);
        assert_eq!(node.qualified_name, "file_search_mcp::graph::loader::load");
    }

    #[test]
    fn lookup_by_qualified_name_unresolvable_terminates() {
        // No node carries this name and no facade points at it. The recursive
        // fallback must terminate (bounded by MAX_REEXPORT_HOPS) and return None
        // rather than spinning.
        let snap = shared_snapshot();
        let result = snap
            .lookup_by_qualified_name("file_search_mcp::nonexistent::thing")
            .unwrap();
        assert!(
            result.is_none(),
            "lookup of an unknown name should return None, got {result:?}"
        );
    }

    #[test]
    fn private_visibility_blocks_export() {
        // file_search_mcp::graph::extract has private helpers like `crate_display_name`.
        // From outside the loader/extract sibling (e.g., file_search_mcp root module),
        // those should NOT be exported.
        let snap = shared_snapshot();
        let (extract_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::extract")
            .unwrap()
            .unwrap();
        let (root_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp")
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
            .lookup_by_qualified_name("file_search_mcp::graph::loader::load")
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
            .lookup_by_qualified_name("file_search_mcp::graph::snapshot")
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
            .lookup_by_qualified_name("file_search_mcp")
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
        // external→file_search_mcp edge. We only assert non-empty here.
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
    fn module_tree_roots_at_requested_crate() {
        let snap = shared_snapshot();
        let tree = snap.module_tree("file_search_mcp", None).unwrap();
        assert_eq!(tree.qualified_name, "file_search_mcp");
        assert_eq!(tree.kind, "Crate");
        assert!(
            !tree.children.is_empty(),
            "crate root should have at least one child (the root Module)"
        );
    }

    #[test]
    fn module_tree_respects_depth_limit() {
        let snap = shared_snapshot();
        let tree = snap.module_tree("file_search_mcp", Some(0)).unwrap();
        // Depth 0 => no children walked.
        assert!(tree.children.is_empty(), "depth=0 must not recurse");
    }

    #[test]
    fn declared_reexports_of_lists_all_pub_uses() {
        // `file_search_mcp::graph` has `pub use loader::load;` (and other
        // `pub use`s). declared_reexports_of(graph_mod_id) must include `load`
        // and every binding in the result must satisfy is_explicit_pub_use.
        let snap = shared_snapshot();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph")
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
        // `file_search_mcp::graph::mod` carries `pub use loader::load;`. The
        // resulting binding must have `is_explicit_pub_use == true`. The
        // declared binding for `loader` (sibling module declaration with no
        // `pub use`) must have `is_explicit_pub_use == false`.
        let snap = shared_snapshot();
        let (graph_mod_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph")
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
        // `file_search_mcp::graph::queries` has plenty of private `use` lines.
        let (queries_mod_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::queries")
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
            .lookup_by_qualified_name("file_search_mcp::graph::loader::load")
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
            .lookup_by_qualified_name("file_search_mcp::graph::snapshot::build_and_persist")
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
                Some("file_search_mcp::graph::snapshot::build_and_persist"),
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
    fn call_graph_returns_root_with_callees() {
        // `build_and_persist` is a known caller of `loader::load` (and others);
        // a depth-2 descent must produce a non-empty `callees` vec on the root.
        let snap = shared_snapshot();
        let (root_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::snapshot::build_and_persist")
            .unwrap()
            .expect("build_and_persist not in graph");
        let tree = snap
            .call_graph(root_id, 2)
            .expect("call_graph failed");
        assert_eq!(
            tree.fn_qualified_name,
            "file_search_mcp::graph::snapshot::build_and_persist"
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
            .lookup_by_qualified_name("file_search_mcp::graph::snapshot::build_and_persist")
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
        // `loader::load` is referenced from inside `file_search_mcp` itself
        // (e.g., from `build_and_persist`). Filtering by the workspace's own
        // crate must return a strict subset of who_calls — equal or smaller.
        let snap = shared_snapshot();
        let (target_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::loader::load")
            .unwrap()
            .expect("loader::load not in graph");
        let all = snap.who_calls(target_id).expect("who_calls failed");
        let filtered = snap
            .callers_in_crate(target_id, "file_search_mcp")
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
                    .map(|s| s.starts_with("file_search_mcp"))
                    .unwrap_or(false),
                "caller {:?} not in file_search_mcp",
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

    #[test]
    fn recursive_callers_count_grows_with_depth() {
        // `loader::load` has at least one direct caller (`build_and_persist`),
        // which itself has callers somewhere in the codebase. So the depth=3
        // count must be >= depth=1 count.
        let snap = shared_snapshot();
        let (target_id, _) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::loader::load")
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
