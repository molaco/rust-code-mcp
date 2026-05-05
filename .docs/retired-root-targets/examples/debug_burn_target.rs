//! Debug: dump bindings_by_target for `burn_tensor::tensor::api::base::Tensor`
//! straight out of LMDB, bypassing all MCP serialization.
//!
//! Usage: cargo run --release --example debug_burn_target [-- <workspace> [<qualified_name>]]
//! Default workspace: /home/molaco/Documents/burn
//! Default symbol:    burn_tensor::tensor::api::base::Tensor
//!
//! Special mode: pass "--scan-tensor-nodes" as the second arg to instead list
//! every node whose qualified_name ends in "::Tensor" or starts with
//! "extern::" and contains "Tensor".

use std::collections::BTreeMap;
use std::path::Path;

use file_search_mcp::graph::{
    BindingId, GraphEnvOptions, GraphPaths, NodeId, open_current,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let workspace = args
        .next()
        .unwrap_or_else(|| "/home/molaco/Documents/burn".to_string());
    let qname = args
        .next()
        .unwrap_or_else(|| "burn_tensor::tensor::api::base::Tensor".to_string());

    let workspace_path = Path::new(&workspace);
    let paths = GraphPaths::for_workspace(workspace_path);
    eprintln!("workspace_hash = {}", paths.workspace_hash);
    eprintln!("snapshots_dir  = {}", paths.snapshots_dir.display());

    let opened = match open_current(&paths, GraphEnvOptions::default()) {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!("ERR no current snapshot found");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("ERR open_current: {e:#}");
            std::process::exit(1);
        }
    };

    eprintln!(
        "snapshot opened: nodes={} bindings={}",
        opened.manifest.node_count, opened.manifest.binding_count,
    );

    if qname == "--scan-tensor-nodes" {
        scan_tensor_nodes(&opened);
        return;
    }
    if qname == "--scan-externals" {
        scan_externals(&opened);
        return;
    }
    if qname == "--bindings-by-from-crate" {
        bindings_by_from_crate(&opened);
        return;
    }

    let (target_id, target_node) = match opened.lookup_by_qualified_name(&qname).unwrap() {
        Some(v) => v,
        None => {
            eprintln!("ERR symbol not found: {qname}");
            std::process::exit(2);
        }
    };
    println!(
        "target: kind={:?} item_kind={:?} qname={}",
        target_node.kind, target_node.item_kind, target_node.qualified_name
    );

    let rtxn = opened.read_txn().unwrap();

    // Iterate bindings_by_target.get_duplicates(target_id) and resolve each
    // BindingId to a Binding via bindings_by_id.
    let dup_iter = opened
        .dbs
        .bindings_by_target
        .get_duplicates(&rtxn, target_id.as_bytes())
        .unwrap();

    let mut total: usize = 0;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_crate_prefix: BTreeMap<String, usize> = BTreeMap::new();
    let mut entries: Vec<(String, String, String)> = Vec::new(); // (from_qname, kind, visible_name)

    if let Some(iter) = dup_iter {
        for entry in iter {
            let (_k, bid_bytes) = entry.unwrap();
            let mut bid = [0u8; 32];
            bid.copy_from_slice(bid_bytes);
            let _bid_marker = BindingId(bid);
            let binding = opened
                .dbs
                .bindings_by_id
                .get(&rtxn, &bid)
                .unwrap()
                .expect("dangling BindingId");

            let from_module: NodeId = binding.from_module;
            let from_node = opened.dbs.nodes_by_id.get(&rtxn, from_module.as_bytes()).unwrap();
            let from_qname = from_node
                .as_ref()
                .map(|n| n.qualified_name.clone())
                .unwrap_or_else(|| "<unknown>".to_string());

            let crate_prefix = from_qname
                .split("::")
                .next()
                .unwrap_or("<empty>")
                .to_string();

            *by_crate_prefix.entry(crate_prefix).or_insert(0) += 1;
            *by_kind.entry(format!("{:?}", binding.kind)).or_insert(0) += 1;
            entries.push((from_qname, format!("{:?}", binding.kind), binding.visible_name));
            total += 1;
        }
    }

    println!();
    println!("=== bindings_by_target total = {total} ===");
    println!();
    println!("--- per-crate histogram (from_module crate prefix) ---");
    for (crate_name, count) in &by_crate_prefix {
        println!("  {count:>5}  {crate_name}");
    }
    println!();
    println!("--- per-kind histogram ---");
    for (k, count) in &by_kind {
        println!("  {count:>5}  {k}");
    }
    println!();
    println!("--- all entries (from_module_qualified_name, kind, visible_name) ---");
    entries.sort();
    for (qn, k, vn) in &entries {
        println!("  [{k:>16}] {vn}  <-  {qn}");
    }
}

fn bindings_by_from_crate(opened: &file_search_mcp::graph::OpenedSnapshot) {
    let rtxn = opened.read_txn().unwrap();
    let mut by_crate: BTreeMap<String, usize> = BTreeMap::new();
    for entry in opened.dbs.bindings_by_id.iter(&rtxn).unwrap() {
        let (_k, b) = entry.unwrap();
        let from = opened.dbs.nodes_by_id.get(&rtxn, b.from_module.as_bytes()).unwrap();
        let q = from.map(|n| n.qualified_name).unwrap_or_else(|| "<?>".to_string());
        let prefix = q.split("::").next().unwrap_or("?").to_string();
        *by_crate.entry(prefix).or_insert(0) += 1;
    }
    println!("=== total bindings per from_module crate prefix ===");
    let mut v: Vec<_> = by_crate.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (k, c) in &v {
        println!("  {c:>6}  {k}");
    }
}

fn scan_externals(opened: &file_search_mcp::graph::OpenedSnapshot) {
    use file_search_mcp::graph::NodeKind;
    let rtxn = opened.read_txn().unwrap();
    let mut by_prefix: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    let mut samples: Vec<String> = Vec::new();
    for entry in opened.dbs.nodes_by_id.iter(&rtxn).unwrap() {
        let (_k, node) = entry.unwrap();
        if node.kind != NodeKind::ExternalSymbol {
            continue;
        }
        total += 1;
        let prefix = node
            .qualified_name
            .split("::")
            .nth(1)
            .unwrap_or("<none>")
            .to_string(); // skip "extern::" prefix or first segment
        *by_prefix.entry(prefix).or_insert(0) += 1;
        if samples.len() < 40 {
            samples.push(node.qualified_name.clone());
        }
    }
    println!("=== ExternalSymbol nodes total = {total} ===");
    println!("--- top prefixes ---");
    let mut v: Vec<_> = by_prefix.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (p, c) in v.iter().take(40) {
        println!("  {c:>6}  {p}");
    }
    println!("--- samples ---");
    for s in &samples {
        println!("  {s}");
    }
}

fn scan_tensor_nodes(opened: &file_search_mcp::graph::OpenedSnapshot) {
    let rtxn = opened.read_txn().unwrap();
    let mut hits: Vec<(String, String, usize)> = Vec::new();
    for entry in opened.dbs.nodes_by_id.iter(&rtxn).unwrap() {
        let (key, node) = entry.unwrap();
        let q = &node.qualified_name;
        let visible = q.ends_with("::Tensor")
            || q == "Tensor"
            || (q.starts_with("extern::") && q.ends_with("Tensor"));
        if !visible {
            continue;
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(key);
        let nid = NodeId(id);
        // Count bindings_by_target dups for this node.
        let mut count = 0usize;
        if let Some(iter) = opened
            .dbs
            .bindings_by_target
            .get_duplicates(&rtxn, nid.as_bytes())
            .unwrap()
        {
            for r in iter {
                r.unwrap();
                count += 1;
            }
        }
        hits.push((q.clone(), format!("{:?}/{:?}", node.kind, node.item_kind), count));
    }
    hits.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    println!("=== nodes whose qualified_name ends with ::Tensor ({}) ===", hits.len());
    for (q, k, c) in &hits {
        println!("  bindings_by_target={c:>4}  kind={k}  qname={q}");
    }
}
