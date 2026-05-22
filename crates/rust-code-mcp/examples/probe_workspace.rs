//! General-purpose probe: build (or reuse) a snapshot at the default data
//! dir for any workspace, then run `who_imports` for one or more qualified
//! names and print fan-in by crate.
//!
//! Usage:
//!   cargo run --example probe_workspace -- <workspace> <qualified_name>...

use std::collections::BTreeMap;
use std::path::Path;

use rmc_graph::graph::{
    BuildOptions, GraphEnvOptions, GraphPaths, build_and_persist, open_current,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let workspace = args.next().expect("usage: probe_workspace <workspace> <qname>...");
    let targets: Vec<String> = args.collect();
    if targets.is_empty() {
        eprintln!("ERR: provide at least one qualified name to probe");
        std::process::exit(1);
    }

    let workspace_path = Path::new(&workspace);
    eprintln!("Building snapshot for {}…", workspace_path.display());
    let started = std::time::Instant::now();
    let r = build_and_persist(
        workspace_path,
        BuildOptions {
            force_rebuild: true,
            ..Default::default()
        },
    )
    .expect("build_and_persist");
    eprintln!(
        "  built in {:?}: nodes={} bindings={}",
        started.elapsed(),
        r.node_count,
        r.binding_count
    );
    eprintln!();

    let paths = GraphPaths::for_workspace(&r.workspace_root);
    let snap = open_current(&paths, GraphEnvOptions::default())
        .expect("open")
        .expect("snapshot present");

    for qname in &targets {
        eprintln!("=== who_imports({qname}) ===");
        let resolved = snap.lookup_by_qualified_name(qname).expect("lookup");
        let Some((target_id, target_node)) = resolved else {
            eprintln!("  not found.\n");
            continue;
        };
        eprintln!(
            "  resolved -> {} ({:?})",
            target_node.qualified_name, target_node.kind
        );

        let bindings = snap.who_imports(target_id).expect("who_imports");
        let mut by_crate: BTreeMap<String, usize> = BTreeMap::new();
        let rtxn = snap.read_txn().unwrap();
        for b in &bindings {
            let from_qual = snap
                .node_by_id(&rtxn, b.from_module)
                .ok()
                .flatten()
                .map(|n| n.qualified_name)
                .unwrap_or_default();
            let crate_prefix = from_qual.split("::").next().unwrap_or("?").to_string();
            *by_crate.entry(crate_prefix).or_default() += 1;
        }

        eprintln!("  total importers: {}", bindings.len());
        let mut sorted: Vec<_> = by_crate.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (k, v) in sorted.iter().take(20) {
            eprintln!("    {v:>4}  {k}");
        }
        eprintln!();
    }
}
