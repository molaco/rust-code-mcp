//! Call graph construction for tracking function relationships

use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Tree};

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

    /// Build a call graph from a parse tree
    pub fn build(tree: &Tree, source: &str) -> Self {
        let mut graph = Self::new();
        graph.extract_calls(tree.root_node(), source, None);
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

    /// Extract function calls from AST
    fn extract_calls(&mut self, node: Node, source: &str, current_function: Option<String>) {
        match node.kind() {
            "function_item" => {
                // Found a function definition - extract its name and recurse into its body
                if let Some(name) = self.get_function_name(node, source) {
                    // Recurse into function body with this as the current function
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        self.extract_calls(child, source, Some(name.clone()));
                    }
                }
            }
            "call_expression" => {
                // Found a function call
                if let (Some(caller), Some(callee)) =
                    (current_function.as_ref(), self.get_call_target(node, source))
                {
                    self.add_call(caller.clone(), callee);
                }
                // Continue recursing (calls can be nested)
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.extract_calls(child, source, current_function.clone());
                }
            }
            _ => {
                // Recurse into children
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.extract_calls(child, source, current_function.clone());
                }
            }
        }
    }

    /// Extract function name from function_item node
    fn get_function_name(&self, node: Node, source: &str) -> Option<String> {
        node.children(&mut node.walk())
            .find(|c| c.kind() == "identifier")
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
            .map(String::from)
    }

    /// Extract the target function name from a call_expression
    fn get_call_target(&self, node: Node, source: &str) -> Option<String> {
        // A call expression looks like:
        //   function(args)         - simple call
        //   obj.method(args)       - method call
        //   Type::function(args)   - associated function call

        // The first child is usually the function/method being called
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" => {
                    // Simple function call: foo()
                    return child.utf8_text(source.as_bytes()).ok().map(String::from);
                }
                "field_expression" => {
                    // Method call: obj.method()
                    // Get the field (method) name - it's usually the last child
                    let mut field_cursor = child.walk();
                    for field_child in child.children(&mut field_cursor) {
                        if field_child.kind() == "field_identifier" {
                            return field_child
                                .utf8_text(source.as_bytes())
                                .ok()
                                .map(String::from);
                        }
                    }
                }
                "scoped_identifier" => {
                    // Associated function: Type::function()
                    // Get the last identifier
                    let mut scoped_cursor = child.walk();
                    let identifiers: Vec<_> = child
                        .children(&mut scoped_cursor)
                        .filter(|c| c.kind() == "identifier" || c.kind() == "type_identifier")
                        .collect();

                    if let Some(last) = identifiers.last() {
                        return last.utf8_text(source.as_bytes()).ok().map(String::from);
                    }
                }
                "generic_function" => {
                    // Generic function call: func::<T>()
                    // Look for the identifier inside
                    let mut gen_cursor = child.walk();
                    for gen_child in child.children(&mut gen_cursor) {
                        if gen_child.kind() == "identifier" || gen_child.kind() == "scoped_identifier" {
                            return self.get_call_target(gen_child, source);
                        }
                    }
                }
                _ => continue,
            }
        }
        None
    }
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
    fn test_simple_call() {
        let source = r#"
            fn caller() {
                callee();
            }
            fn callee() {}
        "#;

        let tree = parse_rust(source);
        let graph = CallGraph::build(&tree, source);

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

        let tree = parse_rust(source);
        let graph = CallGraph::build(&tree, source);

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

        let tree = parse_rust(source);
        let graph = CallGraph::build(&tree, source);

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

        let tree = parse_rust(source);
        let graph = CallGraph::build(&tree, source);

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

        let tree = parse_rust(source);
        let graph = CallGraph::build(&tree, source);

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

        let tree = parse_rust(source);
        let graph = CallGraph::build(&tree, source);

        assert_eq!(graph.edge_count(), 3); // a->b, a->c, b->c
    }
}
