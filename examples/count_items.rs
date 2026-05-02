use file_search_mcp::graph::{
    BuildOptions, GraphEnvOptions, GraphPaths, build_and_persist, open_current,
};
use file_search_mcp::graph::model::NodeKind;

fn main() {
    let workspace = std::env::args().nth(1).unwrap();
    let r = build_and_persist(
        std::path::Path::new(&workspace),
        BuildOptions::default(),
    ).unwrap();
    let paths = GraphPaths::for_workspace(&r.workspace_root);
    let snap = open_current(&paths, GraphEnvOptions::default()).unwrap().unwrap();
    
    let rtxn = snap.env.read_txn().unwrap();
    let mut counts = std::collections::BTreeMap::new();
    for entry in snap.dbs.nodes_by_id.iter(&rtxn).unwrap() {
        let (_k, node) = entry.unwrap();
        *counts.entry(format!("{:?}", node.kind)).or_insert(0u32) += 1;
    }
    println!("{:#?}", counts);
}
