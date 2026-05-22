//! Integration tests for the `graph::query` read-path methods.
//!
//! Migrated from the deleted `graph::queries::tests` facade in PR 19.
//! The pre-deletion test module relied on `use super::*;` resolving through
//! the `queries.rs` facade (which re-exported `model::*`, `classify_metadata`,
//! and pulled in helpers via `#[cfg(test)]` use lines). We re-stage those
//! names explicitly here so the test bodies stay verbatim.

use std::collections::HashMap;

use super::super::ids::NodeId;
use super::super::model::{Binding, BindingKind, ItemKind, Node, NodeKind};
use super::model::*;
use super::navigation::{impl_module_item_alias_parts, is_impl_module_item_alias_candidate};
use super::shared::{MAX_REEXPORT_HOPS, dependency_node_for};

use crate::graph::model::BindingVisibility;
use crate::graph::test_support::shared_snapshot;

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
        "rmc_graph::graph::query::navigation::OpenedSnapshot::lookup_by_qualified_name",
    )
    .expect("alias parts");
    assert_eq!(module_prefix, "rmc_graph::graph::query::navigation");
    assert_eq!(type_name, "OpenedSnapshot");
    assert_eq!(member_name, "lookup_by_qualified_name");

    let node = test_node(
        "rmc_graph::graph::snapshot::OpenedSnapshot::lookup_by_qualified_name",
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
        "rmc_graph::graph::snapshot::OpenedSnapshot::lookup_by_qualified_name",
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
        "rmc_graph::graph::snapshot::OpenedSnapshot::lookup_by_qualified_name",
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
        .lookup_by_qualified_name("rmc_graph::graph::loader")
        .unwrap()
        .expect("graph::loader module found");
    assert_eq!(node.kind, NodeKind::Module);
}

#[test]
fn imports_of_graph_mod_includes_loader_load() {
    let snap = shared_snapshot();
    let (graph_mod_id, _) = snap
        .lookup_by_qualified_name("rmc_graph::graph")
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
        .lookup_by_qualified_name("rmc_graph::graph::loader::load")
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
        .lookup_by_qualified_name("rmc_graph::graph")
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
        .lookup_by_qualified_name("rmc_graph::graph::loader")
        .unwrap()
        .unwrap();
    let (graph_mod_id, _) = snap
        .lookup_by_qualified_name("rmc_graph::graph")
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
    // `rmc_graph::graph::load` is exposed via `pub use loader::load;`
    // in crates/rmc-graph/src/graph/mod.rs. The canonical declaration lives at
    // `rmc_graph::graph::loader::load`. The fallback should follow the
    // re-export and return the canonical Item node.
    let snap = shared_snapshot();
    let (_id, node) = snap
        .lookup_by_qualified_name("rmc_graph::graph::load")
        .unwrap()
        .expect("re-export facade should resolve to the canonical Item");
    assert_eq!(node.kind, NodeKind::Item);
    assert_eq!(
        node.qualified_name, "rmc_graph::graph::loader::load",
        "facade should resolve to the canonical declaration site"
    );
}

#[test]
fn lookup_by_qualified_name_canonical_still_works() {
    // Regression check: the canonical-name path remains the primary lookup
    // and is not affected by the re-export fallback.
    let snap = shared_snapshot();
    let (_id, node) = snap
        .lookup_by_qualified_name("rmc_graph::graph::loader::load")
        .unwrap()
        .expect("canonical name should resolve directly");
    assert_eq!(node.kind, NodeKind::Item);
    assert_eq!(node.qualified_name, "rmc_graph::graph::loader::load");
}

#[test]
fn lookup_by_qualified_name_unresolvable_terminates() {
    // No node carries this name and no facade points at it. The recursive
    // fallback must terminate (bounded by MAX_REEXPORT_HOPS) and return None
    // rather than spinning.
    let snap = shared_snapshot();
    let result = snap
        .lookup_by_qualified_name("rmc_graph::nonexistent::thing")
        .unwrap();
    assert!(
        result.is_none(),
        "lookup of an unknown name should return None, got {result:?}"
    );
}

#[test]
fn private_visibility_blocks_export() {
    // rmc_graph::graph::extract has private helpers like `crate_display_name`.
    // From outside the loader/extract sibling (e.g., rust_code_mcp root module),
    // those should NOT be exported.
    let snap = shared_snapshot();
    let (extract_id, _) = snap
        .lookup_by_qualified_name("rmc_graph::graph::extract")
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
        .lookup_by_qualified_name("rmc_graph::graph::loader::load")
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
        .lookup_by_qualified_name("rmc_graph::graph::snapshot")
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
        dependency_node_for(&nodes, variant_id).expect("variant dependency");
    assert_eq!(resolved_id, module_id);
    assert_eq!(resolved_node.qualified_name, "crate::search");
    let (resolved_id, resolved_node) =
        dependency_node_for(&nodes, external_id).expect("external dependency");
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
    // `rmc_graph::graph` has `pub use loader::load;` (and other
    // `pub use`s). declared_reexports_of(graph_mod_id) must include `load`
    // and every binding in the result must satisfy is_explicit_pub_use.
    let snap = shared_snapshot();
    let (graph_mod_id, _) = snap
        .lookup_by_qualified_name("rmc_graph::graph")
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
    // `rmc_graph::graph::mod` carries `pub use loader::load;`. The
    // resulting binding must have `is_explicit_pub_use == true`. The
    // declared binding for `loader` (sibling module declaration with no
    // `pub use`) must have `is_explicit_pub_use == false`.
    let snap = shared_snapshot();
    let (graph_mod_id, _) = snap
        .lookup_by_qualified_name("rmc_graph::graph")
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
    // Pick a module with private `use` lines: `rmc_graph::graph::loader`.
    let (loader_mod_id, _) = snap
        .lookup_by_qualified_name("rmc_graph::graph::loader")
        .unwrap()
        .unwrap();
    let loader_imports = snap.imports_of(loader_mod_id).unwrap();
    let private_imports: Vec<&Binding> = loader_imports
        .iter()
        .filter(|b| !b.is_explicit_pub_use)
        .collect();
    assert!(
        !private_imports.is_empty(),
        "expected at least one private (non-pub) `use` in graph::loader"
    );
}

#[test]
fn who_uses_summary_aggregates_by_consumer() {
    let snap = shared_snapshot();
    let (load_fn_id, _) = snap
        .lookup_by_qualified_name("rmc_graph::graph::loader::load")
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
        .lookup_by_qualified_name("rmc_graph::graph::snapshot::build_and_persist")
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
            Some("rmc_graph::graph::snapshot::build_and_persist"),
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
        .lookup_by_qualified_name("rmc_graph::graph::snapshot::build_and_persist")
        .unwrap()
        .expect("build_and_persist not in graph");
    let tree = snap
        .call_graph(root_id, 2)
        .expect("call_graph failed");
    assert_eq!(
        tree.fn_qualified_name,
        "rmc_graph::graph::snapshot::build_and_persist"
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
        .lookup_by_qualified_name("rmc_graph::graph::snapshot::build_and_persist")
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
        .lookup_by_qualified_name("rmc_graph::graph::loader::load")
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
        .lookup_by_qualified_name("rmc_graph::graph::model::BindingKind")
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
            format!("rmc_graph::graph::model::BindingKind::{}", v.display_name)
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
        .lookup_by_qualified_name("rmc_graph::graph::model::Node")
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
/// `rmc_graph` should find at least the `Node` and `ItemKind`
/// types (moved here from `rmc_graph::graph::model` in Phase 7 B.7).
#[test]
fn items_with_attribute_finds_derive_users() {
    let snap = shared_snapshot();
    // Resolve the crate node — `rmc_graph` resolves to the crate
    // root MODULE; promote to the actual Crate node via parent_id.
    let (root_id, root_node) = snap
        .lookup_by_qualified_name("rmc_graph")
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
        "expected at least one derive-bearing item in rmc_graph"
    );
    let qnames: Vec<String> = hits.iter().map(|h| h.qualified_name.clone()).collect();
    assert!(
        qnames
            .iter()
            .any(|q| q == "rmc_graph::graph::model::Node"),
        "expected Node among derive-bearing items, got {qnames:?}"
    );
    assert!(
        qnames
            .iter()
            .any(|q| q == "rmc_graph::graph::model::ItemKind"),
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
/// which `src/graph/mod.rs` re-exports from `query::model`. Walking from
/// the canonical declaration must surface at least one link (the
/// `pub use` in `graph/mod.rs`).
#[test]
fn re_export_chain_finds_known_facade() {
    let snap = shared_snapshot();
    let (target_id, _) = snap
        .lookup_by_qualified_name(
            "rmc_graph::graph::query::model::ForbiddenDependencyRule",
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
    // Sanity: at least one link must re-export the type under its own name
    // (the `pub use query::model::ForbiddenDependencyRule;` in graph::mod);
    // module-level re-export hops added by Phase 7 B.7's main `lib.rs` facade
    // (`pub use rmc_graph::graph;`) carry visible_name = "graph", which is also
    // a valid re-export hop for the type (transitive via module re-export).
    assert!(
        chain
            .links
            .iter()
            .any(|l| l.visible_name == "ForbiddenDependencyRule"),
        "expected at least one same-name re-export link"
    );
    for link in &chain.links {
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
        .lookup_by_qualified_name("rmc_graph::graph::loader::load")
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
