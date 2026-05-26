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
use super::skeleton::*;
use super::surface::*;

use super::audits::graph_audit_error;

use rmc_graph::graph::{EnrichedUsage, GraphAuditError};
use ra_ap_syntax::SourceFile;
use crate::tools::params::{
    BuildHypergraphParams, CrateSkeletonParams, CrateTypesParams, DeadPubParams, GraphExportsParams,
    GraphImportsParams, ListPaginationParams, ModuleDependenciesParams, WhoImportsParams,
    WhoUsesParams,
};
use rmcp::model::{CallToolResult, ErrorCode};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tempfile::{Builder, TempDir};

// Default-snapshot cases share `~/.local/share/search/graphs/...`. heed forbids
// opening the same env twice in the same process, so one async test runs those
// cases sequentially instead of relying on `--test-threads=1`.
static DEFAULT_SNAPSHOT_BUILT: OnceLock<()> = OnceLock::new();

fn test_project_root() -> String {
    env!("CARGO_MANIFEST_DIR").to_string()
}

fn test_workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root")
        .to_path_buf()
}

struct WorkspaceSkeletonPathGuard {
    entries: Vec<WorkspaceSkeletonPath>,
    restored: bool,
    _temp_dir: TempDir,
}

struct WorkspaceSkeletonPath {
    original_path: PathBuf,
    preserved_path: Option<PathBuf>,
}

impl WorkspaceSkeletonPathGuard {
    fn new(workspace_root: &Path) -> Self {
        let preserve_root = workspace_root
            .parent()
            .expect("workspace root has a parent for preservation");
        let temp_dir = Builder::new()
            .prefix(".crate-skeleton-test-preserve-")
            .tempdir_in(preserve_root)
            .expect("create crate_skeleton preservation dir");
        let mut guard = Self {
            entries: Vec::new(),
            restored: false,
            _temp_dir: temp_dir,
        };
        guard
            .preserve(workspace_root, ".skeleton")
            .expect("preserve existing .skeleton");
        guard
            .preserve(workspace_root, ".skeleton-backup")
            .expect("preserve existing .skeleton-backup");
        guard
    }

    fn preserve(&mut self, workspace_root: &Path, name: &str) -> io::Result<()> {
        let original_path = workspace_root.join(name);
        let preserved_path = match fs::symlink_metadata(&original_path) {
            Ok(_) => {
                let preserved_path = self
                    ._temp_dir
                    .path()
                    .join(name.strip_prefix('.').unwrap_or(name));
                fs::rename(&original_path, &preserved_path)?;
                Some(preserved_path)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        };
        self.entries.push(WorkspaceSkeletonPath {
            original_path,
            preserved_path,
        });
        Ok(())
    }

    fn restore(mut self) {
        self.restore_all()
            .expect("restore pre-existing workspace .skeleton paths");
        self.restored = true;
    }

    fn restore_all(&mut self) -> io::Result<()> {
        for entry in self.entries.iter().rev() {
            remove_path_if_exists(&entry.original_path)?;
            if let Some(preserved_path) = &entry.preserved_path {
                fs::rename(preserved_path, &entry.original_path)?;
            }
        }
        Ok(())
    }
}

impl Drop for WorkspaceSkeletonPathGuard {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        if let Err(err) = self.restore_all() {
            eprintln!("failed to restore crate_skeleton test workspace paths: {err}");
        }
    }
}

fn remove_path_if_exists(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn assert_generated_skeleton_rust_file_parses(path: &Path) {
    let source = fs::read_to_string(path).unwrap_or_else(|err| {
        panic!(
            "read generated skeleton Rust file {}: {err}",
            path.display()
        )
    });
    let parsed = SourceFile::parse(&source, ra_ap_syntax::Edition::Edition2024);
    assert!(
        parsed.errors().is_empty(),
        "{} parse errors: {:?}\n{}",
        path.display(),
        parsed.errors(),
        source
    );
}

async fn ensure_default_snapshot(directory: &str) {
    if DEFAULT_SNAPSHOT_BUILT.get().is_none() {
        build_hypergraph(BuildHypergraphParams {
            directory: directory.to_string(),
            force_rebuild: Some(true),
        })
        .await
        .expect("build_hypergraph");
        let _ = DEFAULT_SNAPSHOT_BUILT.set(());
    }
}

#[tokio::test]
async fn default_snapshot_graph_round_trips_with_crate_types_and_crate_skeleton() {
    mcp_round_trip_against_self().await;
    get_exports_accepts_crate_name_as_consumer().await;
    who_uses_and_dead_pub_round_trip().await;
    functions_with_filter_default_limit_caps_results().await;
    functions_with_filter_summary_mode_omits_signature().await;
    functions_with_filter_offset_pagination().await;
    crate_dependency_metric_top_n_caps_count().await;
    crate_dependency_metric_sort_by_instability_descending().await;
    crate_dependency_metric_unknown_sort_by_errors().await;
    crate_types_round_trip().await;
    crate_skeleton_round_trip().await;
}

/// Round-trip: build_hypergraph → get_imports / who_imports against this
/// crate. Uses the default data dir so the snapshot lifecycle exercised
/// here mirrors what an MCP client would see.
async fn mcp_round_trip_against_self() {
    let manifest_dir = test_project_root();

    let build = build_hypergraph(BuildHypergraphParams {
        directory: manifest_dir.to_string(),
        force_rebuild: Some(true),
    })
    .await
    .expect("build_hypergraph");
    let _ = DEFAULT_SNAPSHOT_BUILT.set(());
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
        module: "rmc_server::tools::endpoints::query".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("module_dependencies");
    let body = first_text(&dependencies);
    assert!(
        body.contains("\"target_module\": \"rmc_indexing::indexing::search\""),
        "expected query endpoint to depend on indexing search facade: {body}"
    );
    assert!(
        body.contains("\"target_qualified\": \"rmc_indexing::indexing::search::open_bm25_search\""),
        "expected open_bm25_search symbol in module dependency payload: {body}"
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

/// Regression: passing a Crate qualified name (e.g. `rmc_server`)
/// where a Module is expected (`get_exports`'s `consumer`) should be
/// transparent — the resolver should fall through to the crate's root
/// module rather than erroring with "is a Crate, expected Module".
async fn get_exports_accepts_crate_name_as_consumer() {
    let manifest_dir = test_project_root();

    // Ensure a snapshot exists for the workspace.
    ensure_default_snapshot(&manifest_dir).await;

    let exports = get_exports(GraphExportsParams {
        directory: manifest_dir.to_string(),
        // `rmc_graph::graph` re-exports `load` (from loader),
        // visible from anywhere inside the crate.
        module: "rmc_graph::graph".to_string(),
        // Crate name, NOT a module path — must be transparently
        // promoted to the crate's root module.
        consumer: "rmc_server".to_string(),
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
async fn who_uses_and_dead_pub_round_trip() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

    // who_uses against a helper we know is referenced inside the server crate.
    let users = who_uses(WhoUsesParams {
        directory: manifest_dir.to_string(),
        target: "rmc_server::tools::graph::response::json_result".to_string(),
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
        krate: "rmc_server".to_string(),
        pagination: ListPaginationParams::default(),
    })
    .await
    .expect("dead_pub_in_crate");
    let body = first_text(&dead);
    assert!(
        body.contains("\"findings\""),
        "expected a findings array in response: {body}"
    );
}

/// Item #4: default `limit=50` caps the matches returned by the
/// wrapper, while `total_match_count` always reflects the unfiltered
/// (pre-slice) count. We use `is_async=true` as the permissive filter
/// (signatures.rs::tests confirms this returns >0 matches in the
/// workspace). The default-limit cap holds whether or not the
/// workspace currently has > 50 async fns: `match_count <= limit` and
/// `total_match_count >= match_count` regardless.
async fn functions_with_filter_default_limit_caps_results() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

    let result = functions_with_filter(crate::tools::params::FunctionsWithFilterParams {
        directory: manifest_dir.to_string(),
        krate: "rmc_server".to_string(),
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
async fn functions_with_filter_summary_mode_omits_signature() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

    let result = functions_with_filter(crate::tools::params::FunctionsWithFilterParams {
        directory: manifest_dir.to_string(),
        krate: "rmc_server".to_string(),
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
async fn functions_with_filter_offset_pagination() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

    // First page.
    let page1 = functions_with_filter(crate::tools::params::FunctionsWithFilterParams {
        directory: manifest_dir.to_string(),
        krate: "rmc_server".to_string(),
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
        krate: "rmc_server".to_string(),
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
async fn crate_dependency_metric_top_n_caps_count() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

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
async fn crate_dependency_metric_sort_by_instability_descending() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

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
async fn crate_dependency_metric_unknown_sort_by_errors() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

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

async fn crate_types_round_trip() {
    let manifest_dir = test_project_root();

    ensure_default_snapshot(&manifest_dir).await;

    let result = crate_types(CrateTypesParams {
        directory: manifest_dir.to_string(),
        krate: "rmc_graph".to_string(),
        item_kind: None,
        pub_only: None,
        include_associated_types: None,
        skip_test_items: Some(true),
        pagination: ListPaginationParams {
            limit: Some(5),
            offset: Some(0),
            summary: Some(false),
        },
    })
    .await
    .expect("crate_types");
    let body = first_text(&result);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
    assert_eq!(v["krate"], "rmc_graph");
    assert!(
        v.get("type_count").and_then(|x| x.as_u64()).unwrap_or(0) > 0,
        "expected crate_types to find at least one type: {body}"
    );
    let types = v
        .get("types")
        .and_then(|x| x.as_array())
        .expect("types array present");
    assert!(
        !types.is_empty(),
        "expected returned types after pagination: {body}"
    );
    assert!(
        body.contains("\"qualified_name\""),
        "expected qualified_name fields in crate_types response: {body}"
    );

    let summary = crate_types(CrateTypesParams {
        directory: manifest_dir.to_string(),
        krate: "rmc_graph".to_string(),
        item_kind: Some(vec!["Struct".to_string()]),
        pub_only: None,
        include_associated_types: None,
        skip_test_items: Some(true),
        pagination: ListPaginationParams {
            limit: Some(1),
            offset: Some(0),
            summary: Some(true),
        },
    })
    .await
    .expect("crate_types summary");
    let body = first_text(&summary);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
    let first = v["types"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.as_object())
        .expect("one summary type object");
    assert!(
        !first.contains_key("file") && !first.contains_key("span"),
        "summary=true must omit file/span: {first:?}"
    );
    assert_eq!(
        v.get("returned_match_count").and_then(|x| x.as_u64()),
        Some(1),
        "limit=1 must return one item in this fixture: {body}"
    );

    let invalid = crate_types(CrateTypesParams {
        directory: manifest_dir.to_string(),
        krate: "rmc_graph".to_string(),
        item_kind: Some(vec!["Function".to_string()]),
        pub_only: None,
        include_associated_types: None,
        skip_test_items: Some(true),
        pagination: ListPaginationParams::default(),
    })
    .await;
    let err = invalid.expect_err("non-type item_kind should be rejected");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("crate_types only accepts"),
        "invalid kind error should explain accepted type kinds, got: {}",
        err.message
    );
}

async fn crate_skeleton_round_trip() {
    let workspace_root = test_workspace_root();
    let workspace_skeleton_paths = WorkspaceSkeletonPathGuard::new(&workspace_root);
    let workspace = workspace_root.display().to_string();
    build_hypergraph(BuildHypergraphParams {
        directory: workspace.clone(),
        force_rebuild: Some(true),
    })
    .await
    .expect("build workspace-root snapshot for crate_skeleton");

    let skeleton_dir = workspace_root.join(".skeleton");
    let backup_dir = workspace_root.join(".skeleton-backup");
    let stale_file = skeleton_dir.join("stale/old.rs");
    let backup_file = backup_dir.join("sentinel");
    fs::create_dir_all(stale_file.parent().expect("stale parent"))
        .expect("create stale skeleton dir");
    fs::write(&stale_file, "stale").expect("write stale skeleton file");
    fs::create_dir_all(&backup_dir).expect("create backup dir");
    fs::write(&backup_file, "keep").expect("write backup sentinel");

    let result = crate_skeleton(CrateSkeletonParams {
        directory: workspace.clone(),
        crates: Some(vec!["rmc_server".to_string()]),
        include: Some(vec!["all".to_string()]),
        include_docs: Some(true),
        include_attrs: Some(true),
        include_impls: Some(true),
        skip_test_items: Some(true),
        exclude_vendor: Some(true),
        clean: Some(true),
        pagination: ListPaginationParams {
            limit: Some(1),
            offset: Some(0),
            summary: Some(false),
        },
    })
    .await
    .expect("crate_skeleton");

    let body = first_text(&result);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
    assert_eq!(
        v["skeleton_dir"].as_str(),
        Some(skeleton_dir.to_string_lossy().as_ref())
    );
    assert!(v["total_files"].as_u64().unwrap_or(0) > 0, "{body}");
    assert!(v["total_items"].as_u64().unwrap_or(0) > 0, "{body}");
    assert_eq!(v["page"]["returned_match_count"].as_u64(), Some(1));
    assert!(
        v["page"]["total_match_count"].as_u64().unwrap_or(0)
            >= v["total_files"].as_u64().unwrap_or(0)
    );

    let first = v["files_written"]
        .as_array()
        .and_then(|files| files.first())
        .expect("one paged file summary");
    let source_path = first["source_path"]
        .as_str()
        .expect("source path string");
    let written = skeleton_dir.join(source_path);
    assert!(written.exists(), "expected generated file {}", written.display());
    let generated = fs::read_to_string(&written).expect("read generated skeleton file");
    assert!(generated.contains("@generated by rust-code-mcp crate_skeleton"));
    let server_router = skeleton_dir.join("crates/rmc-server/src/tools/router.rs");
    assert!(
        server_router.exists(),
        "expected server router skeleton at {}",
        server_router.display()
    );
    let server_router_text = fs::read_to_string(&server_router).expect("read server router skeleton");
    assert!(server_router_text.contains("fn crate_skeleton"));
    assert_generated_skeleton_rust_file_parses(&server_router);
    assert!(!stale_file.exists(), "clean=true should remove stale skeleton files");
    assert!(
        backup_file.exists(),
        "clean=true must not remove sibling .skeleton-backup"
    );

    let summary = crate_skeleton(CrateSkeletonParams {
        directory: workspace,
        crates: Some(vec!["rmc_graph".to_string()]),
        include: None,
        include_docs: None,
        include_attrs: None,
        include_impls: None,
        skip_test_items: None,
        exclude_vendor: None,
        clean: Some(false),
        pagination: ListPaginationParams {
            limit: Some(5),
            offset: Some(0),
            summary: Some(true),
        },
    })
    .await
    .expect("crate_skeleton summary");
    let body = first_text(&summary);
    let v: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("summary response was not valid JSON: {e} — body: {body}"));
    assert_eq!(v["page"]["summary"].as_bool(), Some(true));
    assert_eq!(v["page"]["returned_match_count"].as_u64(), Some(0));
    assert!(
        v["files_written"]
            .as_array()
            .map(|files| files.is_empty())
            .unwrap_or(false),
        "summary=true should omit file summaries: {body}"
    );
    let graph_model = skeleton_dir.join("crates/rmc-graph/src/graph/model.rs");
    assert!(
        graph_model.exists(),
        "expected graph model skeleton at {}",
        graph_model.display()
    );
    let graph_model_text = fs::read_to_string(&graph_model).expect("read graph model skeleton");
    assert!(graph_model_text.contains("pub struct Node"));
    assert_generated_skeleton_rust_file_parses(&graph_model);

    workspace_skeleton_paths.restore();
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

// ----- retained vector-to-graph helper tests -----

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
        None,   // search_cache
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
