//! Type reference tracking for finding where types/structs are used
//!
//! Extracts type usage patterns from Rust code, including:
//! - Function parameters and return types
//! - Struct field types
//! - Generic type arguments
//! - Impl blocks
//! - Let bindings

use std::collections::HashMap;

use ra_ap_syntax::{
    ast::{self, HasGenericArgs, HasModuleItem, HasName},
    AstNode, Edition, SourceFile,
};

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

    /// Build type references from source code (convenience wrapper that parses internally)
    pub fn build(source: &str) -> Vec<TypeReference> {
        Self::build_with_edition(source, Edition::Edition2021)
    }

    /// Build type references from source code with a specific Rust edition
    pub fn build_with_edition(source: &str, edition: Edition) -> Vec<TypeReference> {
        let parse = SourceFile::parse(source, edition);
        let file = parse.tree();
        Self::build_from_ast(&file, source)
    }

    /// Build type references from a pre-parsed AST (avoids re-parsing)
    /// Requires source string for line number calculation
    pub fn build_from_ast(file: &SourceFile, source: &str) -> Vec<TypeReference> {
        let mut refs = Vec::new();

        for item in file.items() {
            match &item {
                ast::Item::Fn(f) => {
                    let fn_name = f
                        .name()
                        .map(|n| n.text().to_string())
                        .unwrap_or_default();

                    // Parameters
                    if let Some(params) = f.param_list() {
                        for param in params.params() {
                            if let Some(ty) = param.ty() {
                                extract_types_from_type(
                                    &ty,
                                    source,
                                    &mut refs,
                                    TypeUsageContext::FunctionParameter {
                                        function_name: fn_name.clone(),
                                    },
                                );
                            }
                        }
                    }

                    // Return type
                    if let Some(ret) = f.ret_type() {
                        if let Some(ty) = ret.ty() {
                            extract_types_from_type(
                                &ty,
                                source,
                                &mut refs,
                                TypeUsageContext::FunctionReturn {
                                    function_name: fn_name.clone(),
                                },
                            );
                        }
                    }
                }
                ast::Item::Struct(s) => {
                    let struct_name = s
                        .name()
                        .map(|n| n.text().to_string())
                        .unwrap_or_default();

                    if let Some(field_list) = s.field_list() {
                        match field_list {
                            ast::FieldList::RecordFieldList(record) => {
                                for field in record.fields() {
                                    let field_name = field
                                        .name()
                                        .map(|n| n.text().to_string())
                                        .unwrap_or_default();
                                    if let Some(ty) = field.ty() {
                                        extract_types_from_type(
                                            &ty,
                                            source,
                                            &mut refs,
                                            TypeUsageContext::StructField {
                                                struct_name: struct_name.clone(),
                                                field_name,
                                            },
                                        );
                                    }
                                }
                            }
                            ast::FieldList::TupleFieldList(tuple) => {
                                for (i, field) in tuple.fields().enumerate() {
                                    if let Some(ty) = field.ty() {
                                        extract_types_from_type(
                                            &ty,
                                            source,
                                            &mut refs,
                                            TypeUsageContext::StructField {
                                                struct_name: struct_name.clone(),
                                                field_name: format!("{}", i),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                ast::Item::Impl(i) => {
                    let type_name = i.self_ty().map(|t| t.syntax().text().to_string());
                    let trait_name = i.trait_().map(|t| t.syntax().text().to_string());

                    if let Some(ref tn) = type_name {
                        refs.push(TypeReference {
                            type_name: tn.clone(),
                            usage_context: TypeUsageContext::ImplBlock {
                                trait_name: trait_name.clone(),
                            },
                            line: line_of_offset(source, i.syntax().text_range().start().into()),
                        });
                    }

                    // Process functions inside impl block
                    if let Some(assoc_items) = i.assoc_item_list() {
                        for assoc in assoc_items.assoc_items() {
                            if let ast::AssocItem::Fn(f) = assoc {
                                let fn_name = f
                                    .name()
                                    .map(|n| n.text().to_string())
                                    .unwrap_or_default();

                                // Parameters
                                if let Some(params) = f.param_list() {
                                    for param in params.params() {
                                        if let Some(ty) = param.ty() {
                                            extract_types_from_type(
                                                &ty,
                                                source,
                                                &mut refs,
                                                TypeUsageContext::FunctionParameter {
                                                    function_name: fn_name.clone(),
                                                },
                                            );
                                        }
                                    }
                                }

                                // Return type
                                if let Some(ret) = f.ret_type() {
                                    if let Some(ty) = ret.ty() {
                                        extract_types_from_type(
                                            &ty,
                                            source,
                                            &mut refs,
                                            TypeUsageContext::FunctionReturn {
                                                function_name: fn_name.clone(),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        refs
    }
}

/// Calculate line number (1-indexed) from byte offset
fn line_of_offset(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .chars()
        .filter(|&c| c == '\n')
        .count()
        + 1
}

/// Extract types from a type AST node
fn extract_types_from_type(
    ty: &ast::Type,
    source: &str,
    refs: &mut Vec<TypeReference>,
    context: TypeUsageContext,
) {
    match ty {
        ast::Type::PathType(path_ty) => {
            if let Some(path) = path_ty.path() {
                if let Some(segment) = path.segments().last() {
                    if let Some(name) = segment.name_ref() {
                        refs.push(TypeReference {
                            type_name: name.text().to_string(),
                            usage_context: context.clone(),
                            line: line_of_offset(source, name.syntax().text_range().start().into()),
                        });
                    }
                    // Handle generics like Vec<T>
                    if let Some(generic_args) = segment.generic_arg_list() {
                        for arg in generic_args.generic_args() {
                            if let ast::GenericArg::TypeArg(ref type_arg) = arg {
                                if let Some(inner_ty) = type_arg.ty() {
                                    extract_types_from_type(
                                        &inner_ty,
                                        source,
                                        refs,
                                        TypeUsageContext::GenericArgument,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        ast::Type::RefType(ref_ty) => {
            if let Some(inner) = ref_ty.ty() {
                extract_types_from_type(&inner, source, refs, context);
            }
        }
        _ => {}
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

    #[test]
    fn test_function_parameter() {
        let source = r#"
            fn process(parser: RustParser) {
                // ...
            }
        "#;

        let refs = TypeReferenceTracker::build(source);

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

        let refs = TypeReferenceTracker::build(source);

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

        let refs = TypeReferenceTracker::build(source);

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

        let refs = TypeReferenceTracker::build(source);

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

        let refs = TypeReferenceTracker::build(source);

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

        let refs = TypeReferenceTracker::build(source);

        let parser_refs = find_type_references(&refs, "RustParser");
        assert!(!parser_refs.is_empty());

        let generic_refs: Vec<_> = parser_refs
            .iter()
            .filter(|r| matches!(r.usage_context, TypeUsageContext::GenericArgument))
            .collect();
        assert!(!generic_refs.is_empty());
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

        let refs = TypeReferenceTracker::build(source);

        let parser_refs = find_type_references(&refs, "RustParser");
        // Should find: struct field, parameter in new(), return type in get_parser()
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
