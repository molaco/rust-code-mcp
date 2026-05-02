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

use std::collections::HashSet;

use anyhow::{Context, Result};
use heed::RoTxn;
use serde::{Deserialize, Serialize};

use super::ids::{BindingId, NodeId};
use super::model::{Binding, BindingKind, BindingVisibility, ItemKind, Node, NodeKind, Usage};
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
        Ok(out)
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
}
