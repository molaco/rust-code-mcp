//! Analysis tools module
//!
//! This module provides MCP tools for analyzing Rust codebases.
//!
//! ## Tools
//! - `analyze_complexity`: Calculate code metrics (LOC, cyclomatic complexity)
//! - `find_references`: Find all references to a symbol
//! - `find_definition`: Find where a symbol is defined
//! - `get_call_graph`: Get function call relationships
//! - `get_dependencies`: Get import dependencies

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use std::fs;
use std::path::Path;
use tracing;

use crate::parser::RustParser;

/// Helper function to recursively visit Rust files
fn visit_rust_files<F>(dir: &Path, visitor: &mut F) -> Result<(), String>
where
    F: FnMut(&Path) -> Result<(), String>,
{
    for entry in fs::read_dir(dir).map_err(|e| format!("Directory read error: {}", e))? {
        let entry = entry.map_err(|e| format!("Entry read error: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            visit_rust_files(&path, visitor)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            visitor(&path)?;
        }
    }
    Ok(())
}

/// Find the definition of a symbol in Rust code
pub async fn find_definition(
    symbol_name: &str,
    directory: &str,
) -> Result<CallToolResult, McpError> {
    let dir_path = Path::new(directory);
    if !dir_path.is_dir() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a directory", directory),
            None,
        ));
    }

    tracing::debug!("Searching for definition of '{}'", symbol_name);

    let mut found_definitions = Vec::new();
    let mut parser = RustParser::new().map_err(|e| {
        McpError::invalid_params(format!("Parser initialization error: {}", e), None)
    })?;

    // Recursively search .rs files
    let mut visitor = |path: &Path| -> Result<(), String> {
        if let Ok(symbols) = parser.parse_file(path) {
            for symbol in symbols {
                if symbol.name == symbol_name {
                    found_definitions.push((
                        path.to_path_buf(),
                        symbol.range.start_line,
                        symbol.kind.as_str().to_string(),
                    ));
                }
            }
        }
        Ok(())
    };

    visit_rust_files(dir_path, &mut visitor).map_err(|e| McpError::invalid_params(e, None))?;

    if found_definitions.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "No definition found for symbol '{}'",
            symbol_name
        ))]))
    } else {
        let mut result = format!(
            "Found {} definition(s) for '{}':\n",
            found_definitions.len(),
            symbol_name
        );
        for (path, line, kind) in found_definitions {
            result.push_str(&format!("- {}:{} ({})\n", path.display(), line, kind));
        }
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }
}

/// Find all references to a symbol in the codebase
pub async fn find_references(
    symbol_name: &str,
    directory: &str,
) -> Result<CallToolResult, McpError> {
    let dir_path = Path::new(directory);
    if !dir_path.is_dir() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a directory", directory),
            None,
        ));
    }

    tracing::debug!("Searching for references to '{}'", symbol_name);

    let mut found_call_refs = Vec::new();
    let mut found_type_refs = Vec::new();
    let mut parser = RustParser::new().map_err(|e| {
        McpError::invalid_params(format!("Parser initialization error: {}", e), None)
    })?;

    // Recursively search .rs files for references (both function calls and type usage)
    let mut visitor = |path: &Path| -> Result<(), String> {
        if let Ok(parse_result) = parser.parse_file_complete(path) {
            // Check function call references
            let callers = parse_result.call_graph.get_callers(symbol_name);
            if !callers.is_empty() {
                found_call_refs.push((
                    path.to_path_buf(),
                    callers.into_iter().map(String::from).collect::<Vec<String>>(),
                ));
            }

            // Check type references
            let type_usages: Vec<String> = parse_result
                .type_references
                .iter()
                .filter(|r| r.type_name == symbol_name)
                .map(|r| match &r.usage_context {
                    crate::parser::TypeUsageContext::FunctionParameter {
                        function_name,
                    } => {
                        format!("parameter in {}", function_name)
                    }
                    crate::parser::TypeUsageContext::FunctionReturn {
                        function_name,
                    } => {
                        format!("return type of {}", function_name)
                    }
                    crate::parser::TypeUsageContext::StructField {
                        struct_name,
                        field_name,
                    } => {
                        format!("field '{}' in struct {}", field_name, struct_name)
                    }
                    crate::parser::TypeUsageContext::ImplBlock { trait_name } => {
                        if let Some(trait_name) = trait_name {
                            format!("impl {} for type", trait_name)
                        } else {
                            "impl block".to_string()
                        }
                    }
                    crate::parser::TypeUsageContext::LetBinding => {
                        "let binding".to_string()
                    }
                    crate::parser::TypeUsageContext::GenericArgument => {
                        "generic type argument".to_string()
                    }
                })
                .collect();

            if !type_usages.is_empty() {
                found_type_refs.push((path.to_path_buf(), type_usages));
            }
        }
        Ok(())
    };

    visit_rust_files(dir_path, &mut visitor).map_err(|e| McpError::invalid_params(e, None))?;

    if found_call_refs.is_empty() && found_type_refs.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "No references found for symbol '{}'",
            symbol_name
        ))]))
    } else {
        let total_call_refs: usize = found_call_refs
            .iter()
            .map(|(_, callers)| callers.len())
            .sum();
        let total_type_refs: usize =
            found_type_refs.iter().map(|(_, usages)| usages.len()).sum();
        let total_refs = total_call_refs + total_type_refs;
        let total_files = found_call_refs
            .iter()
            .map(|(p, _)| p)
            .chain(found_type_refs.iter().map(|(p, _)| p))
            .collect::<std::collections::HashSet<_>>()
            .len();

        let mut result = format!(
            "Found {} reference(s) to '{}' in {} file(s):\n\n",
            total_refs, symbol_name, total_files
        );

        // Show function call references
        if !found_call_refs.is_empty() {
            result.push_str(&format!(
                "Function Calls ({} references):\n",
                total_call_refs
            ));
            for (path, callers) in found_call_refs {
                // Use relative path if possible
                let display_path: String = path.strip_prefix(dir_path)
                    .map(|p: &std::path::Path| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.to_string_lossy().into_owned());
                result.push_str(&format!(
                    "- {} (called by: {})\n",
                    display_path,
                    callers.join(", ")
                ));
            }
            result.push('\n');
        }

        // Show type usage references
        if !found_type_refs.is_empty() {
            result.push_str(&format!("Type Usage ({} references):\n", total_type_refs));
            for (path, usages) in found_type_refs {
                // Use relative path if possible
                let display_path: String = path.strip_prefix(dir_path)
                    .map(|p: &std::path::Path| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.to_string_lossy().into_owned());
                result.push_str(&format!("- {} ({})\n", display_path, usages.join(", ")));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }
}

/// Get dependencies for a file (imports and files that depend on it)
pub async fn get_dependencies(file_path: &str) -> Result<CallToolResult, McpError> {
    let file_path_obj = Path::new(file_path);

    if !file_path_obj.exists() {
        return Err(McpError::invalid_params(
            format!("File '{}' does not exist", file_path),
            None,
        ));
    }

    if !file_path_obj.is_file() {
        return Err(McpError::invalid_params(
            format!("'{}' is not a file", file_path),
            None,
        ));
    }

    let mut parser = RustParser::new().map_err(|e| {
        McpError::invalid_params(format!("Parser initialization error: {}", e), None)
    })?;
    let parse_result = parser
        .parse_file_complete(file_path_obj)
        .map_err(|e| McpError::invalid_params(format!("Parse error: {}", e), None))?;

    let mut result = format!("Dependencies for '{}':\n\n", file_path_obj.display());

    // List imports
    if parse_result.imports.is_empty() {
        result.push_str("No imports found.\n");
    } else {
        result.push_str(&format!("Imports ({}):\n", parse_result.imports.len()));
        for import in &parse_result.imports {
            result.push_str(&format!("- {}\n", import.path));
        }
    }

    Ok(CallToolResult::success(vec![Content::text(result)]))
}

/// Get call graph for a file or specific symbol
pub async fn get_call_graph(
    file_path: &str,
    symbol_name: Option<&str>,
) -> Result<CallToolResult, McpError> {
    let file_path_obj = Path::new(file_path);

    if !file_path_obj.exists() {
        return Err(McpError::invalid_params(
            format!("File '{}' does not exist", file_path),
            None,
        ));
    }

    if !file_path_obj.is_file() {
        return Err(McpError::invalid_params(
            format!("'{}' is not a file", file_path),
            None,
        ));
    }

    let mut parser = RustParser::new().map_err(|e| {
        McpError::invalid_params(format!("Parser initialization error: {}", e), None)
    })?;
    let parse_result = parser
        .parse_file_complete(file_path_obj)
        .map_err(|e| McpError::invalid_params(format!("Parse error: {}", e), None))?;

    let mut result = format!("Call graph for '{}':\n\n", file_path_obj.display());

    if let Some(symbol_name) = symbol_name {
        // Show call graph for specific symbol
        let callees = parse_result.call_graph.get_callees(symbol_name);
        let callers = parse_result.call_graph.get_callers(symbol_name);

        result.push_str(&format!("Symbol: {}\n\n", symbol_name));

        if callees.is_empty() && callers.is_empty() {
            result.push_str("No call relationships found.\n");
        } else {
            if !callees.is_empty() {
                result.push_str(&format!("Calls ({}):\n", callees.len()));
                for callee in callees {
                    result.push_str(&format!("  → {}\n", callee));
                }
            }

            if !callers.is_empty() {
                result.push_str(&format!("\nCalled by ({}):\n", callers.len()));
                for caller in callers {
                    result.push_str(&format!("  ← {}\n", caller));
                }
            }
        }
    } else {
        // Show entire call graph for file
        let all_functions = parse_result.call_graph.all_functions();
        let edge_count = parse_result.call_graph.edge_count();

        result.push_str(&format!("Functions: {}\n", all_functions.len()));
        result.push_str(&format!("Call relationships: {}\n\n", edge_count));

        if edge_count == 0 {
            result.push_str("No function calls found.\n");
        } else {
            result.push_str("Call relationships:\n");
            for function in all_functions {
                let callees = parse_result.call_graph.get_callees(function);
                if !callees.is_empty() {
                    result.push_str(&format!("{} → [{}]\n", function, callees.join(", ")));
                }
            }
        }
    }

    Ok(CallToolResult::success(vec![Content::text(result)]))
}

/// Analyze code complexity metrics for a file
pub async fn analyze_complexity(file_path: &str) -> Result<CallToolResult, McpError> {
    let file_path_obj = Path::new(file_path);

    if !file_path_obj.exists() {
        return Err(McpError::invalid_params(
            format!("File '{}' does not exist", file_path),
            None,
        ));
    }

    if !file_path_obj.is_file() {
        return Err(McpError::invalid_params(
            format!("'{}' is not a file", file_path),
            None,
        ));
    }

    // Read source file
    let source = fs::read_to_string(file_path_obj)
        .map_err(|e| McpError::invalid_params(format!("Failed to read file: {}", e), None))?;

    // Parse file to get symbols
    let mut parser = RustParser::new().map_err(|e| {
        McpError::invalid_params(format!("Parser initialization error: {}", e), None)
    })?;
    let parse_result = parser
        .parse_file_complete(file_path_obj)
        .map_err(|e| McpError::invalid_params(format!("Parse error: {}", e), None))?;

    // Calculate metrics
    let lines_of_code = source.lines().count();
    let non_empty_loc = source.lines().filter(|l| !l.trim().is_empty()).count();
    let comment_lines = source
        .lines()
        .filter(|l| l.trim().starts_with("//"))
        .count();
    let function_count = parse_result
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, crate::parser::SymbolKind::Function { .. }))
        .count();
    let struct_count = parse_result
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, crate::parser::SymbolKind::Struct))
        .count();
    let trait_count = parse_result
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, crate::parser::SymbolKind::Trait))
        .count();

    // Calculate cyclomatic complexity (simplified - count decision points)
    let complexity_keywords = ["if", "else if", "while", "for", "match", "&&", "||"];
    let cyclomatic_complexity: usize = source
        .lines()
        .map(|line| {
            complexity_keywords
                .iter()
                .map(|kw| line.matches(kw).count())
                .sum::<usize>()
        })
        .sum();

    // Calculate average function complexity
    let avg_complexity = if function_count > 0 {
        cyclomatic_complexity as f64 / function_count as f64
    } else {
        0.0
    };

    let mut result = format!("Complexity analysis for '{}':\n\n", file_path_obj.display());
    result.push_str("=== Code Metrics ===\n");
    result.push_str(&format!("Total lines:           {}\n", lines_of_code));
    result.push_str(&format!("Non-empty lines:       {}\n", non_empty_loc));
    result.push_str(&format!("Comment lines:         {}\n", comment_lines));
    result.push_str(&format!(
        "Code lines (approx):   {}\n\n",
        non_empty_loc - comment_lines
    ));

    result.push_str("=== Symbol Counts ===\n");
    result.push_str(&format!("Functions:             {}\n", function_count));
    result.push_str(&format!("Structs:               {}\n", struct_count));
    result.push_str(&format!("Traits:                {}\n\n", trait_count));

    result.push_str("=== Complexity ===\n");
    result.push_str(&format!(
        "Total cyclomatic:      {}\n",
        cyclomatic_complexity
    ));
    result.push_str(&format!("Avg per function:      {:.2}\n", avg_complexity));

    // Add call graph complexity
    let edge_count = parse_result.call_graph.edge_count();
    result.push_str(&format!("Function calls:        {}\n", edge_count));

    Ok(CallToolResult::success(vec![Content::text(result)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_find_definition_invalid_directory() {
        let result = find_definition("test", "/nonexistent/directory").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_find_references_invalid_directory() {
        let result = find_references("test", "/nonexistent/directory").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_dependencies_nonexistent_file() {
        let result = get_dependencies("/nonexistent/file.rs").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_call_graph_nonexistent_file() {
        let result = get_call_graph("/nonexistent/file.rs", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_analyze_complexity_nonexistent_file() {
        let result = analyze_complexity("/nonexistent/file.rs").await;
        assert!(result.is_err());
    }
}
