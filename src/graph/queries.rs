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

impl OpenedSnapshot {
    pub fn lookup_by_qualified_name(&self, name: &str) -> Result<Option<(NodeId, Node)>> {
        let rtxn = self.env.read_txn()?;
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.qualified_name == name {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                return Ok(Some((NodeId(id), node)));
            }
        }
        Ok(None)
    }

    pub fn node_by_id(&self, rtxn: &RoTxn<'_, heed::WithoutTls>, id: NodeId) -> Result<Option<Node>> {
        Ok(self.dbs.nodes_by_id.get(rtxn, id.as_bytes())?)
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
    use crate::graph::loader::load;
    use crate::graph::snapshot::{BuildOptions, build_and_persist, open_current};
    use crate::graph::storage::{GraphEnvOptions, GraphPaths};
    use std::path::Path;

    fn open_self_snapshot() -> (OpenedSnapshot, tempfile::TempDir) {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let tempdir = tempfile::tempdir().unwrap();
        let opts = BuildOptions {
            data_dir_override: Some(tempdir.path().to_path_buf()),
            ..Default::default()
        };
        let result = build_and_persist(Path::new(manifest_dir), opts).unwrap();
        let paths = GraphPaths::for_workspace_in(tempdir.path(), &result.workspace_root);
        let opened = open_current(&paths, GraphEnvOptions::default())
            .unwrap()
            .unwrap();
        (opened, tempdir)
    }

    #[test]
    fn lookup_by_qualified_name_resolves_known_modules() {
        let (snap, _td) = open_self_snapshot();
        let (_id, node) = snap
            .lookup_by_qualified_name("file_search_mcp::graph::loader")
            .unwrap()
            .expect("graph::loader module found");
        assert_eq!(node.kind, NodeKind::Module);
    }

    #[test]
    fn imports_of_graph_mod_includes_loader_load() {
        let (snap, _td) = open_self_snapshot();
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
        let (snap, _td) = open_self_snapshot();
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
        let (snap, _td) = open_self_snapshot();
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
    fn private_visibility_blocks_export() {
        // file_search_mcp::graph::extract has private helpers like `crate_display_name`.
        // From outside the loader/extract sibling (e.g., file_search_mcp root module),
        // those should NOT be exported.
        let (snap, _td) = open_self_snapshot();
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
