//! Query methods on `OpenedSnapshot` — modules family.
//!
//! Covers module-tree and workspace-level structural queries:
//! `module_tree`, `workspace_stats`. Moved here from `graph::queries`
//! in PR 11.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use heed::RoTxn;

use super::super::ids::NodeId;
use super::super::labels::{
    binding_kind_label as label_binding_kind, item_kind_short_label as label_item_kind,
    node_kind_label,
};
use super::super::model::{Binding, BindingKind, BindingVisibility, ItemKind, Node, NodeKind};
use super::super::snapshot::OpenedSnapshot;
use super::model::{
    CrateTypeItem, ModuleTreeNode, NodeKindCounts, VisibilityCounts, WorkspaceStats,
};

impl OpenedSnapshot {
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

        let mut crate_items: HashSet<NodeId> = HashSet::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Item && node.crate_id == Some(crate_id) {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                crate_items.insert(NodeId(id));
            }
        }
        let item_visibility = self.declared_item_visibility_map(&rtxn, &crate_items)?;

        self.build_module_tree(&rtxn, crate_id, depth, 0, &item_visibility)
    }

    pub fn crate_types(
        &self,
        crate_id: NodeId,
        kind_filter: &HashSet<ItemKind>,
        pub_only: bool,
        skip_test_items: bool,
    ) -> Result<Vec<CrateTypeItem>> {
        let rtxn = self.env.read_txn()?;
        let mut candidates: Vec<(NodeId, Node)> = Vec::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind != NodeKind::Item || node.crate_id != Some(crate_id) {
                continue;
            }
            let Some(kind) = node.item_kind else {
                continue;
            };
            if !kind_filter.contains(&kind) {
                continue;
            }
            if skip_test_items && node.qualified_name.contains("::tests::") {
                continue;
            }
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            candidates.push((NodeId(id), node));
        }

        let item_ids: HashSet<NodeId> = candidates.iter().map(|(id, _)| *id).collect();
        let item_visibility = self.declared_item_visibility_map(&rtxn, &item_ids)?;

        let mut out = Vec::with_capacity(candidates.len());
        for (id, node) in candidates {
            let visibility = item_visibility.get(&id).cloned();
            if pub_only && visibility.as_deref() != Some("pub") {
                continue;
            }
            let Some(kind) = node.item_kind else {
                continue;
            };
            out.push(CrateTypeItem {
                target: id,
                qualified_name: node.qualified_name,
                display_name: node.display_name,
                item_kind: kind,
                visibility,
                file: node.file,
                span: node.span,
            });
        }
        out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(out)
    }

    fn declared_item_visibility_map(
        &self,
        rtxn: &RoTxn<'_, heed::WithoutTls>,
        item_ids: &HashSet<NodeId>,
    ) -> Result<HashMap<NodeId, String>> {
        let mut item_parents: HashMap<NodeId, NodeId> = HashMap::new();
        for item_id in item_ids {
            if let Some(node) = self.dbs.nodes_by_id.get(rtxn, item_id.as_bytes())? {
                if let Some(parent) = node.parent_id {
                    item_parents.insert(*item_id, parent);
                }
            }
        }
        let mut item_vis_picks: HashMap<NodeId, (BindingVisibility, bool)> = HashMap::new();
        for entry in self.dbs.bindings_by_id.iter(rtxn)? {
            let (_k, binding) = entry?;
            if binding.kind != BindingKind::Declared {
                continue;
            }
            if !item_ids.contains(&binding.target) {
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
        let mut item_visibility: HashMap<NodeId, String> = HashMap::new();
        for (id, (vis, _)) in item_vis_picks {
            item_visibility.insert(id, format_binding_visibility(rtxn, self, vis));
        }
        Ok(item_visibility)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::Namespace;

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
}
