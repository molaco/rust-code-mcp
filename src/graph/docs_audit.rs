//! Phase 8 — `missing_docs_audit`.
//!
//! Pure read-side query: enumerate `pub` Items whose `node.attributes` carry
//! no `///` doc-comment line. Visibility lives on the declaring `Binding`
//! (not on the Item Node), so we pre-build a target → BindingVisibility map
//! by walking `bindings_by_id` once, mirroring the pattern in
//! `queries::module_tree_for_crate`.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::ids::NodeId;
use super::model::{BindingKind, BindingVisibility, ItemKind, Node, NodeKind};
use super::snapshot::OpenedSnapshot;

#[derive(Debug, Clone)]
pub struct AuditOpts {
    pub crate_id_filter: Option<NodeId>,
    pub kind_filter: HashSet<ItemKind>,
    pub skip_test_items: bool,
}

#[derive(Debug, Clone)]
pub struct MissingDocsFinding {
    pub target: NodeId,
    pub qualified_name: String,
    pub item_kind: ItemKind,
    pub visibility: String,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
}

pub fn missing_docs_audit(
    snap: &OpenedSnapshot,
    opts: AuditOpts,
) -> Result<Vec<MissingDocsFinding>> {
    let rtxn = snap.env.read_txn()?;

    // Pass 1: collect candidate Items (NodeKind::Item, optional crate filter,
    // kind filter, optional ::tests:: filter). Drop the iterator borrow before
    // walking bindings_by_id.
    let mut candidates: Vec<(NodeId, Node)> = Vec::new();
    for entry in snap.dbs.nodes_by_id.iter(&rtxn)? {
        let (key, node) = entry?;
        if node.kind != NodeKind::Item {
            continue;
        }
        if let Some(filter_id) = opts.crate_id_filter {
            if node.crate_id != Some(filter_id) {
                continue;
            }
        }
        let Some(kind) = node.item_kind else {
            continue;
        };
        if !opts.kind_filter.contains(&kind) {
            continue;
        }
        if opts.skip_test_items && node.qualified_name.contains("::tests::") {
            continue;
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(key);
        candidates.push((NodeId(id), node));
    }

    // Pass 2: pre-build a target → BindingVisibility map for the candidate
    // set by walking bindings_by_id once and keeping Declared rows.
    let candidate_ids: HashSet<NodeId> = candidates.iter().map(|(id, _)| *id).collect();
    let mut item_parents: HashMap<NodeId, NodeId> = HashMap::new();
    for (id, node) in &candidates {
        if let Some(parent) = node.parent_id {
            item_parents.insert(*id, parent);
        }
    }
    let mut item_vis: HashMap<NodeId, (BindingVisibility, bool)> = HashMap::new();
    for entry in snap.dbs.bindings_by_id.iter(&rtxn)? {
        let (_k, binding) = entry?;
        if binding.kind != BindingKind::Declared {
            continue;
        }
        if !candidate_ids.contains(&binding.target) {
            continue;
        }
        let parent_match = item_parents
            .get(&binding.target)
            .map(|p| *p == binding.from_module)
            .unwrap_or(false);
        match item_vis.get(&binding.target) {
            None => {
                item_vis.insert(binding.target, (binding.visibility, parent_match));
            }
            Some((_, existing_parent_match)) => {
                if !existing_parent_match && parent_match {
                    item_vis.insert(binding.target, (binding.visibility, parent_match));
                }
            }
        }
    }

    // Pass 3: predicate + finding emission.
    let mut out: Vec<MissingDocsFinding> = Vec::new();
    for (id, mut node) in candidates {
        let vis = match item_vis.get(&id).map(|(v, _)| *v) {
            Some(v) => v,
            None => continue,
        };
        // Only pure-`pub` (BindingVisibility::Public) is in scope; pub(crate)
        // and friends are internal API per §10 and skipped.
        let vis_str = match vis {
            BindingVisibility::Public => "pub".to_string(),
            BindingVisibility::Private => continue,
            BindingVisibility::Crate(_) => continue,
            BindingVisibility::RestrictedTo(_) => continue,
        };
        node.visibility = Some(vis_str.clone());

        if !is_undocumented_pub_item(&node, &opts.kind_filter, opts.skip_test_items) {
            continue;
        }
        let Some(kind) = node.item_kind else {
            continue;
        };
        out.push(MissingDocsFinding {
            target: id,
            qualified_name: node.qualified_name,
            item_kind: kind,
            visibility: vis_str,
            file: node.file,
            span: node.span,
        });
    }

    out.sort_by(|a, b| {
        a.file
            .as_deref()
            .unwrap_or("")
            .cmp(b.file.as_deref().unwrap_or(""))
            .then_with(|| {
                a.span
                    .map(|s| s.0)
                    .unwrap_or(0)
                    .cmp(&b.span.map(|s| s.0).unwrap_or(0))
            })
    });
    Ok(out)
}

/// Predicate: `true` if `node` is a `pub` Item of an in-filter kind that
/// carries no `///` line in `attributes`. Caller is responsible for
/// populating `node.visibility` from the declaring Binding (the Node itself
/// stores `None` for Item visibility).
pub fn is_undocumented_pub_item(
    node: &Node,
    kind_filter: &HashSet<ItemKind>,
    skip_tests: bool,
) -> bool {
    if node.kind != NodeKind::Item {
        return false;
    }
    let Some(kind) = node.item_kind else {
        return false;
    };
    if !kind_filter.contains(&kind) {
        return false;
    }
    if node.visibility.as_deref() != Some("pub") {
        return false;
    }
    if skip_tests && node.qualified_name.contains("::tests::") {
        return false;
    }
    !node.attributes.iter().any(|a| a.starts_with("///"))
}

/// Default kind set per Phase 8 plan: documentable kinds excluding
/// EnumVariant, AssocConst, AssocType (rarely carry standalone docs).
pub fn default_kind_filter() -> HashSet<ItemKind> {
    let mut s = HashSet::new();
    s.insert(ItemKind::Function);
    s.insert(ItemKind::Struct);
    s.insert(ItemKind::Enum);
    s.insert(ItemKind::Union);
    s.insert(ItemKind::Trait);
    s.insert(ItemKind::TypeAlias);
    s.insert(ItemKind::Const);
    s.insert(ItemKind::Static);
    s.insert(ItemKind::Method);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::ids::NodeId;
    use crate::graph::model::{Node, NodeKind};

    fn make_node(
        kind: ItemKind,
        visibility: Option<&str>,
        qualified_name: &str,
        attributes: Vec<&str>,
    ) -> Node {
        Node {
            id: NodeId([0u8; 32]),
            kind: NodeKind::Item,
            display_name: qualified_name
                .rsplit("::")
                .next()
                .unwrap_or(qualified_name)
                .to_string(),
            qualified_name: qualified_name.to_string(),
            crate_id: None,
            parent_id: None,
            item_kind: Some(kind),
            file: None,
            span: None,
            visibility: visibility.map(|s| s.to_string()),
            attributes: attributes.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn detects_missing_doc_on_pub_item() {
        let node = make_node(ItemKind::Function, Some("pub"), "krate::do_thing", vec![]);
        let kinds = default_kind_filter();
        assert!(is_undocumented_pub_item(&node, &kinds, true));
    }

    #[test]
    fn skips_pub_item_with_doc_comment() {
        let node = make_node(
            ItemKind::Function,
            Some("pub"),
            "krate::do_thing",
            vec!["/// Top doc.", "/// Continued."],
        );
        let kinds = default_kind_filter();
        assert!(!is_undocumented_pub_item(&node, &kinds, true));
    }

    #[test]
    fn skips_non_pub_item() {
        let node = make_node(
            ItemKind::Function,
            Some("pub(crate)"),
            "krate::do_thing",
            vec![],
        );
        let kinds = default_kind_filter();
        assert!(!is_undocumented_pub_item(&node, &kinds, true));
    }

    #[test]
    fn skips_test_item_when_skip_flag_set() {
        let node = make_node(
            ItemKind::Function,
            Some("pub"),
            "krate::tests::helper",
            vec![],
        );
        let kinds = default_kind_filter();
        assert!(!is_undocumented_pub_item(&node, &kinds, true));
        // With skip_tests=false the same node IS flagged.
        assert!(is_undocumented_pub_item(&node, &kinds, false));
    }
}
