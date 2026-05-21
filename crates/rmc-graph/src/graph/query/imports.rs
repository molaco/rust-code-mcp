//! Query methods on `OpenedSnapshot` — imports family.
//!
//! Covers import/export/re-export queries: `imports_of`,
//! `module_dependencies`, `exports_of`, `reexports_of`,
//! `declared_reexports_of`. Moved here from `graph::queries` in PR 09.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use heed::RoTxn;

use super::super::ids::NodeId;
use super::super::labels::{
    binding_kind_label as label_binding_kind, item_kind_short_label as label_item_kind,
    node_kind_label,
};
use super::super::model::{Binding, BindingKind, BindingVisibility, Node, NodeKind};
use super::shared::dependency_node_for;
use super::super::snapshot::OpenedSnapshot;
use super::model::{ModuleDependency, ModuleDependencySymbol};

impl OpenedSnapshot {
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
