use super::{core::export_graph, response::open_workspace_snapshot};
use crate::tools::params::GraphExportParams;
use rmc_graph::graph::{self, BuildOptions};
use serde_json::{json, Value};

fn body(result: rmcp::model::CallToolResult) -> Value {
    match &result.content[0].raw {
        rmcp::model::RawContent::Text(text) => serde_json::from_str(&text.text).unwrap(),
        _ => panic!("expected JSON text"),
    }
}

#[tokio::test]
async fn export_graph_pins_all_tables_and_rejects_replaced_current() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname='export_api'\nversion='0.1.0'\nedition='2021'\n[workspace]\n").unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "pub fn leaf() {} pub fn run() { leaf(); }\n").unwrap();
    graph::build_and_persist(root.path(), BuildOptions::default()).unwrap();
    let directory = root.path().display().to_string();
    let params = |table: Option<&str>, graph_id: Option<&str>, cursor| GraphExportParams {
        directory: directory.clone(), table: table.map(str::to_owned), graph_id: graph_id.map(str::to_owned), cursor, limit: Some(1),
    };
    let metadata = body(export_graph(params(None, None, None)).await.unwrap());
    let id = metadata["manifest"]["graph_id"].as_str().unwrap();
    assert!(export_graph(params(Some("nodes"), None, None)).await.is_err());
    let first = body(export_graph(params(Some("nodes"), Some(id), None)).await.unwrap());
    let mut cursor = first["next_cursor"].clone();
    assert_ne!(cursor, Value::Null);
    let old_cursor = cursor.to_string();
    let mut rows = first["rows"].as_array().unwrap().clone();
    while cursor != Value::Null {
        let page = body(export_graph(params(Some("nodes"), Some(id), Some(cursor.to_string()))).await.unwrap());
        rows.extend(page["rows"].as_array().unwrap().clone());
        cursor = page["next_cursor"].clone();
    }
    assert_eq!(json!(rows.len()), metadata["table_counts"]["nodes"]);
    let old = open_workspace_snapshot(&directory).unwrap();
    graph::build_and_persist(root.path(), BuildOptions { force_rebuild: true, ..Default::default() }).unwrap();
    let error = export_graph(params(Some("nodes"), Some(id), Some(old_cursor))).await.unwrap_err();
    assert!(error.message.contains("restart export"));
    assert_eq!(graph::export::export_page(&old, "nodes", None, 100).unwrap().rows.len(), rows.len());
    drop(old);
    assert!(graph::clear_workspace_snapshots(root.path(), graph::GraphSnapshotCleanupOptions::default()).errors.is_empty());
}
