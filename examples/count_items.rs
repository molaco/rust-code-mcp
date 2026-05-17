use rust_code_mcp::graph::{
    BuildOptions, GraphEnvOptions, GraphPaths, build_and_persist, open_current,
};

fn main() {
    let workspace = std::env::args().nth(1).unwrap();
    let t = std::time::Instant::now();
    let r = build_and_persist(
        std::path::Path::new(&workspace),
        BuildOptions {
            force_rebuild: true,
            ..Default::default()
        },
    )
    .unwrap();
    let elapsed = t.elapsed();
    println!(
        "build: {:?}  reused={}  nodes={}  bindings={}  usages={}",
        elapsed, r.reused, r.node_count, r.binding_count, r.usage_count
    );

    let paths = GraphPaths::for_workspace(&r.workspace_root);
    let snap = open_current(&paths, GraphEnvOptions::default()).unwrap().unwrap();
    let rtxn = snap.env.read_txn().unwrap();

    let mut node_counts = std::collections::BTreeMap::new();
    for entry in snap.dbs.nodes_by_id.iter(&rtxn).unwrap() {
        let (_k, node) = entry.unwrap();
        *node_counts.entry(format!("{:?}", node.kind)).or_insert(0u32) += 1;
    }
    println!("nodes: {:?}", node_counts);

    let mut usage_cat_counts = std::collections::BTreeMap::new();
    for entry in snap.dbs.usages_by_id.iter(&rtxn).unwrap() {
        let (_k, u) = entry.unwrap();
        *usage_cat_counts
            .entry(format!("{:?}", u.category))
            .or_insert(0u32) += 1;
    }
    println!("usages by category: {:?}", usage_cat_counts);
}
