//! Type reference tracking for finding where types/structs are used
//!
//! Extracts type usage patterns from Rust code, including:
//! - Function parameters and return types
//! - Struct field types
//! - Generic type arguments
//! - Impl blocks
//! - Let bindings

use std::collections::HashMap;
use tree_sitter::{Node, Tree};

/// A reference to a type in the code
#[derive(Debug, Clone, PartialEq)]
pub struct TypeReference {
    /// The type name being referenced
    pub type_name: String,
    /// Where and how this type is used
    pub usage_context: TypeUsageContext,
    /// Line number where the reference appears
    pub line: usize,
}

/// Context describing how a type is being used
#[derive(Debug, Clone, PartialEq)]
pub enum TypeUsageContext {
    /// Type used in function parameter
    FunctionParameter { function_name: String },
    /// Type used as function return type
    FunctionReturn { function_name: String },
    /// Type used in struct field
    StructField { struct_name: String, field_name: String },
    /// Type used in impl block
    ImplBlock { trait_name: Option<String> },
    /// Type used in let binding
    LetBinding,
    /// Type used as generic argument
    GenericArgument,
}

/// Type reference tracker
#[derive(Debug, Default)]
pub struct TypeReferenceTracker {
    /// All type references found
    references: Vec<TypeReference>,
}

impl TypeReferenceTracker {
    /// Create a new empty tracker
    pub fn new() -> Self {
        Self {
            references: Vec::new(),
        }
    }

    /// Build type references from a parse tree
    pub fn build(tree: &Tree, source: &str) -> Vec<TypeReference> {
        let mut tracker = Self::new();
        tracker.extract_type_refs(tree.root_node(), source, None, None);
        tracker.references
    }

    /// Add a type reference
    fn add_reference(&mut self, reference: TypeReference) {
        self.references.push(reference);
    }

    /// Extract type references from AST recursively
    fn extract_type_refs(
        &mut self,
        node: Node,
        source: &str,
        current_function: Option<String>,
        current_struct: Option<String>,
    ) {
        match node.kind() {
            "function_item" => {
                // Extract function name and process parameters/return type
                let function_name = self.get_function_name(node, source);

                // Process parameters
                if let Some(params_node) = self.find_child_by_kind(node, "parameters") {
                    self.extract_from_parameters(params_node, source, function_name.as_ref());
                }

                // Process return type
                if let Some(return_type) = self.find_child_by_kind(node, "type_identifier") {
                    if let Some(type_name) = self.get_node_text(return_type, source) {
                        if let Some(ref fn_name) = function_name {
                            self.add_reference(TypeReference {
                                type_name,
                                usage_context: TypeUsageContext::FunctionReturn {
                                    function_name: fn_name.clone(),
                                },
                                line: return_type.start_position().row + 1,
                            });
                        }
                    }
                }

                // Recurse into function body
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.extract_type_refs(child, source, function_name.clone(), current_struct.clone());
                }
            }
            "struct_item" => {
                // Extract struct name and process fields
                let struct_name = self.get_struct_name(node, source);

                // Process struct fields
                if let Some(field_list) = self.find_child_by_kind(node, "field_declaration_list") {
                    self.extract_from_struct_fields(field_list, source, struct_name.as_ref());
                }

                // Recurse into children
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.extract_type_refs(child, source, current_function.clone(), struct_name.clone());
                }
            }
            "impl_item" => {
                // Extract type being implemented and optional trait name
                let (trait_name, type_name) = self.extract_impl_info(node, source);

                if let Some(ref type_name) = type_name {
                    self.add_reference(TypeReference {
                        type_name: type_name.clone(),
                        usage_context: TypeUsageContext::ImplBlock {
                            trait_name: trait_name.clone(),
                        },
                        line: node.start_position().row + 1,
                    });
                }

                // Recurse into impl body
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.extract_type_refs(child, source, current_function.clone(), current_struct.clone());
                }
            }
            "let_declaration" => {
                // Extract type from let bindings with explicit type annotations
                // Pattern: let x: Type = ...
                if let Some(type_id) = self.find_descendant_by_kind(node, "type_identifier") {
                    if let Some(type_name) = self.get_node_text(type_id, source) {
                        self.add_reference(TypeReference {
                            type_name,
                            usage_context: TypeUsageContext::LetBinding,
                            line: type_id.start_position().row + 1,
                        });
                    }
                }

                // Recurse into children
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.extract_type_refs(child, source, current_function.clone(), current_struct.clone());
                }
            }
            "type_arguments" => {
                // Extract types from generics: Vec<Type>, HashMap<K, V>
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "type_identifier" {
                        if let Some(type_name) = self.get_node_text(child, source) {
                            self.add_reference(TypeReference {
                                type_name,
                                usage_context: TypeUsageContext::GenericArgument,
                                line: child.start_position().row + 1,
                            });
                        }
                    } else {
                        // Recurse for nested generics
                        self.extract_type_refs(child, source, current_function.clone(), current_struct.clone());
                    }
                }
            }
            _ => {
                // Recurse into children for all other node types
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.extract_type_refs(child, source, current_function.clone(), current_struct.clone());
                }
            }
        }
    }

    /// Extract types from function parameters
    fn extract_from_parameters(&mut self, params_node: Node, source: &str, function_name: Option<&String>) {
        let mut cursor = params_node.walk();
        for child in params_node.children(&mut cursor) {
            if child.kind() == "parameter" {
                // Find type_identifier in parameter
                if let Some(type_id) = self.find_descendant_by_kind(child, "type_identifier") {
                    if let Some(type_name) = self.get_node_text(type_id, source) {
                        if let Some(fn_name) = function_name {
                            self.add_reference(TypeReference {
                                type_name,
                                usage_context: TypeUsageContext::FunctionParameter {
                                    function_name: fn_name.clone(),
                                },
                                line: type_id.start_position().row + 1,
                            });
                        }
                    }
                }

                // Also check for generic types in parameters
                if let Some(type_args) = self.find_descendant_by_kind(child, "type_arguments") {
                    self.extract_type_refs(type_args, source, function_name.cloned(), None);
                }
            }
        }
    }

    /// Extract types from struct fields
    fn extract_from_struct_fields(&mut self, field_list: Node, source: &str, struct_name: Option<&String>) {
        let mut cursor = field_list.walk();
        for child in field_list.children(&mut cursor) {
            if child.kind() == "field_declaration" {
                // Get field name
                let field_name = child
                    .children(&mut child.walk())
                    .find(|c| c.kind() == "field_identifier")
                    .and_then(|n| self.get_node_text(n, source));

                // Find type_identifier in field
                if let Some(type_id) = self.find_descendant_by_kind(child, "type_identifier") {
                    if let Some(type_name) = self.get_node_text(type_id, source) {
                        if let Some(struct_name) = struct_name {
                            self.add_reference(TypeReference {
                                type_name,
                                usage_context: TypeUsageContext::StructField {
                                    struct_name: struct_name.clone(),
                                    field_name: field_name.unwrap_or_else(|| "unknown".to_string()),
                                },
                                line: type_id.start_position().row + 1,
                            });
                        }
                    }
                }

                // Also check for generic types in fields
                if let Some(type_args) = self.find_descendant_by_kind(child, "type_arguments") {
                    self.extract_type_refs(type_args, source, None, struct_name.cloned());
                }
            }
        }
    }

    /// Extract impl block information (type and optional trait)
    fn extract_impl_info(&self, node: Node, source: &str) -> (Option<String>, Option<String>) {
        let type_identifiers: Vec<_> = node
            .children(&mut node.walk())
            .filter(|c| c.kind() == "type_identifier")
            .collect();

        if type_identifiers.is_empty() {
            return (None, None);
        }

        // Check for "for" keyword to distinguish trait impl from inherent impl
        let has_for_keyword = node
            .children(&mut node.walk())
            .any(|c| c.kind() == "for");

        if type_identifiers.len() >= 2 || has_for_keyword {
            // Trait impl: impl Trait for Type
            let trait_name = self.get_node_text(type_identifiers[0], source);
            let type_name = self.get_node_text(*type_identifiers.last().unwrap(), source);
            (trait_name, type_name)
        } else {
            // Inherent impl: impl Type
            (None, self.get_node_text(type_identifiers[0], source))
        }
    }

    /// Get function name from function_item node
    fn get_function_name(&self, node: Node, source: &str) -> Option<String> {
        node.children(&mut node.walk())
            .find(|c| c.kind() == "identifier")
            .and_then(|n| self.get_node_text(n, source))
    }

    /// Get struct name from struct_item node
    fn get_struct_name(&self, node: Node, source: &str) -> Option<String> {
        node.children(&mut node.walk())
            .find(|c| c.kind() == "type_identifier")
            .and_then(|n| self.get_node_text(n, source))
    }

    /// Find first child node with given kind
    fn find_child_by_kind<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        node.children(&mut node.walk()).find(|c| c.kind() == kind)
    }

    /// Find first descendant node with given kind (recursive search)
    fn find_descendant_by_kind<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = self.find_descendant_by_kind(child, kind) {
                return Some(found);
            }
        }
        None
    }

    /// Get text content of a node
    fn get_node_text(&self, node: Node, source: &str) -> Option<String> {
        node.utf8_text(source.as_bytes()).ok().map(String::from)
    }
}

/// Get all references to a specific type name
pub fn find_type_references<'a>(references: &'a [TypeReference], type_name: &str) -> Vec<&'a TypeReference> {
    references
        .iter()
        .filter(|r| r.type_name == type_name)
        .collect()
}

/// Get references grouped by file line
pub fn group_by_line(references: &[TypeReference]) -> HashMap<usize, Vec<&TypeReference>> {
    let mut grouped: HashMap<usize, Vec<&TypeReference>> = HashMap::new();
    for reference in references {
        grouped
            .entry(reference.line)
            .or_insert_with(Vec::new)
            .push(reference);
    }
    grouped
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
    fn test_function_parameter() {
        let source = r#"
            fn process(parser: RustParser) {
                // ...
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert_eq!(parser_refs.len(), 1);
        assert!(matches!(
            parser_refs[0].usage_context,
            TypeUsageContext::FunctionParameter { ref function_name } if function_name == "process"
        ));
    }

    #[test]
    fn test_function_return_type() {
        let source = r#"
            fn create() -> RustParser {
                // ...
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert_eq!(parser_refs.len(), 1);
        assert!(matches!(
            parser_refs[0].usage_context,
            TypeUsageContext::FunctionReturn { ref function_name } if function_name == "create"
        ));
    }

    #[test]
    fn test_struct_field() {
        let source = r#"
            struct Container {
                parser: RustParser,
                name: String,
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert_eq!(parser_refs.len(), 1);
        assert!(matches!(
            parser_refs[0].usage_context,
            TypeUsageContext::StructField { ref struct_name, ref field_name }
            if struct_name == "Container" && field_name == "parser"
        ));

        let string_refs = find_type_references(&refs, "String");
        assert_eq!(string_refs.len(), 1);
    }

    #[test]
    fn test_impl_block() {
        let source = r#"
            impl RustParser {
                fn new() -> Self {
                    // ...
                }
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert_eq!(parser_refs.len(), 1);
        assert!(matches!(
            parser_refs[0].usage_context,
            TypeUsageContext::ImplBlock { trait_name: None }
        ));
    }

    #[test]
    fn test_trait_impl_block() {
        let source = r#"
            impl Display for RustParser {
                // ...
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert!(!parser_refs.is_empty());

        let impl_refs: Vec<_> = parser_refs
            .iter()
            .filter(|r| matches!(r.usage_context, TypeUsageContext::ImplBlock { .. }))
            .collect();
        assert!(!impl_refs.is_empty());
    }

    #[test]
    fn test_generic_type_arguments() {
        let source = r#"
            fn process(items: Vec<RustParser>) {
                // ...
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert!(!parser_refs.is_empty());

        let generic_refs: Vec<_> = parser_refs
            .iter()
            .filter(|r| matches!(r.usage_context, TypeUsageContext::GenericArgument))
            .collect();
        assert!(!generic_refs.is_empty());
    }

    #[test]
    fn test_let_binding() {
        let source = r#"
            fn main() {
                let parser: RustParser = create_parser();
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert!(!parser_refs.is_empty());

        let let_refs: Vec<_> = parser_refs
            .iter()
            .filter(|r| matches!(r.usage_context, TypeUsageContext::LetBinding))
            .collect();
        assert!(!let_refs.is_empty());
    }

    #[test]
    fn test_multiple_contexts() {
        let source = r#"
            struct Container {
                parser: RustParser,
            }

            impl Container {
                fn new(parser: RustParser) -> Self {
                    Self { parser }
                }

                fn get_parser(&self) -> &RustParser {
                    &self.parser
                }
            }
        "#;

        let tree = parse_rust(source);
        let refs = TypeReferenceTracker::build(&tree, source);

        let parser_refs = find_type_references(&refs, "RustParser");
        // Should find: struct field, parameter in new()
        // Note: return type &RustParser is found
        assert!(parser_refs.len() >= 2, "Expected at least 2 references, found {}", parser_refs.len());

        // Verify we have the expected contexts
        let has_struct_field = parser_refs.iter().any(|r|
            matches!(r.usage_context, TypeUsageContext::StructField { .. })
        );
        let has_param = parser_refs.iter().any(|r|
            matches!(r.usage_context, TypeUsageContext::FunctionParameter { .. })
        );
        assert!(has_struct_field, "Should find struct field reference");
        assert!(has_param, "Should find function parameter reference");
    }
}
