use file_search_mcp::hypergraph::*;
use file_search_mcp::parser::{Symbol, SymbolKind, Range, Visibility};
use std::path::PathBuf;

fn create_test_node(name: &str) -> HyperNode {
    HyperNode {
        id: NodeId(0),
        name: name.to_string(),
        file_path: PathBuf::from("test.rs"),
        line_start: 1,
        line_end: 10,
        symbol: Symbol {
            kind: SymbolKind::Function {
                is_async: false,
                is_unsafe: false,
                is_const: false,
            },
            name: name.to_string(),
            range: Range {
                start_line: 1,
                end_line: 10,
                start_byte: 0,
                end_byte: 100,
            },
            docstring: None,
            visibility: Visibility::Public,
        },
    }
}

#[test]
fn test_basic_workflow() {
    let mut hg = Hypergraph::new();

    // Add nodes
    let main = hg.add_node(create_test_node("main")).unwrap();
    let parse = hg.add_node(create_test_node("parse_file")).unwrap();
    let build = hg.add_node(create_test_node("build_graph")).unwrap();

    // Add hyperedge: main calls parse and build
    let _edge = hg
        .add_hyperedge(
            [main].into(),
            [parse, build].into(),
            HyperedgeType::CallPattern,
            1.0,
        )
        .unwrap();

    // Query
    let neighbors = hg.get_neighbors(main).unwrap();
    assert_eq!(neighbors.len(), 2);
    assert!(neighbors.contains(&parse));
    assert!(neighbors.contains(&build));

    // Stats
    let stats = hg.stats();
    assert_eq!(stats.node_count, 3);
    assert_eq!(stats.edge_count, 1);
}

#[test]
fn test_many_to_many() {
    let mut hg = Hypergraph::new();

    let fn1 = hg.add_node(create_test_node("fn1")).unwrap();
    let fn2 = hg.add_node(create_test_node("fn2")).unwrap();
    let helper1 = hg.add_node(create_test_node("helper1")).unwrap();
    let helper2 = hg.add_node(create_test_node("helper2")).unwrap();

    // fn1 and fn2 both call helper1 and helper2
    hg.add_hyperedge(
        [fn1, fn2].into(),
        [helper1, helper2].into(),
        HyperedgeType::CallPattern,
        1.0,
    )
    .unwrap();

    // Verify directionality
    let from_fn1 = hg.get_neighbors_from(fn1).unwrap();
    assert_eq!(from_fn1, [helper1, helper2].into_iter().collect());

    let to_helper1 = hg.get_neighbors_to(helper1).unwrap();
    assert_eq!(to_helper1, [fn1, fn2].into_iter().collect());
}

#[test]
fn test_find_and_query() {
    let mut hg = Hypergraph::new();

    let main_fn = hg.add_node(create_test_node("main")).unwrap();
    let helper = hg.add_node(create_test_node("helper")).unwrap();

    hg.add_hyperedge(
        [main_fn].into(),
        [helper].into(),
        HyperedgeType::CallPattern,
        1.0,
    )
    .unwrap();

    // Find by name
    let found = hg.find_node_by_name("main");
    assert_eq!(found, Some(main_fn));

    // Get node details
    let node = hg.get_node(main_fn).unwrap();
    assert_eq!(node.name, "main");

    // Get hyperedges containing node
    let edges = hg.get_hyperedges_containing(main_fn).unwrap();
    assert_eq!(edges.len(), 1);
}

#[test]
fn test_stable_ids() {
    let mut hg = Hypergraph::new();

    let n1 = hg.add_node(create_test_node("fn1")).unwrap();
    let n2 = hg.add_node(create_test_node("fn2")).unwrap();
    let n3 = hg.add_node(create_test_node("fn3")).unwrap();

    assert_eq!(n1, NodeId(0));
    assert_eq!(n2, NodeId(1));
    assert_eq!(n3, NodeId(2));

    // IDs should remain stable even after operations
    let retrieved1 = hg.get_node(n1).unwrap();
    assert_eq!(retrieved1.id, n1);
    assert_eq!(retrieved1.name, "fn1");
}

#[test]
fn test_hyperedge_types() {
    let mut hg = Hypergraph::new();

    let module = hg.add_node(create_test_node("my_module")).unwrap();
    let func1 = hg.add_node(create_test_node("func1")).unwrap();
    let func2 = hg.add_node(create_test_node("func2")).unwrap();

    // Test ModuleContainment
    hg.add_hyperedge(
        [module].into(),
        [func1, func2].into(),
        HyperedgeType::ModuleContainment,
        1.0,
    )
    .unwrap();

    // Test CallPattern
    hg.add_hyperedge(
        [func1].into(),
        [func2].into(),
        HyperedgeType::CallPattern,
        1.0,
    )
    .unwrap();

    let stats = hg.stats();
    assert_eq!(stats.edge_count, 2);
}
