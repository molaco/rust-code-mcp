//! Call graph construction for tracking function relationships

use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{
    ast::{self, HasModuleItem, HasName},
    AstNode, Edition, SourceFile, SyntaxKind,
};

/// A call graph tracking function call relationships
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    /// Maps caller function to the functions it calls
    /// Key: function name, Value: set of called function names
    edges: HashMap<String, HashSet<String>>,
}

impl CallGraph {
    /// Create a new empty call graph
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }

    /// Build a call graph from source code
    pub fn build(source: &str) -> Self {
        Self::build_with_edition(source, Edition::Edition2021)
    }

    /// Build a call graph from source code with a specific Rust edition
    pub fn build_with_edition(source: &str, edition: Edition) -> Self {
        let parse = SourceFile::parse(source, edition);
        let file = parse.tree();
        let mut graph = Self::new();

        for item in file.items() {
            match &item {
                ast::Item::Fn(f) => {
                    if let Some(name) = f.name() {
                        let caller = name.text().to_string();
                        if let Some(body) = f.body() {
                            extract_calls_from_expr(body.syntax(), &caller, &mut graph);
                        }
                    }
                }
                ast::Item::Impl(i) => {
                    if let Some(assoc_items) = i.assoc_item_list() {
                        for assoc in assoc_items.assoc_items() {
                            if let ast::AssocItem::Fn(f) = assoc {
                                if let Some(name) = f.name() {
                                    let caller = name.text().to_string();
                                    if let Some(body) = f.body() {
                                        extract_calls_from_expr(body.syntax(), &caller, &mut graph);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        graph
    }

    /// Add a call edge (caller -> callee)
    pub fn add_call(&mut self, caller: String, callee: String) {
        self.edges
            .entry(caller)
            .or_insert_with(HashSet::new)
            .insert(callee);
    }

    /// Get all functions called by the given function
    pub fn get_callees(&self, caller: &str) -> Vec<&str> {
        self.edges
            .get(caller)
            .map(|callees| callees.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Get all functions that call the given function
    pub fn get_callers(&self, callee: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|(_, callees)| callees.contains(callee))
            .map(|(caller, _)| caller.as_str())
            .collect()
    }

    /// Get all functions in the graph
    pub fn all_functions(&self) -> HashSet<&str> {
        let mut functions = HashSet::new();
        for (caller, callees) in &self.edges {
            functions.insert(caller.as_str());
            for callee in callees {
                functions.insert(callee.as_str());
            }
        }
        functions
    }

    /// Get the number of edges in the graph
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|s| s.len()).sum()
    }

    /// Check if function A calls function B
    pub fn has_call(&self, caller: &str, callee: &str) -> bool {
        self.edges
            .get(caller)
            .map(|callees| callees.contains(callee))
            .unwrap_or(false)
    }
}

/// Extract function calls from an expression
fn extract_calls_from_expr(
    node: &ra_ap_syntax::SyntaxNode,
    caller: &str,
    graph: &mut CallGraph,
) {
    for descendant in node.descendants() {
        if descendant.kind() == SyntaxKind::CALL_EXPR {
            if let Some(call) = ast::CallExpr::cast(descendant.clone()) {
                if let Some(callee_name) = extract_call_target(&call) {
                    graph.add_call(caller.to_string(), callee_name);
                }
            }
        }
        if descendant.kind() == SyntaxKind::METHOD_CALL_EXPR {
            if let Some(method_call) = ast::MethodCallExpr::cast(descendant.clone()) {
                if let Some(name) = method_call.name_ref() {
                    graph.add_call(caller.to_string(), name.text().to_string());
                }
            }
        }
    }
}

/// Extract the target function name from a call expression
fn extract_call_target(call: &ast::CallExpr) -> Option<String> {
    let expr = call.expr()?;

    match expr {
        ast::Expr::PathExpr(path_expr) => {
            let path = path_expr.path()?;
            // Get the last segment (function name)
            path.segments()
                .last()
                .and_then(|seg| seg.name_ref())
                .map(|name| name.text().to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_call() {
        let source = r#"
            fn caller() {
                callee();
            }
            fn callee() {}
        "#;

        let graph = CallGraph::build(source);

        assert!(graph.has_call("caller", "callee"));
        assert_eq!(graph.get_callees("caller"), vec!["callee"]);
        assert_eq!(graph.get_callers("callee"), vec!["caller"]);
    }

    #[test]
    fn test_multiple_calls() {
        let source = r#"
            fn main() {
                foo();
                bar();
                foo();
            }
            fn foo() {}
            fn bar() {}
        "#;

        let graph = CallGraph::build(source);

        let callees = graph.get_callees("main");
        assert_eq!(callees.len(), 2); // foo and bar (deduplicated)
        assert!(callees.contains(&"foo"));
        assert!(callees.contains(&"bar"));
    }

    #[test]
    fn test_method_call() {
        let source = r#"
            fn process() {
                let s = String::new();
                s.len();
                s.push_str("test");
            }
        "#;

        let graph = CallGraph::build(source);

        let callees = graph.get_callees("process");
        assert!(callees.contains(&"new"));
        assert!(callees.contains(&"len"));
        assert!(callees.contains(&"push_str"));
    }

    #[test]
    fn test_nested_calls() {
        let source = r#"
            fn outer() {
                inner();
            }
            fn inner() {
                helper();
            }
            fn helper() {}
        "#;

        let graph = CallGraph::build(source);

        assert!(graph.has_call("outer", "inner"));
        assert!(graph.has_call("inner", "helper"));
        assert!(!graph.has_call("outer", "helper")); // Not a direct call
    }

    #[test]
    fn test_all_functions() {
        let source = r#"
            fn a() { b(); }
            fn b() { c(); }
            fn c() {}
        "#;

        let graph = CallGraph::build(source);

        let functions = graph.all_functions();
        assert_eq!(functions.len(), 3);
        assert!(functions.contains("a"));
        assert!(functions.contains("b"));
        assert!(functions.contains("c"));
    }

    #[test]
    fn test_edge_count() {
        let source = r#"
            fn a() {
                b();
                c();
            }
            fn b() { c(); }
            fn c() {}
        "#;

        let graph = CallGraph::build(source);

        assert_eq!(graph.edge_count(), 3); // a->b, a->c, b->c
    }
}
