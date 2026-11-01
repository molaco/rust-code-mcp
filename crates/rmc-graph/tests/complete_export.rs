//! Canonical row equality, pagination and independently established fixture counts.
use std::{fs, path::Path};
use rmc_graph::graph::{self, export::{self, ExportCursor}};

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "target" { continue; }
        if entry.file_type().unwrap().is_dir() { copy_tree(&entry.path(), &to.join(entry.file_name())); }
        else { fs::copy(entry.path(), to.join(entry.file_name())).unwrap(); }
    }
}

#[test]
fn complete_export_preserves_every_row_and_duplicate_key() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    copy_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph_truth"), work.path());
    graph::build_and_persist(work.path(), graph::BuildOptions { data_dir_override: Some(data.path().to_owned()), ..Default::default() }).unwrap();
    let snap = graph::open_current(&graph::GraphPaths::for_workspace_in(data.path(), work.path()), Default::default()).unwrap().unwrap();
    assert_eq!((snap.manifest.node_count, snap.manifest.binding_count, snap.manifest.usage_count), (55, 41, 67));
    let counts = snap.manifest.usage_counts.as_ref().unwrap();
    assert_eq!((counts.source_sites, counts.expanded_observations), (64, 3));
    let context = snap.manifest.extraction_context.as_ref().unwrap();
    assert_eq!(context["features"], "all");
    assert!(context["targets"].as_array().unwrap().iter().any(|t| t["cfg_options"].as_str().unwrap().contains("feature=extra")));
    for table in export::TABLES {
        let whole = export::export_page(&snap, table, None, 10000).unwrap();
        assert_eq!(whole.rows.len() as u64, whole.total_rows);
        assert!(whole.next_cursor.is_none());
        for limit in [1, 2, 17] {
            let mut cursor = None;
            let mut rows = Vec::new();
            loop {
                let page = export::export_page(&snap, table, cursor.as_ref(), limit).unwrap();
                assert_eq!(page.total_rows, whole.total_rows);
                rows.extend(page.rows);
                cursor = page.next_cursor;
                if cursor.is_none() { break; }
            }
            assert_eq!(rows, whole.rows, "lost or repeated {table} rows at page size {limit}");
        }
    }
    // Compare direct storage reads with exported records, not only two calls
    // to the same exporter. Includes a duplicate parent with many children.
    let txn = snap.read_txn().unwrap();
    let nodes: Vec<_> = snap.dbs.nodes_by_id.iter(&txn).unwrap().map(|r| serde_json::to_value(r.unwrap().1).unwrap()).collect();
    let usages: Vec<_> = snap.dbs.usages_by_id.iter(&txn).unwrap().map(|r| serde_json::to_value(r.unwrap().1).unwrap()).collect();
    let bindings: Vec<_> = snap.dbs.bindings_by_id.iter(&txn).unwrap().map(|r| serde_json::to_value(r.unwrap().1).unwrap()).collect();
    for (table, expected) in [("nodes", nodes), ("usages", usages), ("bindings", bindings)] {
        let page = export::export_page(&snap, table, None, 10000).unwrap();
        assert_eq!(page.rows.into_iter().map(|r| r.record).collect::<Vec<_>>(), expected);
    }
    let contains = export::export_page(&snap, "contains", None, 10000).unwrap();
    assert_eq!(contains.rows.len() as u64, snap.dbs.children_by_parent.len(&txn).unwrap());
    assert!(contains.rows.windows(2).any(|w| w[0].key == w[1].key));
    let mut c = ExportCursor { graph_id: "another".into(), table: "nodes".into(), key: "00".into(), value: "00".into() };
    assert!(export::export_page(&snap, "nodes", Some(&c), 1).is_err());
    c.graph_id = snap.manifest.graph_id.clone();
    c.table = "contains".into();
    assert!(export::export_page(&snap, "nodes", Some(&c), 1).is_err());
    c.table = "nodes".into(); c.key = "oops".into();
    assert!(export::export_page(&snap, "nodes", Some(&c), 1).is_err());
    assert!(export::export_page(&snap, "nodes", None, 0).is_err());
    assert!(export::export_page(&snap, "nodes", None, 10001).is_err());
    assert!(export::export_page(&snap, "unknown", None, 1).is_err());
    drop(txn);
    let old_origins = context["row_origins"].as_object().unwrap();
    assert_eq!(old_origins.len(), context["targets"].as_array().unwrap().len());
    let bin = work.path().join("app/src/main.rs");
    fs::write(&bin, format!("{}\npub fn export_marker() {{}}\n", fs::read_to_string(&bin).unwrap())).unwrap();
    let build = graph::build_and_persist(work.path(), graph::BuildOptions { data_dir_override: Some(data.path().to_owned()), ..Default::default() }).unwrap();
    assert!(matches!(build.kind, graph::BuildKind::Incremental { .. }));
    let newer = graph::open_current(&graph::GraphPaths::for_workspace_in(data.path(), work.path()), Default::default()).unwrap().unwrap();
    let newer_context = newer.manifest.extraction_context.as_ref().unwrap();
    assert_eq!(newer_context["incremental_base"], snap.manifest.graph_id);
    for (target, origin) in old_origins {
        if target.starts_with("truth_app[") {
            assert_eq!(newer_context["row_origins"][target]["graph_id"], newer.manifest.graph_id);
        } else {
            assert_eq!(&newer_context["row_origins"][target], origin, "clean target lost its extraction provenance");
        }
    }
    assert!(export::export_page(&newer, "nodes", Some(&ExportCursor { graph_id: snap.manifest.graph_id.clone(), table: "nodes".into(), key: "00".into(), value: "00".into() }), 1).is_err());
    // Older v16 JSON manifests remain readable; do not invent their provenance.
    let manifest_path = newer.snapshot_dir.join("manifest.json");
    let mut legacy: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("extraction_context");
    fs::write(&manifest_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let legacy = graph::open_current(&graph::GraphPaths::for_workspace_in(data.path(), work.path()), Default::default()).unwrap().unwrap();
    assert!(legacy.manifest.extraction_context.is_none());
    assert_eq!(export::export_page(&legacy, "nodes", None, 10000).unwrap().total_rows, 56);
}
