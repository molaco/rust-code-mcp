//! Node operations for the hypergraph

mod add_node;
mod count_nodes;
mod find_node;
mod get_node;
mod get_neighbors;
mod remove_node;

// Export test helper
#[cfg(test)]
pub(crate) mod tests {
    use crate::hypergraph::HyperNode;
    use crate::hypergraph::indexes::NodeId;
    use crate::parser::{Symbol, SymbolKind, Range, Visibility};
    use std::path::PathBuf;

    pub fn create_test_node(name: &str) -> HyperNode {
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
}
