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

use super::ids::{BindingId, NodeId};
use super::labels::{
    binding_kind_label as label_binding_kind, item_kind_short_label as label_item_kind,
    node_kind_label,
};
use super::model::{
    Binding, BindingKind, BindingVisibility, FunctionSignature, ItemKind, Node, NodeKind, SelfKind,
    Usage,
};
use super::snapshot::OpenedSnapshot;

// Result types moved to `super::query::model` in PR 08; re-exported here so
// `crate::graph::queries::FooResult` still resolves for external consumers.
pub use super::query::model::*;

// `classify_metadata` moved to `super::query::audits` in PR 10; re-exported
// here so `crate::graph::queries::classify_metadata` still resolves for
// external consumers (e.g. `src/graph/statics.rs` unit tests).
pub use super::query::audits::classify_metadata;

/// Maximum re-export facade hops to follow before giving up. Bounds recursion
/// in the (pathological) case of a binding chain or a self-referential cycle.
pub(crate) const MAX_REEXPORT_HOPS: usize = 8;

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

    /// v9: return the recorded `FunctionSignature` for `target` (a local
    /// function NodeId), or `None` if no signature is present (e.g. the
    /// target isn't a function, or extraction skipped it). Single-key LMDB
    /// lookup, no scan.
    pub fn function_signature(&self, target: NodeId) -> Result<Option<FunctionSignature>> {
        let rtxn = self.env.read_txn()?;
        Ok(self.dbs.signatures_by_target.get(&rtxn, target.as_bytes())?)
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

    pub(super) fn bindings_for_from_module<'txn>(
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

    pub(super) fn bindings_for_target<'txn>(
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

    pub(super) fn usages_for_target<'txn>(
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

    pub(super) fn usages_for_consumer<'txn>(
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

    pub(super) fn usages_for_consumer_function<'txn>(
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

pub(super) fn dependency_node_for(
    nodes: &HashMap<NodeId, Node>,
    target: NodeId,
) -> Option<(NodeId, &Node)> {
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
