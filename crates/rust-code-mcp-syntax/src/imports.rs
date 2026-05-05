//! Import extraction for tracking dependencies

use ra_ap_syntax::{
    ast::{self, HasModuleItem, HasName},
    AstNode, Edition, SourceFile,
};

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

/// Extract all import statements from source code (convenience wrapper that parses internally)
pub fn extract_imports(source: &str) -> Vec<Import> {
    extract_imports_with_edition(source, Edition::Edition2021)
}

/// Extract all import statements from source code with a specific Rust edition
pub fn extract_imports_with_edition(source: &str, edition: Edition) -> Vec<Import> {
    let parse = SourceFile::parse(source, edition);
    let file = parse.tree();
    extract_imports_from_ast(&file)
}

/// Extract imports from a pre-parsed AST (avoids re-parsing)
pub fn extract_imports_from_ast(file: &SourceFile) -> Vec<Import> {
    let mut imports = Vec::new();

    for item in file.items() {
        if let ast::Item::Use(use_item) = item {
            extract_use_tree(use_item.use_tree(), &mut imports, String::new());
        }
    }

    imports
}

/// Extract imports from a use tree recursively
fn extract_use_tree(use_tree: Option<ast::UseTree>, imports: &mut Vec<Import>, prefix: String) {
    let Some(tree) = use_tree else { return };

    let path = tree
        .path()
        .map(|p| {
            let path_str = p.syntax().text().to_string();
            if prefix.is_empty() {
                path_str
            } else {
                format!("{}::{}", prefix, path_str)
            }
        })
        .unwrap_or(prefix.clone());

    // Check for glob (*)
    if tree.star_token().is_some() {
        imports.push(Import {
            path: path.clone(),
            is_glob: true,
            items: vec![],
        });
        return;
    }

    // Check for use list {a, b, c}
    if let Some(use_tree_list) = tree.use_tree_list() {
        for subtree in use_tree_list.use_trees() {
            extract_use_tree(Some(subtree), imports, path.clone());
        }
        return;
    }

    // Check for rename (as)
    let rename = tree
        .rename()
        .and_then(|r| r.name().map(|n| n.text().to_string()));

    // Simple import
    if !path.is_empty() {
        imports.push(Import {
            path,
            is_glob: false,
            items: rename.map(|r| vec![r]).unwrap_or_default(),
        });
    }
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

    #[test]
    fn test_simple_import() {
        let source = "use std::collections::HashMap;";
        let imports = extract_imports(source);

        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, "std::collections::HashMap");
        assert!(!imports[0].is_glob);
    }

    #[test]
    fn test_glob_import() {
        let source = "use std::collections::*;";
        let imports = extract_imports(source);

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
        let imports = extract_imports(source);

        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|i| i.path.contains("HashMap")));
        assert!(imports.iter().any(|i| i.path.contains("File")));
        assert!(imports.iter().any(|i| i.path.contains("Read")));
    }

    #[test]
    fn test_grouped_imports() {
        let source = "use std::io::{Read, Write};";
        let imports = extract_imports(source);

        assert_eq!(imports.len(), 2);
        assert!(imports.iter().any(|i| i.path.contains("Read")));
        assert!(imports.iter().any(|i| i.path.contains("Write")));
    }

    #[test]
    fn test_external_dependencies() {
        let source = r#"
            use std::collections::HashMap;
            use serde::Serialize;
            use tokio::runtime::Runtime;
        "#;
        let imports = extract_imports(source);
        let deps = get_external_dependencies(&imports);

        assert!(deps.contains(&"std".to_string()));
        assert!(deps.contains(&"serde".to_string()));
        assert!(deps.contains(&"tokio".to_string()));
    }

    #[test]
    fn test_local_import() {
        let source = "use crate::parser::Symbol;";
        let imports = extract_imports(source);

        assert_eq!(imports.len(), 1);
        assert!(imports[0].path.starts_with("crate"));
    }
}
