//! Analysis tools module
//!
//! This module provides MCP tools for static analysis of Rust codebases using rust-analyzer.
//! It enables code understanding through symbol analysis, complexity metrics, and
//! relationship mapping.
//!
//! ## Overview
//!
//! The analysis tools provide deep code insights through:
//! - **Symbol Analysis**: Find definitions and references (functions, structs, traits)
//! - **Call Graph Analysis**: Map function call relationships
//! - **Dependency Analysis**: Track imports and module relationships
//! - **Complexity Metrics**: Calculate LOC, cyclomatic complexity, and function counts
//!
//! ## MCP Tools
//!
//! - [`find_definition`]: Locate where symbols (functions, structs, traits) are defined
//! - [`find_references`]: Find all usages of a symbol (calls + type references)
//! - [`get_call_graph`]: Analyze function call relationships (callers/callees)
//! - [`get_dependencies`]: List file imports and dependencies
//! - [`analyze_complexity`]: Calculate code complexity metrics
//!
//! ## Rust-Analyzer Integration
//!
//! All analysis tools use rust-analyzer for accurate Rust parsing:
//! - AST-based symbol extraction (not regex)
//! - Full Rust syntax support (2021 edition)
//! - Fast incremental parsing
//!
//! ## Examples
//!
//! ### Find Symbol Definition
//! ```rust,no_run
//! use file_search_mcp::tools::analysis_tools::find_definition;
//!
//! # async fn example() -> Result<(), rmcp::ErrorData> {
//! // Find where a function is defined
//! let result = find_definition(
//!     "parse_tokens",
//!     "/path/to/rust/project"
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Analyze Code Complexity
//! ```rust,no_run
//! use file_search_mcp::tools::analysis_tools::analyze_complexity;
//!
//! # async fn example() -> Result<(), rmcp::ErrorData> {
//! // Get complexity metrics for a file
//! let metrics = analyze_complexity(
//!     "/path/to/rust/project/src/main.rs"
//! ).await?;
//! // Returns: LOC, cyclomatic complexity, function count, etc.
//! # Ok(())
//! # }
//! ```
//!
//! ### Get Call Graph
//! ```rust,no_run
//! use file_search_mcp::tools::analysis_tools::get_call_graph;
//!
//! # async fn example() -> Result<(), rmcp::ErrorData> {
//! // Get call graph for a specific function
//! let graph = get_call_graph(
//!     "/path/to/file.rs",
//!     Some("process_data")  // Optional: specific symbol
//! ).await?;
//! // Shows what it calls and what calls it
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! This module is part of the refactored tools layer (Phase 1 refactoring).
//! It uses:
//! - `RustParser` for rust-analyzer-based AST parsing
//! - `CallGraph` for tracking function relationships
//! - `TypeReference` for type usage analysis

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use std::fs;
use std::path::Path;
use tracing;

use crate::parser::RustParser;

use crate::semantic::SEMANTIC;

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

/// Find the definition of a symbol by name
pub async fn find_definition(
    symbol_name: &str,
    directory: &str,
) -> Result<CallToolResult, McpError> {
    use std::path::Path;

    let project_path = Path::new(directory);

    tracing::debug!("Searching for definition of '{}'", symbol_name);

    let locations = SEMANTIC
        .lock()
        .map_err(|e| McpError::internal_error(format!("Failed to acquire lock: {}", e), None))?
        .symbol_search(project_path, symbol_name, 50)
        .map_err(|e| McpError::internal_error(format!("Symbol search failed: {}", e), None))?;

    if locations.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "No definition found for symbol '{}'",
            symbol_name
        ))]))
    } else {
        let result = locations
            .iter()
            .map(|loc| loc.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Found {} definition(s) for '{}':\n{}",
            locations.len(),
            symbol_name,
            result
        ))]))
    }
}

/// Find all references to a symbol by name
pub async fn find_references(
    symbol_name: &str,
    directory: &str,
) -> Result<CallToolResult, McpError> {
    use std::path::Path;

    let project_path = Path::new(directory);

    tracing::debug!("Searching for references to '{}'", symbol_name);

    let locations = SEMANTIC
        .lock()
        .map_err(|e| McpError::internal_error(format!("Failed to acquire lock: {}", e), None))?
        .find_references_by_name(project_path, symbol_name)
        .map_err(|e| McpError::internal_error(format!("Find references failed: {}", e), None))?;

    if locations.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "No references found for symbol '{}'",
            symbol_name
        ))]))
    } else {
        let result = locations
            .iter()
            .map(|loc| loc.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Found {} reference(s) for '{}':\n{}",
            locations.len(),
            symbol_name,
            result
        ))]))
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
    async fn test_find_definition_invalid_project() {
        // /tmp is not a valid Cargo project, should return an error
        let result = find_definition("nonexistent_symbol_xyz", "/tmp").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_find_references_invalid_project() {
        // /tmp is not a valid Cargo project, should return an error
        let result = find_references("nonexistent_symbol_xyz", "/tmp").await;
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
