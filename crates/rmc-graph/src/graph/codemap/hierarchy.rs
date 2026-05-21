//! Filtered module-tree projection used by `build_codemap`.
//!
//! Split out of `mod.rs` in PR 13. `project_hierarchy` discovers the crates
//! represented in the retained set, fetches each crate's full module tree
//! from the snapshot, then post-order filters every tree so only branches
//! containing at least one retained node survive. The recursive filter
//! lives in `filter_module_tree`.

use std::collections::HashSet;

use crate::graph::ids::NodeId;
use crate::graph::model::NodeKind;
use crate::graph::ModuleTreeNode;
use crate::graph::snapshot::OpenedSnapshot;

/// Project a hierarchy `ModuleTreeNode` over the retained set.
///
/// Strategy: discover the distinct crate qualified names represented by
/// retained nodes (via `Node.crate_id`), pull each crate's full module tree
/// via `OpenedSnapshot::module_tree`, then filter each tree post-order so
/// only branches containing at least one retained node survive.
///
/// If only one crate is represented, return its filtered tree directly. If
/// multiple, wrap the per-crate trees under a synthetic `Workspace` root.
pub(super) fn project_hierarchy(
    snap: &OpenedSnapshot,
    retained: &HashSet<NodeId>,
) -> anyhow::Result<ModuleTreeNode> {
    // Find distinct crate ids of retained nodes.
    let mut crate_ids: HashSet<NodeId> = HashSet::new();
    {
        let rtxn = snap.read_txn()?;
        for &nid in retained {
            if let Some(node) = snap.node(&rtxn, nid)? {
                if let Some(cid) = node.crate_id {
                    crate_ids.insert(cid);
                }
            }
        }
    }

    // Map each crate id to its qualified name, sorted for determinism.
    let mut crate_names: Vec<String> = Vec::new();
    {
        let rtxn = snap.read_txn()?;
        for cid in &crate_ids {
            if let Some(c) = snap.node(&rtxn, *cid)? {
                if c.kind == NodeKind::Crate {
                    crate_names.push(c.qualified_name);
                }
            }
        }
    }
    crate_names.sort();
    crate_names.dedup();

    // Retained qualified-name set for the filter predicate.
    let retained_qns: HashSet<String> = {
        let rtxn = snap.read_txn()?;
        retained
            .iter()
            .filter_map(|nid| snap.node(&rtxn, *nid).ok().flatten().map(|n| n.qualified_name))
            .collect()
    };

    let mut filtered_trees: Vec<ModuleTreeNode> = Vec::new();
    for name in &crate_names {
        let tree = snap.module_tree(name, None)?;
        if let Some(filtered) = filter_module_tree(tree, &retained_qns) {
            filtered_trees.push(filtered);
        }
    }

    if filtered_trees.len() == 1 {
        // Safe: len == 1.
        Ok(filtered_trees.into_iter().next().expect("len == 1"))
    } else {
        // Wrap in a synthetic Workspace root. ModuleTreeNode fields are
        // string-typed so we mint sensible labels rather than touching the
        // queries.rs struct.
        Ok(ModuleTreeNode {
            qualified_name: "<workspace>".to_string(),
            display_name: "workspace".to_string(),
            kind: "Workspace".to_string(),
            item_kind: None,
            visibility: None,
            children: filtered_trees,
        })
    }
}

/// Post-order filter on a `ModuleTreeNode`. Keeps a node iff its
/// `qualified_name` is in `retained_qns` OR any descendant is kept.
fn filter_module_tree(
    mut node: ModuleTreeNode,
    retained_qns: &HashSet<String>,
) -> Option<ModuleTreeNode> {
    let kept_children: Vec<ModuleTreeNode> = std::mem::take(&mut node.children)
        .into_iter()
        .filter_map(|c| filter_module_tree(c, retained_qns))
        .collect();
    let self_retained = retained_qns.contains(&node.qualified_name);
    if self_retained || !kept_children.is_empty() {
        node.children = kept_children;
        Some(node)
    } else {
        None
    }
}
