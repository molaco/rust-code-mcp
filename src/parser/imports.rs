//! Import extraction for tracking dependencies

use tree_sitter::{Node, Tree};

/// An import statement in Rust code
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// The full import path (e.g., "std::collections::HashMap")
    pub path: String,
    /// Whether this is a glob import (use foo::*)
    pub is_glob: bool,
    /// Specific items imported (if any)
    pub items: Vec<String>,
}

/// Extract all import statements from a parse tree
pub fn extract_imports(tree: &Tree, source: &str) -> Vec<Import> {
    let mut imports = Vec::new();
    extract_imports_recursive(tree.root_node(), source, &mut imports);
    imports
}

/// Recursively extract imports from AST
fn extract_imports_recursive(node: Node, source: &str, imports: &mut Vec<Import>) {
    if node.kind() == "use_declaration" {
        if let Some(import) = parse_use_declaration(node, source) {
            imports.push(import);
        }
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_imports_recursive(child, source, imports);
    }
}

/// Parse a use declaration node into an Import
fn parse_use_declaration(node: Node, source: &str) -> Option<Import> {
    // Simpler approach: just get the full text and parse it
    let text = get_node_text(node, source)?;

    // Remove "use " and ";"
    let path_part = text
        .trim()
        .strip_prefix("use")?
        .trim()
        .strip_suffix(";")?
        .trim();

    // Check for glob import
    let is_glob = path_part.ends_with("*");
    let path = if is_glob {
        path_part.strip_suffix("::*").unwrap_or(path_part).trim().to_string()
    } else {
        path_part.to_string()
    };

    Some(Import {
        path,
        is_glob,
        items: vec![],
    })
}

/// Parse a use clause (recursive for nested imports)
fn parse_use_clause(node: Node, source: &str, prefix: String) -> Option<Import> {
    match node.kind() {
        "scoped_identifier" => {
            // Simple import: use std::collections::HashMap;
            let path = get_node_text(node, source)?;
            let full_path = if prefix.is_empty() {
                path
            } else {
                format!("{}::{}", prefix, path)
            };
            Some(Import {
                path: full_path,
                is_glob: false,
                items: vec![],
            })
        }
        "identifier" => {
            // Single identifier: use foo;
            let path = get_node_text(node, source)?;
            let full_path = if prefix.is_empty() {
                path
            } else {
                format!("{}::{}", prefix, path)
            };
            Some(Import {
                path: full_path,
                is_glob: false,
                items: vec![],
            })
        }
        "use_wildcard" | "use_as_clause" => {
            // Glob import: use std::collections::*;
            // or: use std::collections::HashMap as Map;
            let mut path_node = node.prev_sibling();
            let mut path_parts = Vec::new();

            // Walk backwards to collect the path
            while let Some(prev) = path_node {
                if prev.kind() == "identifier" || prev.kind() == "scoped_identifier" {
                    if let Some(text) = get_node_text(prev, source) {
                        path_parts.insert(0, text);
                    }
                }
                path_node = prev.prev_sibling();
            }

            let path = if !prefix.is_empty() {
                format!("{}::{}", prefix, path_parts.join("::"))
            } else {
                path_parts.join("::")
            };

            Some(Import {
                path,
                is_glob: node.kind() == "use_wildcard",
                items: vec![],
            })
        }
        "use_list" => {
            // Multiple imports: use std::collections::{HashMap, HashSet};
            let mut items = Vec::new();

            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" || child.kind() == "scoped_identifier" {
                    if let Some(item) = get_node_text(child, source) {
                        items.push(item);
                    }
                }
            }

            // Get the base path (everything before the {})
            let mut path_node = node.prev_sibling();
            let mut path_parts = Vec::new();

            while let Some(prev) = path_node {
                if prev.kind() == "identifier" || prev.kind() == "scoped_identifier" {
                    if let Some(text) = get_node_text(prev, source) {
                        path_parts.insert(0, text);
                    }
                }
                path_node = prev.prev_sibling();
            }

            let path = if !prefix.is_empty() {
                format!("{}::{}", prefix, path_parts.join("::"))
            } else {
                path_parts.join("::")
            };

            Some(Import {
                path,
                is_glob: false,
                items,
            })
        }
        _ => {
            // Try to parse children
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if let Some(import) = parse_use_clause(child, source, prefix.clone()) {
                    return Some(import);
                }
            }
            None
        }
    }
}

/// Helper: Get text content of a node
fn get_node_text(node: Node, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(String::from)
}

/// Get just the module/crate names from imports (for dependency tracking)
pub fn get_external_dependencies(imports: &[Import]) -> Vec<String> {
    imports
        .iter()
        .filter_map(|import| {
            // Get the first component (crate name)
            import.path.split("::").next().map(String::from)
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_rust(source: &str) -> Tree {
        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_rust::language().into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn test_simple_import() {
        let source = "use std::collections::HashMap;";
        let tree = parse_rust(source);
        let imports = extract_imports(&tree, source);

        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, "std::collections::HashMap");
        assert!(!imports[0].is_glob);
    }

    #[test]
    fn test_glob_import() {
        let source = "use std::collections::*;";
        let tree = parse_rust(source);
        let imports = extract_imports(&tree, source);

        assert_eq!(imports.len(), 1);
        assert!(imports[0].is_glob);
    }

    #[test]
    fn test_multiple_imports() {
        let source = r#"
            use std::collections::HashMap;
            use std::fs::File;
            use std::io::Read;
        "#;
        let tree = parse_rust(source);
        let imports = extract_imports(&tree, source);

        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|i| i.path.contains("HashMap")));
        assert!(imports.iter().any(|i| i.path.contains("File")));
        assert!(imports.iter().any(|i| i.path.contains("Read")));
    }

    #[test]
    fn test_external_dependencies() {
        let source = r#"
            use std::collections::HashMap;
            use serde::Serialize;
            use tokio::runtime::Runtime;
        "#;
        let tree = parse_rust(source);
        let imports = extract_imports(&tree, source);
        let deps = get_external_dependencies(&imports);

        assert!(deps.contains(&"std".to_string()));
        assert!(deps.contains(&"serde".to_string()));
        assert!(deps.contains(&"tokio".to_string()));
    }

    #[test]
    fn test_local_import() {
        let source = "use crate::parser::Symbol;";
        let tree = parse_rust(source);
        let imports = extract_imports(&tree, source);

        assert_eq!(imports.len(), 1);
        assert!(imports[0].path.starts_with("crate"));
    }
}
