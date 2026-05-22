//! Per-pass timing probe for the graph extraction & snapshot pipeline.
//!
//! Two modes:
//!   `extract` — load + extract only (no LMDB writes).
//!   `build`   — full build_and_persist (load + fingerprint + extract + write).
//!
//! Usage:
//!   EXTRACT_TIMING=1 cargo run --release --example timing_extract -- <workspace> [extract|build]
//!
//! Defaults: workspace=/home/molaco/Documents/coding-agent, mode=extract.

use std::path::Path;
use std::time::Instant;

use rmc_graph::graph::{
    BuildOptions, GraphEnvOptions, GraphPaths, NodeId, NodeKind, OpenedSnapshot, build_and_persist,
    extract, loader, open_current,
};
use rmc_graph::graph::model::Node;

fn pick_high_traffic_target(snap: &OpenedSnapshot) -> Option<(NodeId, Node)> {
    // Find some Item with the largest number of usages_by_target entries.
    let rtxn = snap.read_txn().ok()?;
    let mut best: Option<(NodeId, u32)> = None;
    let mut current_key: Option<[u8; 32]> = None;
    let mut current_count: u32 = 0;
    for entry in snap.dbs.usages_by_target.iter(&rtxn).ok()? {
        let (k, _v) = entry.ok()?;
        let mut id = [0u8; 32];
        id.copy_from_slice(k);
        if Some(id) == current_key {
            current_count += 1;
        } else {
            if let Some(prev) = current_key {
                if best.map_or(true, |(_, c)| current_count > c) {
                    best = Some((NodeId(prev), current_count));
                }
            }
            current_key = Some(id);
            current_count = 1;
        }
    }
    if let Some(prev) = current_key {
        if best.map_or(true, |(_, c)| current_count > c) {
            best = Some((NodeId(prev), current_count));
        }
    }
    drop(rtxn);
    let (id, _count) = best?;
    let rtxn = snap.read_txn().ok()?;
    let node = snap.node(&rtxn, id).ok().flatten()?;
    if node.kind != NodeKind::Item {
        return None;
    }
    Some((id, node))
}

fn main() {
    let workspace = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/molaco/Documents/coding-agent".to_string());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "extract".to_string());
    let workspace_path = Path::new(&workspace);
    eprintln!("workspace: {}", workspace_path.display());
    eprintln!("mode:      {mode}");
    if std::env::var_os("EXTRACT_TIMING").is_none() {
        eprintln!("note: EXTRACT_TIMING not set; per-pass timers will be silent.");
    }
    eprintln!();

    match mode.as_str() {
        "extract" => {
            let t = Instant::now();
            let loaded = loader::load(workspace_path).expect("load");
            eprintln!(
                "loader::load                          {:>9.2?}  ({} local crates)",
                t.elapsed(),
                loaded.local_crates.len()
            );
            // Dump local-crate names + their CrateOrigin classification.
            use ra_ap_hir::{Crate, db::HirDatabase};
            use ra_ap_ide::RootDatabase;
            fn label_origin(c: Crate, db: &RootDatabase) -> String {
                let _ = db as &dyn HirDatabase;
                let o = c.origin(db);
                format!("{o:?}")
            }
            for &k in &loaded.local_crates {
                let name = k
                    .display_name(&loaded.db)
                    .map(|n| n.canonical_name().as_str().to_string())
                    .unwrap_or_else(|| "?".into());
                eprintln!("  - {:<32}  origin={}", name, label_origin(k, &loaded.db));
            }
            let t = Instant::now();
            let model = extract::extract(&loaded);
            eprintln!("extract::extract (wall)               {:>9.2?}", t.elapsed());
            eprintln!();
            eprintln!("model summary:");
            eprintln!("  nodes:      {}", model.nodes.len());
            eprintln!("  bindings:   {}", model.bindings.len());
            eprintln!("  usages:     {}", model.usages.len());
            eprintln!("  contains:   {}", model.contains.len());
        }
        "build" => {
            let tempdir = tempfile::tempdir().expect("tempdir");
            let opts = BuildOptions {
                force_rebuild: true,
                data_dir_override: Some(tempdir.path().to_path_buf()),
                env: GraphEnvOptions::default(),
            };
            let t = Instant::now();
            let result = build_and_persist(workspace_path, opts).expect("build_and_persist");
            let total = t.elapsed();
            eprintln!("build_and_persist (wall)              {:>9.2?}", total);
            eprintln!();
            eprintln!("result:");
            eprintln!("  graph_id:    {}", result.graph_id);
            eprintln!("  reused:      {}", result.reused);
            eprintln!("  nodes:       {}", result.node_count);
            eprintln!("  bindings:    {}", result.binding_count);
            eprintln!("  usages:      {}", result.usage_count);
        }
        "read" => {
            // Time the snapshot-open path that every read MCP tool walks
            // through, plus a few representative queries.
            let canonical = workspace_path.canonicalize().expect("canonicalize");
            let paths = GraphPaths::for_workspace(&canonical);

            let t = Instant::now();
            let snap = open_current(&paths, GraphEnvOptions::default())
                .expect("open_current")
                .expect("snapshot exists; run `build` first if not");
            eprintln!(
                "open_current                          {:>9.2?}  (graph_id={})",
                t.elapsed(),
                snap.manifest.graph_id
            );

            let t = Instant::now();
            let stats = snap.workspace_stats().expect("workspace_stats");
            eprintln!(
                "workspace_stats                       {:>9.2?}  ({} nodes total)",
                t.elapsed(),
                stats.nodes.workspace
                    + stats.nodes.crate_
                    + stats.nodes.module
                    + stats.nodes.item
                    + stats.nodes.external_symbol
            );

            let t = Instant::now();
            let edges = snap.crate_edges().expect("crate_edges");
            eprintln!(
                "crate_edges                           {:>9.2?}  ({} edges)",
                t.elapsed(),
                edges.len()
            );

            let t = Instant::now();
            let report = snap.overlaps().expect("overlaps");
            eprintln!(
                "overlaps                              {:>9.2?}  ({} type collisions)",
                t.elapsed(),
                report.cross_crate_type_collisions.len()
            );

            let t = Instant::now();
            let report = snap.dead_pub_report().expect("dead_pub_report");
            eprintln!(
                "dead_pub_report                       {:>9.2?}  ({} crates)",
                t.elapsed(),
                report.len()
            );

            // Heaviest read tools: who_uses / who_uses_summary on a high-fan-in
            // symbol (if present). Pick the first crate's root module which is
            // usually a high-traffic target.
            if let Some((target_id, target_node)) = pick_high_traffic_target(&snap) {
                let t = Instant::now();
                let usages = snap.usages_of(target_id).expect("usages_of");
                eprintln!(
                    "usages_of (`{}`)\n   {:>9.2?}  ({} usages)",
                    target_node.qualified_name,
                    t.elapsed(),
                    usages.len()
                );
                let t = Instant::now();
                let rows = snap.who_uses_summary(target_id).expect("who_uses_summary");
                eprintln!(
                    "who_uses_summary same target          {:>9.2?}  ({} rows)",
                    t.elapsed(),
                    rows.len()
                );
            }
        }
        other => {
            eprintln!("unknown mode: {other} (expected `extract`, `build`, or `read`)");
            std::process::exit(2);
        }
    }
}
