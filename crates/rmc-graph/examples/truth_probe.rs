//! Fixture-scale raw evidence, deliberately independent of the handwritten oracle.
//! Usage: truth_probe extract|build|open WORKSPACE DATA_DIR OUTPUT.json
use std::path::Path;
use anyhow::{Context, Result, bail};
use rmc_graph::graph::{self, BuildOptions, GraphEnvOptions, GraphPaths, ItemKind};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 { bail!("usage: truth_probe extract|build|open WORKSPACE DATA_DIR OUTPUT.json"); }
    let root = Path::new(&args[2]).canonicalize()?;
    let output = if args[1] == "extract" {
        let loaded = graph::load(&root)?;
        let model = graph::extract_workspace_model(&loaded);
        json!({"mode": "extract", "nodes": model.nodes.values().collect::<Vec<_>>(),
            "bindings": model.bindings, "usages": model.usages, "contains": model.contains,
            "unresolved_calls": model.unresolved_calls})
    } else {
        let data = Path::new(&args[3]);
        if args[1] == "build" {
            graph::build_and_persist(&root, BuildOptions {
                data_dir_override: Some(data.to_owned()), ..Default::default()
            })?;
        } else if args[1] != "open" { bail!("unknown mode {}", args[1]); }
        let paths = GraphPaths::for_workspace_in(data, &root);
        let snap = graph::open_current(&paths, GraphEnvOptions::default())?.context("no snapshot")?;
        let txn = snap.read_txn()?;
        let nodes = snap.dbs.nodes_by_id.iter(&txn)?.map(|r| r.map(|(_, n)| n)).collect::<std::result::Result<Vec<_>, _>>()?;
        let usages = snap.dbs.usages_by_id.iter(&txn)?.map(|r| r.map(|(_, u)| u)).collect::<std::result::Result<Vec<_>, _>>()?;
        let bindings = snap.dbs.bindings_by_id.iter(&txn)?.map(|r| r.map(|(_, b)| b)).collect::<std::result::Result<Vec<_>, _>>()?;
        let unresolved = snap.dbs.unresolved_calls_by_owner.iter(&txn)?.map(|r| r.map(|(_, c)| c)).collect::<std::result::Result<Vec<_>, _>>()?.into_iter().flatten().collect::<Vec<_>>();
        drop(txn);
        let mut calls = Vec::new();
        for node in &nodes {
            if node.item_kind.is_some_and(ItemKind::is_callable) {
                calls.push(json!({"node": node.id, "calls_from": snap.calls_from(node.id)?,
                    "who_calls": snap.who_calls(node.id)?}));
            }
        }
        let seeds = vec!["truth_app::direct".to_owned(), "truth_app::macros".to_owned(),
            "truth_app::data".to_owned(), "truth_app::references_only".to_owned()];
        let codemap = graph::codemap::build_codemap(&snap, None, Some(&seeds), None,
            &graph::codemap::CodemapOptions { max_nodes: 200, depth: 3, ..Default::default() }, &[]).await?;
        json!({"mode": &args[1], "manifest": snap.manifest, "nodes": nodes,
            "bindings": bindings, "usages": usages, "calls": calls, "codemap": codemap,
            "unresolved_calls": unresolved})
    };
    std::fs::write(&args[4], serde_json::to_vec_pretty(&output)?)?;
    Ok(())
}
