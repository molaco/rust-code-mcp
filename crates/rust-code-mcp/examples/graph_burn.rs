//! Acceptance test for Layer 4 — build a hypergraph snapshot against burn,
//! the multi-crate workspace that previously crashed with MDB_BAD_VALSIZE.
//!
//! Usage: cargo run --release --example graph_burn -- <workspace> <data_dir>

use std::path::{Path, PathBuf};

use rmc_graph::graph::{BuildOptions, build_and_persist, open_current, GraphPaths, GraphEnvOptions};

fn main() {
    let mut args = std::env::args().skip(1);
    let workspace = args.next().expect("usage: graph_burn <workspace> <data_dir>");
    let data_dir: PathBuf = args
        .next()
        .map(PathBuf::from)
        .expect("usage: graph_burn <workspace> <data_dir>");

    let workspace_path = Path::new(&workspace);
    let opts = BuildOptions {
        data_dir_override: Some(data_dir.clone()),
        ..Default::default()
    };

    let started = std::time::Instant::now();
    match build_and_persist(workspace_path, opts) {
        Ok(result) => {
            eprintln!(
                "ok in {:?}: graph_id={} nodes={} bindings={} reused={} snapshot={}",
                started.elapsed(),
                result.graph_id,
                result.node_count,
                result.binding_count,
                result.reused,
                result.snapshot_path.display(),
            );
            // Re-open and dump basic counts.
            let paths = GraphPaths::for_workspace_in(&data_dir, &result.workspace_root);
            let opened = open_current(&paths, GraphEnvOptions::default())
                .expect("open_current ok")
                .expect("snapshot present");
            let rtxn = opened.read_txn().unwrap();
            let nodes = opened.dbs.nodes_by_id.len(&rtxn).unwrap();
            let bindings = opened.dbs.bindings_by_id.len(&rtxn).unwrap();
            eprintln!("readback: nodes_by_id={nodes} bindings_by_id={bindings}");
        }
        Err(e) => {
            eprintln!("ERR: {e:#}");
            std::process::exit(1);
        }
    }
}
