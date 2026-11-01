//! Streaming complete JSONL export: graph_export WORKSPACE DATA_DIR OUTPUT.jsonl
use std::{path::Path, io::{BufWriter, Write}};
use anyhow::{Result, Context, bail};
use rmc_graph::graph::{self, export, GraphPaths, GraphEnvOptions};

fn main() -> Result<()> {
    match std::thread::Builder::new().name("graph-export".into()).stack_size(64 * 1024 * 1024).spawn(run)?.join() {
        Ok(result) => result,
        Err(payload) => {
            if let Some(cancelled) = payload.downcast_ref::<ra_ap_ide_db::base_db::salsa::Cancelled>() {
                bail!("graph extraction {cancelled}");
            }
            std::panic::resume_unwind(payload)
        }
    }
}

fn run() -> Result<()> {
    let mut args: Vec<_> = std::env::args().collect();
    let build = args.get(1).is_some_and(|a| a == "--build");
    if build { args.remove(1); }
    if args.len() != 4 { bail!("usage: graph_export [--build] WORKSPACE DATA_DIR OUTPUT.jsonl"); }
    let root = Path::new(&args[1]).canonicalize()?;
    if build {
        let result = graph::build_and_persist(&root, graph::BuildOptions {
            data_dir_override: Some(Path::new(&args[2]).to_owned()), ..Default::default()
        })?;
        eprintln!("graph_id={} nodes={} bindings={} usages={} reused={}", result.graph_id,
            result.node_count, result.binding_count, result.usage_count, result.reused);
    }
    let snap = graph::open_current(&GraphPaths::for_workspace_in(Path::new(&args[2]), &root), GraphEnvOptions::default())?.context("no current snapshot")?;
    let mut out = BufWriter::new(std::fs::File::create(&args[3])?);
    serde_json::to_writer(&mut out, &serde_json::json!({"metadata": export::export_metadata(&snap)?}))?;
    writeln!(out)?;
    for table in export::TABLES {
        let mut cursor = None;
        loop {
            let page = export::export_page(&snap, table, cursor.as_ref(), 1000)?;
            for row in page.rows {
                serde_json::to_writer(&mut out, &serde_json::json!({"table": table, "key": row.key, "record": row.record}))?;
                writeln!(out)?;
            }
            cursor = page.next_cursor;
            if cursor.is_none() { break; }
        }
    }
    out.flush()?;
    Ok(())
}
