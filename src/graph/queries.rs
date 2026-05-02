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

use super::ids::{BindingId, NodeId};
use super::model::{Binding, BindingKind, BindingVisibility, Node, NodeKind};
use super::snapshot::OpenedSnapshot;

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
}
