//! Phase 8 — `derive_audit`.
//!
//! Pure read-side query: enumerate `pub` Items (Struct / Enum / Union) whose
//! `node.attributes` are missing one or more required derive macros (e.g.
//! every public Struct without `Debug` per §8 "Debug almost always"). The
//! visibility filter mirrors `docs_audit`: only pure `pub` Items count;
//! `pub(crate)` and friends are internal API and skipped.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::ids::NodeId;
use super::model::{BindingKind, BindingVisibility, ItemKind, Node, NodeKind};
use super::snapshot::OpenedSnapshot;

#[derive(Debug, Clone)]
pub struct DeriveAuditOpts {
    pub crate_id_filter: Option<NodeId>,
    pub kind_filter: HashSet<ItemKind>,
    pub required_derives: HashSet<String>,
    pub pub_only: bool,
    pub skip_test_items: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DeriveFinding {
    pub target: NodeId,
    pub qualified_name: String,
    pub item_kind: ItemKind,
    pub visibility: String,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    pub current_derives: Vec<String>,
    pub missing_derives: Vec<String>,
}

pub(crate) fn derive_audit(snap: &OpenedSnapshot, opts: DeriveAuditOpts) -> Result<Vec<DeriveFinding>> {
    let rtxn = snap.env.read_txn()?;

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

    let mut out: Vec<DeriveFinding> = Vec::new();
    for (id, mut node) in candidates {
        let vis_str = match item_vis.get(&id).map(|(v, _)| *v) {
            Some(BindingVisibility::Public) => "pub".to_string(),
            Some(BindingVisibility::Private)
            | Some(BindingVisibility::Crate(_))
            | Some(BindingVisibility::RestrictedTo(_)) => {
                if opts.pub_only {
                    continue;
                }
                "non-pub".to_string()
            }
            None => {
                if opts.pub_only {
                    continue;
                }
                "non-pub".to_string()
            }
        };
        node.visibility = Some(if vis_str == "pub" { "pub".to_string() } else { vis_str.clone() });

        let Some((current, missing)) = missing_required_derives(
            &node,
            &opts.kind_filter,
            opts.pub_only,
            opts.skip_test_items,
            &opts.required_derives,
        ) else {
            continue;
        };
        let Some(kind) = node.item_kind else {
            continue;
        };
        let mut current_sorted: Vec<String> = current.into_iter().collect();
        current_sorted.sort();
        let mut missing_sorted: Vec<String> = missing.into_iter().collect();
        missing_sorted.sort();
        out.push(DeriveFinding {
            target: id,
            qualified_name: node.qualified_name,
            item_kind: kind,
            visibility: vis_str,
            file: node.file,
            span: node.span,
            current_derives: current_sorted,
            missing_derives: missing_sorted,
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

/// Parse all `#[derive(...)]` attribute strings from a Node's attribute list
/// and return the set of derive identifiers (stripped of path qualifiers).
///
/// Handles:
///   - `#[derive(Debug)]`            -> ["Debug"]
///   - `#[derive(Debug, Clone, PartialEq)]` -> ["Debug", "Clone", "PartialEq"]
///   - `#[derive(serde::Serialize)]` -> ["Serialize"]  (leading path stripped)
///   - `#[derive(::std::fmt::Debug)]` -> ["Debug"]     (absolute path stripped)
///   - whitespace and trailing commas tolerated
///
/// Multiple `#[derive(...)]` attributes on one item accumulate.
pub(crate) fn extract_derives(attributes: &[String]) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    for raw in attributes {
        let trimmed = raw.trim();
        let Some(rest) = trimmed.strip_prefix("#[derive(") else {
            continue;
        };
        // Find the matching closing `)`; the attribute text ends with `)]`.
        let Some(end) = rest.rfind(")]") else {
            continue;
        };
        let inner = &rest[..end];
        // Drop a trailing `)` if `rfind(")]")` left one (it does for nested
        // close-parens like `#[derive(Foo)]` — `rest = "Foo)"`, `end = 3`).
        let inner = inner.trim_end_matches(')').trim();
        for piece in inner.split(',') {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let last = match piece.rsplit_once("::") {
                Some((_, tail)) => tail,
                None => piece,
            };
            let last = last.trim();
            if !last.is_empty() {
                out.insert(last.to_string());
            }
        }
    }
    out
}

/// Returns `Some((current_derives, missing_derives))` when `node` should be
/// flagged: it's the right kind, right visibility, not in a test module, and
/// missing at least one required derive. Returns `None` otherwise.
///
/// Caller is responsible for populating `node.visibility` from the declaring
/// Binding before invocation (Item Nodes don't carry visibility themselves).
pub(crate) fn missing_required_derives(
    node: &Node,
    kind_filter: &HashSet<ItemKind>,
    pub_only: bool,
    skip_tests: bool,
    required: &HashSet<String>,
) -> Option<(HashSet<String>, HashSet<String>)> {
    if node.kind != NodeKind::Item {
        return None;
    }
    let kind = node.item_kind?;
    if !kind_filter.contains(&kind) {
        return None;
    }
    if pub_only && node.visibility.as_deref() != Some("pub") {
        return None;
    }
    if skip_tests && node.qualified_name.contains("::tests::") {
        return None;
    }
    let current = extract_derives(&node.attributes);
    let missing: HashSet<String> = required.difference(&current).cloned().collect();
    if missing.is_empty() {
        return None;
    }
    Some((current, missing))
}

/// Default kind set per Phase 8 plan: `Struct`, `Enum`, `Union` — the three
/// kinds that accept derive macros.
pub(crate) fn default_kind_filter() -> HashSet<ItemKind> {
    let mut s = HashSet::new();
    s.insert(ItemKind::Struct);
    s.insert(ItemKind::Enum);
    s.insert(ItemKind::Union);
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
            crate_target_kind: None,
        }
    }

    fn s(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_single_derive() {
        let got = extract_derives(&["#[derive(Debug)]".into()]);
        assert_eq!(got, s(&["Debug"]));
    }

    #[test]
    fn parses_multiple_derives() {
        let got = extract_derives(&["#[derive(Debug, Clone, PartialEq)]".into()]);
        assert_eq!(got, s(&["Debug", "Clone", "PartialEq"]));
    }

    #[test]
    fn strips_single_path_qualifier() {
        let got = extract_derives(&["#[derive(serde::Serialize)]".into()]);
        assert_eq!(got, s(&["Serialize"]));
    }

    #[test]
    fn strips_absolute_path_qualifier() {
        let got = extract_derives(&["#[derive(::std::fmt::Debug)]".into()]);
        assert_eq!(got, s(&["Debug"]));
    }

    #[test]
    fn tolerates_whitespace_and_trailing_commas() {
        let got = extract_derives(&["#[derive(  Debug ,  Clone  )]".into()]);
        assert_eq!(got, s(&["Debug", "Clone"]));
    }

    #[test]
    fn accumulates_multiple_derive_attrs() {
        let got = extract_derives(&[
            "#[derive(Debug)]".into(),
            "#[derive(Clone)]".into(),
        ]);
        assert_eq!(got, s(&["Debug", "Clone"]));
    }

    #[test]
    fn ignores_non_derive_attrs() {
        let got = extract_derives(&["#[must_use]".into()]);
        assert!(got.is_empty());
    }

    #[test]
    fn ignores_doc_comments() {
        let got = extract_derives(&["///doc".into()]);
        assert!(got.is_empty());
    }

    #[test]
    fn predicate_flags_pub_struct_missing_clone() {
        let node = make_node(
            ItemKind::Struct,
            Some("pub"),
            "krate::Foo",
            vec!["#[derive(Debug)]"],
        );
        let kinds = default_kind_filter();
        let required = s(&["Debug", "Clone"]);
        let result = missing_required_derives(&node, &kinds, true, true, &required);
        let (current, missing) = result.expect("should flag");
        assert_eq!(current, s(&["Debug"]));
        assert_eq!(missing, s(&["Clone"]));
    }

    #[test]
    fn predicate_skips_when_all_present() {
        let node = make_node(
            ItemKind::Struct,
            Some("pub"),
            "krate::Foo",
            vec!["#[derive(Debug, Clone)]"],
        );
        let kinds = default_kind_filter();
        let required = s(&["Debug", "Clone"]);
        assert!(missing_required_derives(&node, &kinds, true, true, &required).is_none());
    }

    #[test]
    fn predicate_skips_non_pub_when_pub_only() {
        let node = make_node(
            ItemKind::Struct,
            Some("pub(crate)"),
            "krate::Foo",
            vec![],
        );
        let kinds = default_kind_filter();
        let required = s(&["Debug"]);
        assert!(missing_required_derives(&node, &kinds, true, true, &required).is_none());
    }

    #[test]
    fn predicate_skips_test_module_item() {
        let node = make_node(
            ItemKind::Struct,
            Some("pub"),
            "krate::tests::Fixture",
            vec![],
        );
        let kinds = default_kind_filter();
        let required = s(&["Debug"]);
        assert!(missing_required_derives(&node, &kinds, true, true, &required).is_none());
    }

    #[test]
    fn predicate_skips_kind_outside_filter() {
        let node = make_node(
            ItemKind::Function,
            Some("pub"),
            "krate::do_thing",
            vec![],
        );
        let kinds = default_kind_filter();
        let required = s(&["Debug"]);
        assert!(missing_required_derives(&node, &kinds, true, true, &required).is_none());
    }
}
