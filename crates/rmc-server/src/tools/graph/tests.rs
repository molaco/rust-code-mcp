//! Integration tests for the `tools::graph` endpoint families.
//!
//! Migrated from the deleted `tools::graph_tools::tests` facade in PR 19.
//! The pre-deletion test module relied on `use super::*;` resolving through
//! the facade's `pub use` of every family. We replicate that surface here by
//! bringing each family into scope explicitly.

use super::codemap::*;
use super::core::*;
use super::crates::*;
use super::response::*;
use super::surface::*;

use super::audits::graph_audit_error;

use rmc_graph::graph::{EnrichedUsage, GraphAuditError, ItemKind, Node, NodeId, NodeKind};
use crate::tools::params::{
    BuildHypergraphParams, DeadPubParams, DeadPubReportParams, GraphExportsParams,
    GraphImportsParams, ListPaginationParams, ModuleDependenciesParams, WhoImportsParams,
    WhoUsesParams,
};
use rmcp::model::{CallToolResult, ErrorCode};
use std::sync::Mutex;

// Both tests in this module open the same default data-dir snapshot
// (`~/.local/share/search/graphs/...`). heed forbids opening the same env
// twice in the same process, so we serialize them with a shared mutex
// rather than relying on `--test-threads=1`.
static DEFAULT_SNAPSHOT_LOCK: Mutex<()> = Mutex::new(());

/// Round-trip: build_hypergraph → get_imports / who_imports against this
/// crate. Uses the default data dir so the snapshot lifecycle exercised
/// here mirrors what an MCP client would see.
#[tokio::test]
async fn mcp_round_trip_against_self() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    let build = build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(true),
    })
    .await
    .expect("build_hypergraph");
    // Result is a single text Content with the JSON body.
    let body = first_text(&build);
    assert!(body.contains("\"node_count\""), "build response: {body}");
    assert!(body.contains("\"binding_count\""));

    let imports = get_imports(GraphImportsParams {
        directory: manifest_dir.to_string(),
        module: "rmc_graph::graph".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("get_imports");
    let body = first_text(&imports);
    assert!(
        body.contains("\"visible_name\": \"load\""),
        "expected `load` re-export in graph mod imports: {body}"
    );

    let dependencies = module_dependencies(ModuleDependenciesParams {
        directory: manifest_dir.to_string(),
        module: "rmc_indexing::indexing::tantivy_adapter".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("module_dependencies");
    let body = first_text(&dependencies);
    assert!(
        body.contains("\"target_module\": \"rmc_engine::search::bm25\""),
        "expected inline Bm25Search dependency on search::bm25: {body}"
    );
    assert!(
        body.contains("\"target_qualified\": \"rmc_engine::search::bm25::Bm25Search\""),
        "expected Bm25Search symbol in module dependency payload: {body}"
    );

    let importers = who_imports(WhoImportsParams {
        directory: manifest_dir.to_string(),
        target: "rmc_graph::graph::loader::load".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("who_imports");
    let body = first_text(&importers);
    assert!(
        body.contains("rmc_graph::graph"),
        "expected graph mod among importers of loader::load: {body}"
    );
}

/// Regression: passing a Crate qualified name (e.g. `rust_code_mcp`)
/// where a Module is expected (`get_exports`'s `consumer`) should be
/// transparent — the resolver should fall through to the crate's root
/// module rather than erroring with "is a Crate, expected Module".
#[tokio::test]
async fn get_exports_accepts_crate_name_as_consumer() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    // Ensure a snapshot exists for the workspace.
    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    let exports = get_exports(GraphExportsParams {
        directory: manifest_dir.to_string(),
        // `rmc_graph::graph` re-exports `load` (from loader),
        // visible from anywhere inside the crate.
        module: "rmc_graph::graph".to_string(),
        // Crate name, NOT a module path — must be transparently
        // promoted to the crate's root module.
        consumer: "rust_code_mcp".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("get_exports should accept a crate name as consumer");

    let body = first_text(&exports);
    assert!(
        body.contains("\"bindings\""),
        "expected a bindings array in response: {body}"
    );
    // The visible re-export of `load` from graph mod is one expected entry,
    // but the precise minimum is just "at least one binding".
    assert!(
        body.contains("\"visible_name\""),
        "expected at least one binding entry in response: {body}"
    );
}

/// MCP shape of who_uses + dead_pub_in_crate. Reuses the snapshot
/// produced by other tests in this module via the shared lock.
#[tokio::test]
async fn who_uses_and_dead_pub_round_trip() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    // who_uses against a fn we know is referenced inside the lib.
    let users = who_uses(WhoUsesParams {
        directory: manifest_dir.to_string(),
        target: "rmc_graph::graph::loader::load".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("who_uses");
    let body = first_text(&users);
    assert!(
        body.contains("\"usages\""),
        "expected a usages array in response: {body}"
    );
    assert!(
        body.contains("\"file\""),
        "expected at least one usage entry with file path: {body}"
    );

    // dead_pub_in_crate against this very crate. We don't pin a specific
    // qualified_name (the dead-pub set drifts with refactors); just smoke-
    // test that the tool returns a structured findings array.
    let dead = dead_pub_in_crate(DeadPubParams {
        directory: manifest_dir.to_string(),
        krate: "rust_code_mcp".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("dead_pub_in_crate");
    let body = first_text(&dead);
    assert!(
        body.contains("\"findings\""),
        "expected a findings array in response: {body}"
    );

    // dead_pub_report aggregates the same query across all local crates and
    // stamps a `total_findings` count. rust_code_mcp has at least one
    // local crate (itself), so `crates` is non-empty.
    let report = dead_pub_report(DeadPubReportParams {
        directory: manifest_dir.to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("dead_pub_report");
    let body = first_text(&report);
    assert!(
        body.contains("\"total_findings\""),
        "expected total_findings in response: {body}"
    );
    assert!(
        body.contains("\"crates\""),
        "expected crates array in response: {body}"
    );
}

/// Item #4: default `limit=50` caps the matches returned by the
/// wrapper, while `total_match_count` always reflects the unfiltered
/// (pre-slice) count. We use `is_async=true` as the permissive filter
/// (signatures.rs::tests confirms this returns >0 matches in the
/// workspace). The default-limit cap holds whether or not the
/// workspace currently has > 50 async fns: `match_count <= limit` and
/// `total_match_count >= match_count` regardless.
#[tokio::test]
async fn functions_with_filter_default_limit_caps_results() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    let result = functions_with_filter(crate::tools::params::FunctionsWithFilterParams {
        directory: manifest_dir.to_string(),
        krate: "rust_code_mcp".to_string(),
        min_param_count: None,
        has_param_type: None,
        returns_type_pattern: None,
        is_async: Some(true),
        self_kind: None,
        limit: None,
        offset: None,
        summary: None,
    })
    .await
    .expect("functions_with_filter");

    let body = first_text(&result);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
    let match_count = v
        .get("match_count")
        .and_then(|x| x.as_u64())
        .expect("match_count present") as usize;
    let limit = v
        .get("limit")
        .and_then(|x| x.as_u64())
        .expect("limit present") as usize;
    let total_match_count = v
        .get("total_match_count")
        .and_then(|x| x.as_u64())
        .expect("total_match_count present") as usize;
    let matches = v
        .get("matches")
        .and_then(|x| x.as_array())
        .expect("matches array present");

    assert_eq!(limit, 50, "default limit should be 50");
    assert!(
        match_count <= 50,
        "match_count must respect default limit 50, got {match_count}"
    );
    assert_eq!(
        matches.len(),
        match_count,
        "matches.len() must equal match_count"
    );
    assert!(
        total_match_count >= match_count,
        "total_match_count ({total_match_count}) must be >= match_count ({match_count})"
    );
}

/// Item #5: when `summary=true`, each match drops the full `signature`
/// payload. We rely on `#[serde(skip_serializing_if = "Option::is_none")]`
/// on the field, so the JSON object should not contain a `signature`
/// key at all in summary mode.
#[tokio::test]
async fn functions_with_filter_summary_mode_omits_signature() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    let result = functions_with_filter(crate::tools::params::FunctionsWithFilterParams {
        directory: manifest_dir.to_string(),
        krate: "rust_code_mcp".to_string(),
        min_param_count: None,
        has_param_type: None,
        returns_type_pattern: None,
        is_async: Some(true),
        self_kind: None,
        limit: Some(10),
        offset: None,
        summary: Some(true),
    })
    .await
    .expect("functions_with_filter");

    let body = first_text(&result);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
    let matches = v
        .get("matches")
        .and_then(|x| x.as_array())
        .expect("matches array present");
    assert!(
        !matches.is_empty(),
        "expected at least one match for is_async=true: {body}"
    );
    for (idx, m) in matches.iter().enumerate() {
        let obj = m.as_object().expect("match is an object");
        assert!(
            !obj.contains_key("signature"),
            "summary mode must omit `signature` key from match[{idx}]: {m}"
        );
        assert!(
            obj.contains_key("target"),
            "summary mode must keep `target` key: {m}"
        );
        assert!(
            obj.contains_key("qualified_name"),
            "summary mode must keep `qualified_name` key: {m}"
        );
    }
}

/// Item #4: `offset` skips matches; with `offset` >= `total_match_count`
/// no matches are returned, but `total_match_count` and `limit` still
/// echo the request inputs.
#[tokio::test]
async fn functions_with_filter_offset_pagination() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    // First page.
    let page1 = functions_with_filter(crate::tools::params::FunctionsWithFilterParams {
        directory: manifest_dir.to_string(),
        krate: "rust_code_mcp".to_string(),
        min_param_count: None,
        has_param_type: None,
        returns_type_pattern: None,
        is_async: Some(true),
        self_kind: None,
        limit: Some(5),
        offset: Some(0),
        summary: Some(true),
    })
    .await
    .expect("functions_with_filter");
    let body1 = first_text(&page1);
    let v1: serde_json::Value = serde_json::from_str(&body1).expect("page1 JSON");
    let total = v1
        .get("total_match_count")
        .and_then(|x| x.as_u64())
        .expect("total_match_count") as usize;
    let matches1: Vec<String> = v1
        .get("matches")
        .and_then(|x| x.as_array())
        .expect("matches array")
        .iter()
        .map(|m| m.get("qualified_name").and_then(|s| s.as_str()).unwrap_or("").to_string())
        .collect();

    // Second page (offset = 5).
    let page2 = functions_with_filter(crate::tools::params::FunctionsWithFilterParams {
        directory: manifest_dir.to_string(),
        krate: "rust_code_mcp".to_string(),
        min_param_count: None,
        has_param_type: None,
        returns_type_pattern: None,
        is_async: Some(true),
        self_kind: None,
        limit: Some(5),
        offset: Some(5),
        summary: Some(true),
    })
    .await
    .expect("functions_with_filter");
    let body2 = first_text(&page2);
    let v2: serde_json::Value = serde_json::from_str(&body2).expect("page2 JSON");
    let matches2: Vec<String> = v2
        .get("matches")
        .and_then(|x| x.as_array())
        .expect("matches array")
        .iter()
        .map(|m| m.get("qualified_name").and_then(|s| s.as_str()).unwrap_or("").to_string())
        .collect();

    assert_eq!(
        v2.get("offset").and_then(|x| x.as_u64()).unwrap() as usize,
        5,
        "offset must echo back the request"
    );
    // If total > 5, page2's first row must differ from page1's first row.
    if total > 5 && !matches1.is_empty() && !matches2.is_empty() {
        assert_ne!(
            matches1[0], matches2[0],
            "offset=5 must shift the match window vs offset=0"
        );
    }
}

/// Item #7: `top_n` caps the number of metric rows returned.
#[tokio::test]
async fn crate_dependency_metric_top_n_caps_count() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    let result = crate_dependency_metric(
        crate::tools::params::CrateDependencyMetricParams {
            directory: manifest_dir.to_string(),
            top_n: Some(3),
            sort_by: Some("item_count".to_string()),
            pagination: ListPaginationParams::default(),
        },
    )
    .await
    .expect("crate_dependency_metric");

    let body = first_text(&result);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
    let metrics = v
        .get("metrics")
        .and_then(|x| x.as_array())
        .expect("metrics array present");
    assert!(
        metrics.len() <= 3,
        "top_n=3 must cap metrics.len(), got {}",
        metrics.len()
    );
    let crate_count = v
        .get("crate_count")
        .and_then(|x| x.as_u64())
        .expect("crate_count present") as usize;
    assert_eq!(
        crate_count,
        metrics.len(),
        "crate_count must equal metrics.len() after slicing"
    );
}

/// Item #7: `sort_by=instability` sorts metrics non-increasing.
#[tokio::test]
async fn crate_dependency_metric_sort_by_instability_descending() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    let result = crate_dependency_metric(
        crate::tools::params::CrateDependencyMetricParams {
            directory: manifest_dir.to_string(),
            top_n: None,
            sort_by: Some("instability".to_string()),
            pagination: ListPaginationParams::default(),
        },
    )
    .await
    .expect("crate_dependency_metric");

    let body = first_text(&result);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
    let metrics = v
        .get("metrics")
        .and_then(|x| x.as_array())
        .expect("metrics array present");
    let instabilities: Vec<f64> = metrics
        .iter()
        .map(|m| {
            m.get("instability")
                .and_then(|x| x.as_f64())
                .expect("instability is a number")
        })
        .collect();
    for w in instabilities.windows(2) {
        assert!(
            w[0] >= w[1],
            "instability must be non-increasing under sort_by=instability: {:?}",
            instabilities
        );
    }
}

/// Item #7: an unknown `sort_by` value is rejected with `invalid_params`.
#[tokio::test]
async fn crate_dependency_metric_unknown_sort_by_errors() {
    let _guard = DEFAULT_SNAPSHOT_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(false),
    })
    .await
    .expect("build_hypergraph");

    let result = crate_dependency_metric(
        crate::tools::params::CrateDependencyMetricParams {
            directory: manifest_dir.to_string(),
            top_n: None,
            sort_by: Some("garbage_key".to_string()),
            pagination: ListPaginationParams::default(),
        },
    )
    .await;

    let err = result.expect_err("unknown sort_by must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("sort_by") && msg.contains("garbage_key"),
        "error must mention both `sort_by` and the bad value, got: {msg}"
    );
}

#[test]
fn graph_audit_error_maps_typed_audit_failures_to_invalid_params() {
    let err = graph_audit_error("fn_body_audit")(GraphAuditError::InvalidPattern(
        "unknown pattern `nope`".to_string(),
    )
    .into());

    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("unknown pattern `nope`"),
        "typed graph audit error should be exposed directly, got: {}",
        err.message
    );
}

#[test]
fn graph_audit_error_maps_untyped_failures_to_internal_error() {
    let err = graph_audit_error("unsafe_audit")(anyhow::anyhow!("storage failed"));

    assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
    assert!(
        err.message.contains("unsafe_audit: storage failed"),
        "untyped audit errors should keep the endpoint label, got: {}",
        err.message
    );
}

fn first_text(result: &CallToolResult) -> String {
    result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

// ----- semantic_overlaps unit tests -----
//
// Note: a `resolve_chunk_to_item` end-to-end test would need a fully
// populated LMDB snapshot, which is hard to make deterministic.
// The plan defers it; we only test the pure helpers here:
//   - `line_range_overlaps` (the overlap predicate),
//   - `build_clusters` (single-linkage union-find).

fn nid(byte: u8) -> NodeId {
    let mut id = [0u8; 32];
    id[0] = byte;
    NodeId(id)
}

#[test]
fn line_range_overlaps_inclusive_bounds() {
    // touching at endpoints counts as overlap (inclusive ranges).
    assert!(line_range_overlaps(1, 5, 5, 10));
    assert!(line_range_overlaps(5, 10, 1, 5));
    // strictly disjoint
    assert!(!line_range_overlaps(1, 4, 5, 10));
    assert!(!line_range_overlaps(5, 10, 1, 4));
    // containment
    assert!(line_range_overlaps(1, 100, 50, 60));
    assert!(line_range_overlaps(50, 60, 1, 100));
    // identical
    assert!(line_range_overlaps(7, 7, 7, 7));
}

#[test]
fn build_clusters_two_groups_drops_singletons() {
    let a = nid(1);
    let b = nid(2);
    let c = nid(3);
    let d = nid(4);
    let e = nid(5);
    let edges = vec![
        (a, b, 0.90),
        (b, c, 0.85),
        (d, e, 0.95),
    ];
    let lookup = |id: NodeId| {
        Some(ItemRef {
            qualified_name: format!("n_{}", id.as_bytes()[0]),
            item_kind: Some("Fn".to_string()),
            file: Some("x.rs".to_string()),
            span: Some((0, 0)),
        })
    };

    let clusters = build_clusters(&edges, 50, lookup);

    // Two clusters {A,B,C} and {D,E}; no singletons.
    assert_eq!(clusters.len(), 2);
    // Sorted by avg_similarity desc: {D,E} (avg 0.95) before {A,B,C} (avg 0.875).
    assert_eq!(clusters[0].size, 2);
    assert_eq!(clusters[1].size, 3);
    assert!(!clusters[0].truncated);
    assert!(!clusters[1].truncated);
    // {D,E} avg / min both 0.95.
    assert!((clusters[0].avg_similarity - 0.95).abs() < 1e-5);
    assert!((clusters[0].min_similarity - 0.95).abs() < 1e-5);
    // {A,B,C} avg of 0.90 and 0.85 = 0.875.
    let abc_avg = clusters[1].avg_similarity;
    assert!((abc_avg - 0.875).abs() < 1e-5, "avg = {abc_avg}");
    // min_similarity for {A,B,C} is 0.85.
    assert!((clusters[1].min_similarity - 0.85).abs() < 1e-5);
}

#[test]
fn build_clusters_max_members_caps_and_marks_truncated() {
    let a = nid(1);
    let b = nid(2);
    let c = nid(3);
    let d = nid(4);
    // 4-node single cluster via single-linkage chain.
    let edges = vec![
        (a, b, 0.90),
        (b, c, 0.90),
        (c, d, 0.90),
    ];
    let lookup = |id: NodeId| {
        Some(ItemRef {
            qualified_name: format!("n_{}", id.as_bytes()[0]),
            item_kind: None,
            file: Some("x.rs".to_string()),
            span: Some((0, 0)),
        })
    };
    let clusters = build_clusters(&edges, 2, lookup);
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].size, 4);
    assert_eq!(clusters[0].members.len(), 2);
    assert!(clusters[0].truncated);
}

#[test]
fn item_ref_summary_omits_file_and_span() {
    let node = Node {
        id: nid(1),
        kind: NodeKind::Item,
        display_name: "thing".to_string(),
        qualified_name: "crate::thing".to_string(),
        crate_id: None,
        parent_id: None,
        item_kind: Some(ItemKind::Function),
        file: Some("src/lib.rs".to_string()),
        span: Some((10, 20)),
        visibility: Some("pub".to_string()),
        attributes: Vec::new(),
        crate_target_kind: None,
    };

    let full = serde_json::to_value(node_to_item_ref(&node, false)).unwrap();
    assert_eq!(full["file"], "src/lib.rs");
    assert_eq!(full["span"][0], 10);
    assert_eq!(full["span"][1], 20);

    let summary = serde_json::to_value(node_to_item_ref(&node, true)).unwrap();
    assert_eq!(summary["qualified_name"], "crate::thing");
    assert!(summary.get("file").is_none());
    assert!(summary.get("span").is_none());
}

#[test]
fn page_clusters_caps_total_emitted_members() {
    let mk_ref = |name: &str| ItemRef {
        qualified_name: name.to_string(),
        item_kind: Some("Fn".to_string()),
        file: Some("x.rs".to_string()),
        span: Some((0, 0)),
    };
    let clusters = vec![
        SimilarityCluster {
            members: vec![mk_ref("a"), mk_ref("b"), mk_ref("c")],
            avg_similarity: 0.95,
            min_similarity: 0.90,
            size: 3,
            truncated: false,
        },
        SimilarityCluster {
            members: vec![mk_ref("d"), mk_ref("e")],
            avg_similarity: 0.90,
            min_similarity: 0.85,
            size: 2,
            truncated: false,
        },
    ];

    let paged = page_clusters_by_member_limit(clusters, 0, 4);

    assert_eq!(paged.len(), 2);
    assert_eq!(paged[0].members.len(), 3);
    assert!(!paged[0].truncated);
    assert_eq!(paged[1].members.len(), 1);
    assert!(paged[1].truncated);
    let emitted: usize = paged.iter().map(|c| c.members.len()).sum();
    assert_eq!(emitted, 4);
}

#[test]
fn page_clusters_offset_skips_whole_clusters() {
    let mk_cluster = |name: &str| SimilarityCluster {
        members: vec![ItemRef {
            qualified_name: name.to_string(),
            item_kind: None,
            file: Some("x.rs".to_string()),
            span: Some((0, 0)),
        }],
        avg_similarity: 0.9,
        min_similarity: 0.9,
        size: 1,
        truncated: false,
    };
    let paged = page_clusters_by_member_limit(
        vec![mk_cluster("skip"), mk_cluster("keep")],
        1,
        50,
    );

    assert_eq!(paged.len(), 1);
    assert_eq!(paged[0].members[0].qualified_name, "keep");
}

#[test]
fn page_list_default_limit_caps_and_reports_total() {
    let items: Vec<usize> = (0..75).collect();
    let (page, paged) = page_list(items, list_page(&ListPaginationParams::default()));

    assert_eq!(page.total_match_count, 75);
    assert_eq!(page.offset, 0);
    assert_eq!(page.limit, 50);
    assert!(!page.summary);
    assert_eq!(page.returned_match_count, 50);
    assert_eq!(paged.len(), 50);
    assert_eq!(paged[0], 0);
    assert_eq!(paged[49], 49);
}

#[test]
fn page_list_offset_and_limit_slice_results() {
    let params = ListPaginationParams {
        limit: Some(3),
        offset: Some(4),
        summary: Some(true),
    };
    let (page, paged) = page_list((0..10).collect::<Vec<_>>(), list_page(&params));

    assert_eq!(page.total_match_count, 10);
    assert_eq!(page.offset, 4);
    assert_eq!(page.limit, 3);
    assert!(page.summary);
    assert_eq!(page.returned_match_count, 3);
    assert_eq!(paged, vec![4, 5, 6]);
}

#[test]
fn usage_summary_omits_navigation_fields() {
    let usage = EnrichedUsage {
        file: None,
        start: None,
        end: None,
        category: "Read",
        consumer_module: Some("crate::module".to_string()),
        consumer_function: Some("crate::module::caller".to_string()),
    };

    let body = serde_json::to_value(&usage).unwrap();
    assert!(body.get("file").is_none());
    assert!(body.get("start").is_none());
    assert!(body.get("end").is_none());
    assert_eq!(body["category"], "Read");
    assert_eq!(body["consumer_module"], "crate::module");
    assert_eq!(body["consumer_function"], "crate::module::caller");
}

#[test]
fn call_site_summary_omits_navigation_fields() {
    let site = rmc_graph::graph::EnrichedCallSite {
        caller_qualified_name: Some("crate::caller".to_string()),
        callee_qualified_name: "crate::callee".to_string(),
        file: "src/lib.rs".to_string(),
        start: 10,
        end: 20,
        category: "Read".to_string(),
    };

    let summary = serde_json::to_value(&call_site_views(vec![site.clone()], true)[0]).unwrap();
    assert!(summary.get("file").is_none());
    assert!(summary.get("start").is_none());
    assert!(summary.get("end").is_none());
    assert_eq!(summary["caller_qualified_name"], "crate::caller");
    assert_eq!(summary["callee_qualified_name"], "crate::callee");
    assert_eq!(summary["category"], "Read");

    let full = serde_json::to_value(&call_site_views(vec![site], false)[0]).unwrap();
    assert_eq!(full["file"], "src/lib.rs");
    assert_eq!(full["start"], 10);
    assert_eq!(full["end"], 20);
}

#[test]
fn build_clusters_empty_input() {
    let edges: Vec<(NodeId, NodeId, f32)> = Vec::new();
    let lookup = |_id: NodeId| -> Option<ItemRef> { None };
    let clusters = build_clusters(&edges, 50, lookup);
    assert!(clusters.is_empty());
}

// -----------------------------------------------------------------
// Pass-1 polish: `handle_build_codemap` MCP parameter validation.
//
// Validation runs before `open_workspace_snapshot`, so these tests
// don't need a real snapshot fixture — `/tmp` is a stand-in that
// will never be touched by the failing branches.
// -----------------------------------------------------------------

#[tokio::test]
async fn build_codemap_requires_prompt_or_seeds() {
    let result = handle_build_codemap(
        "/tmp", // never opened — validation fails first
        None,   // task_prompt
        None,   // seed_qualified_names
        None,   // max_nodes
        None,   // depth
        None,   // max_incoming_per_node
        None,   // embedding_policy
        None,   // format
        None,   // include_snippets
    )
    .await;
    let err = result.expect_err("missing prompt and seeds should reject");
    let msg = err.message.as_ref();
    assert!(
        msg.contains("task_prompt") && msg.contains("seed_qualified_names"),
        "error message should mention both knobs, got: {msg}"
    );
}

#[tokio::test]
async fn build_codemap_rejects_bad_format() {
    let result = handle_build_codemap(
        "/tmp",
        Some("anything"),
        None,
        None,
        None,
        None,
        None,
        Some("weird"),
        None,
    )
    .await;
    let err = result.expect_err("unknown format should reject");
    assert_eq!(
        err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "expected INVALID_PARAMS"
    );
    let msg = err.message.as_ref();
    assert!(msg.contains("json"), "message should list valid options: {msg}");
    assert!(msg.contains("mermaid"), "message should list valid options: {msg}");
    assert!(msg.contains("outline"), "message should list valid options: {msg}");
    assert!(msg.contains("all"), "message should list valid options: {msg}");
}

#[tokio::test]
async fn build_codemap_rejects_bad_embedding_policy() {
    let result = handle_build_codemap(
        "/tmp",
        Some("anything"),
        None,
        None,
        None,
        None,
        Some("turbo"),
        None,
        None,
    )
    .await;
    let err = result.expect_err("unknown embedding_policy should reject");
    assert_eq!(
        err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "expected INVALID_PARAMS"
    );
    let msg = err.message.as_ref();
    assert!(
        msg.contains("no_rerank"),
        "message should list valid options: {msg}"
    );
    assert!(
        msg.contains("cached_only"),
        "message should list valid options: {msg}"
    );
    assert!(
        msg.contains("compute_missing"),
        "message should list valid options: {msg}"
    );
}
