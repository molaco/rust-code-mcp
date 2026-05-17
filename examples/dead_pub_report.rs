//! Workspace-wide dead-pub report. Builds (or reuses) the snapshot, runs
//! `dead_pub_report`, and prints a markdown summary grouped by crate with
//! file:byte-range references for each finding.
//!
//! Usage:
//!   cargo run --release --example dead_pub_report -- <workspace> [--force]

use std::path::Path;

use rust_code_mcp::graph::{
    BuildOptions, GraphEnvOptions, GraphPaths, build_and_persist, open_current,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let workspace = args
        .next()
        .expect("usage: dead_pub_report <workspace> [--force]");
    let force_rebuild = args.any(|a| a == "--force");

    let workspace_path = Path::new(&workspace);
    eprintln!("workspace: {}", workspace_path.display());

    let started = std::time::Instant::now();
    let result = build_and_persist(
        workspace_path,
        BuildOptions {
            force_rebuild,
            ..Default::default()
        },
    )
    .expect("build_and_persist");
    eprintln!(
        "snapshot: {} ({} nodes, {} bindings, {} usages){} in {:?}",
        if result.reused { "reused" } else { "fresh" },
        result.node_count,
        result.binding_count,
        result.usage_count,
        if result.reused { "" } else { "" },
        started.elapsed()
    );

    let paths = GraphPaths::for_workspace(&result.workspace_root);
    let snap = open_current(&paths, GraphEnvOptions::default())
        .expect("open_current")
        .expect("snapshot exists");

    let report = snap.dead_pub_report().expect("dead_pub_report");
    let total: usize = report.iter().map(|c| c.findings.len()).sum();

    println!("# Dead-pub report");
    println!();
    println!("Workspace: `{}`", workspace_path.display());
    println!();
    println!("**{} candidate item(s) across {} crate(s).**", total, report.len());
    println!();
    println!("> Items declared `pub` with no cross-crate consumer (no external `use` and no external reference). Conservative: items used only through public type signatures may appear here even when their `pub` is load-bearing.");
    println!();

    if total == 0 {
        println!("_No findings._");
        return;
    }

    let rtxn = snap.env.read_txn().expect("read_txn");
    for crate_report in &report {
        if crate_report.findings.is_empty() {
            continue;
        }
        println!(
            "## `{}` — {} finding(s)",
            crate_report.crate_qualified_name,
            crate_report.findings.len()
        );
        println!();
        println!("| Item | Kind | Location |");
        println!("|---|---|---|");
        for f in &crate_report.findings {
            let node = snap.node_by_id(&rtxn, f.target).ok().flatten();
            let location = match node.as_ref().and_then(|n| n.file.as_ref()) {
                Some(file) => match node.as_ref().and_then(|n| n.span) {
                    Some((start, _end)) => format!("`{file}` @ byte {start}"),
                    None => format!("`{file}`"),
                },
                None => "_(unknown)_".to_string(),
            };
            println!(
                "| `{}` | {:?} | {} |",
                f.qualified_name, f.item_kind, location
            );
        }
        println!();
    }
}
