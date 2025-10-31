//! Builder for constructing hypergraphs from parsed Rust code

use crate::hypergraph::{
    Hypergraph, HyperNode, HyperedgeType, NodeId, NodeType, Result,
};
use crate::parser::{CallGraph, Import, RustParser, Symbol, TypeReference, TypeUsageContext, SymbolKind, Range, Visibility};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Configuration for hypergraph construction
#[derive(Debug, Clone)]
pub struct HypergraphConfig {
    /// If true, only include symbol nodes (functions, structs, traits, enums)
    /// If false, include file nodes and module relationships
    pub symbol_only: bool,
}

impl Default for HypergraphConfig {
    fn default() -> Self {
        Self {
            symbol_only: true, // Default to symbol-only view
        }
    }
}

/// Builder for constructing hypergraphs from Rust source code
pub struct HypergraphBuilder {
    /// Underlying hypergraph being built
    hypergraph: Hypergraph,

    /// Parser instance
    parser: RustParser,

    /// Map: symbol name → NodeId (for quick lookups)
    symbol_index: HashMap<String, NodeId>,

    /// Map: file path → file NodeId
    file_index: HashMap<PathBuf, NodeId>,

    /// Configuration
    config: HypergraphConfig,
}

impl HypergraphBuilder {
    /// Creates a new builder with default config (symbol-only mode)
    pub fn new() -> Result<Self> {
        Self::with_config(HypergraphConfig::default())
    }

    /// Creates a new builder with custom configuration
    pub fn with_config(config: HypergraphConfig) -> Result<Self> {
        Ok(Self {
            hypergraph: Hypergraph::new(),
            parser: RustParser::new()
                .map_err(|e| crate::hypergraph::HypergraphError::NodeNameExists(
                    format!("Parser error: {}", e)
                ))?,
            symbol_index: HashMap::new(),
            file_index: HashMap::new(),
            config,
        })
    }

    /// Builds a hypergraph from a directory of Rust files
    ///
    /// # Arguments
    /// * `dir` - Root directory to scan for .rs files
    ///
    /// # Returns
    /// Complete hypergraph with all nodes and edges
    pub fn build_from_directory(mut self, dir: &Path) -> Result<Hypergraph> {
        println!("Building hypergraph from: {:?}", dir);

        // Step 1: Collect all Rust files
        let rust_files = self.collect_rust_files(dir)?;
        println!("Found {} Rust files", rust_files.len());

        // Step 2: First pass - create all nodes (files + symbols)
        for file in &rust_files {
            self.process_file_nodes(file)?;
        }
        println!("Created {} nodes", self.hypergraph.count_nodes());

        // Step 3: Second pass - create edges (relationships)
        for file in &rust_files {
            self.process_file_edges(file)?;
        }
        println!("Created {} hyperedges", self.hypergraph.count_hyperedges());

        Ok(self.hypergraph)
    }

    /// Collects all .rs files in directory (excluding target/, .git/, etc.)
    fn collect_rust_files(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        for entry in WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !self.is_excluded(e.path()))
        {
            let entry = entry.map_err(|e|
                crate::hypergraph::HypergraphError::NodeNameExists(
                    format!("Walk error: {}", e)
                ))?;

            if entry.path().extension().and_then(|s| s.to_str()) == Some("rs") {
                files.push(entry.path().to_path_buf());
            }
        }

        Ok(files)
    }

    /// Checks if path should be excluded from scanning
    fn is_excluded(&self, path: &Path) -> bool {
        let exclude_dirs = ["target", ".git", "node_modules", ".cargo"];

        path.components().any(|c| {
            if let Some(s) = c.as_os_str().to_str() {
                exclude_dirs.contains(&s)
            } else {
                false
            }
        })
    }

    /// First pass: Create file node and symbol nodes for a file
    fn process_file_nodes(&mut self, file_path: &Path) -> Result<()> {
        // Create file node (only if not in symbol-only mode)
        if !self.config.symbol_only {
            let file_node = HyperNode::from_file(file_path.to_path_buf());
            let file_node_id = self.hypergraph.add_node(file_node)?;
            self.file_index.insert(file_path.to_path_buf(), file_node_id);
        }

        // Parse file
        let parse_result = self.parser.parse_file_complete(file_path)
            .map_err(|e| crate::hypergraph::HypergraphError::NodeNameExists(
                format!("Parse error: {}", e)
            ))?;

        // Create symbol nodes
        for symbol in parse_result.symbols {
            let symbol_node = HyperNode::from_symbol(symbol.clone(), file_path.to_path_buf());

            // Add node (may fail if duplicate name exists)
            match self.hypergraph.add_node(symbol_node) {
                Ok(node_id) => {
                    // Index by simple name for lookups
                    self.symbol_index.insert(symbol.name.clone(), node_id);
                }
                Err(crate::hypergraph::HypergraphError::NodeNameExists(_)) => {
                    // Duplicate symbol name - skip (could be same name in different modules)
                    // TODO: Phase 4 - use qualified names
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    /// Second pass: Create edges for a file
    fn process_file_edges(&mut self, file_path: &Path) -> Result<()> {
        // Parse file again (TODO: cache parse results)
        let parse_result = self.parser.parse_file_complete(file_path)
            .map_err(|e| crate::hypergraph::HypergraphError::NodeNameExists(
                format!("Parse error: {}", e)
            ))?;

        // Create different types of edges based on mode
        if self.config.symbol_only {
            // Symbol-only mode: only create symbol-to-symbol edges
            self.build_call_pattern_edges(&parse_result.call_graph)?;
            self.build_type_edges(&parse_result.type_references)?;
        } else {
            // Full mode: include file-based edges
            let file_node_id = *self.file_index.get(file_path)
                .ok_or_else(|| crate::hypergraph::HypergraphError::NodeNameExists(
                    format!("File node not found: {:?}", file_path)
                ))?;

            self.build_module_containment_edges(file_node_id, &parse_result.symbols)?;
            self.build_call_pattern_edges(&parse_result.call_graph)?;
            self.build_import_cluster_edges(file_node_id, &parse_result.imports)?;
            self.build_type_edges(&parse_result.type_references)?;
        }

        Ok(())
    }

    /// Creates ModuleContainment edges: file → symbols
    ///
    /// Example: {src/parser.rs} → {RustParser, parse_file, Symbol}
    fn build_module_containment_edges(
        &mut self,
        file_node_id: NodeId,
        symbols: &[Symbol],
    ) -> Result<()> {
        use std::collections::HashSet;

        // Collect all symbol node IDs in this file
        let mut symbol_ids = HashSet::new();

        for symbol in symbols {
            if let Some(&node_id) = self.symbol_index.get(&symbol.name) {
                symbol_ids.insert(node_id);
            }
        }

        // Skip if no symbols (empty file)
        if symbol_ids.is_empty() {
            return Ok(());
        }

        // Create edge: {file} → {symbols}
        match self.hypergraph.add_hyperedge(
            [file_node_id].into_iter().collect(),
            symbol_ids,
            HyperedgeType::ModuleContainment,
            1.0,
        ) {
            Ok(_) => {},
            Err(crate::hypergraph::HypergraphError::HyperedgeAlreadyExists) => {
                // Edge already exists, skip
            },
            Err(e) => return Err(e),
        }

        Ok(())
    }

    /// Creates CallPattern edges: caller → callees
    ///
    /// Example: {main} → {parse_file, build_graph}
    fn build_call_pattern_edges(&mut self, call_graph: &CallGraph) -> Result<()> {
        use std::collections::HashSet;

        // Iterate over all callers
        for caller in call_graph.all_functions() {
            // Look up caller node
            let caller_id = match self.symbol_index.get(caller) {
                Some(&id) => id,
                None => continue,  // Caller not in hypergraph (external?)
            };

            // Collect callee node IDs
            let mut callee_ids = HashSet::new();
            for callee in call_graph.get_callees(caller) {
                if let Some(&callee_id) = self.symbol_index.get(callee) {
                    callee_ids.insert(callee_id);
                }
            }

            // Skip if no callees found
            if callee_ids.is_empty() {
                continue;
            }

            // Create edge: {caller} → {callees}
            // Note: This creates separate edges per caller (Decision 1: Option A)
            match self.hypergraph.add_hyperedge(
                [caller_id].into_iter().collect(),
                callee_ids,
                HyperedgeType::CallPattern,
                1.0,
            ) {
                Ok(_) => {},
                Err(crate::hypergraph::HypergraphError::HyperedgeAlreadyExists) => {
                    // Edge already exists, skip
                },
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    /// Creates ImportCluster edges: file → dependencies
    ///
    /// Example: {src/main.rs} → {std::collections, crate::parser}
    fn build_import_cluster_edges(
        &mut self,
        file_node_id: NodeId,
        imports: &[Import],
    ) -> Result<()> {
        use std::collections::HashSet;

        // For imports, we need to create nodes for external modules
        let mut import_nodes = HashSet::new();

        for import in imports {
            // Get or create node for this import path
            let import_name = import.path.clone();

            if let Some(&existing_id) = self.symbol_index.get(&import_name) {
                import_nodes.insert(existing_id);
            } else {
                // Create a virtual node for external import
                let import_node = HyperNode {
                    id: NodeId(0),
                    name: import_name.clone(),
                    file_path: PathBuf::from("<external>"),
                    line_start: 0,
                    line_end: 0,
                    node_type: NodeType::File {
                        path: PathBuf::from(format!("<import:{}>", import_name))
                    },
                };

                match self.hypergraph.add_node(import_node) {
                    Ok(node_id) => {
                        self.symbol_index.insert(import_name, node_id);
                        import_nodes.insert(node_id);
                    }
                    Err(crate::hypergraph::HypergraphError::NodeNameExists(_)) => {
                        // Already exists, get it
                        if let Some(&id) = self.symbol_index.get(&import.path) {
                            import_nodes.insert(id);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // Skip if no imports
        if import_nodes.is_empty() {
            return Ok(());
        }

        // Create edge: {file} → {imports}
        match self.hypergraph.add_hyperedge(
            [file_node_id].into_iter().collect(),
            import_nodes,
            HyperedgeType::ImportCluster,
            1.0,
        ) {
            Ok(_) => {},
            Err(crate::hypergraph::HypergraphError::HyperedgeAlreadyExists) => {
                // Edge already exists, skip
            },
            Err(e) => return Err(e),
        }

        Ok(())
    }

    /// Creates type-related edges (TraitImpl, TypeComposition)
    fn build_type_edges(&mut self, type_refs: &[TypeReference]) -> Result<()> {
        use std::collections::HashMap;
        use std::collections::HashSet;

        // Group type references by context
        let mut trait_impls: HashMap<String, HashSet<NodeId>> = HashMap::new();
        let mut struct_compositions: HashMap<String, HashSet<NodeId>> = HashMap::new();

        for type_ref in type_refs {
            match &type_ref.usage_context {
                TypeUsageContext::ImplBlock { trait_name: Some(trait_name) } => {
                    // This is a trait implementation: impl Trait for Type
                    // type_ref.type_name is the Type being implemented
                    if let Some(&type_id) = self.symbol_index.get(&type_ref.type_name) {
                        trait_impls.entry(trait_name.clone())
                            .or_default()
                            .insert(type_id);
                    }
                }

                TypeUsageContext::StructField { struct_name, .. } => {
                    // type_ref.type_name is the field type
                    // Find field type node (if it exists in our codebase)
                    if let Some(&field_type_id) = self.symbol_index.get(&type_ref.type_name) {
                        struct_compositions.entry(struct_name.clone())
                            .or_default()
                            .insert(field_type_id);
                    }
                }

                _ => {
                    // Other contexts - skip for Phase 2
                }
            }
        }

        // Create TraitImpl edges: {trait} → {implementors}
        for (trait_name, implementors) in trait_impls {
            if implementors.len() < 2 {
                continue;  // Need at least 2 nodes for hyperedge
            }

            // Find or create trait node
            let trait_id = if let Some(&id) = self.symbol_index.get(&trait_name) {
                id
            } else {
                // Create virtual trait node for external traits
                let trait_node = HyperNode {
                    id: NodeId(0),
                    name: trait_name.clone(),
                    file_path: PathBuf::from("<external>"),
                    line_start: 0,
                    line_end: 0,
                    node_type: NodeType::Symbol {
                        symbol: Symbol {
                            kind: SymbolKind::Trait,
                            name: trait_name.clone(),
                            range: Range {
                                start_line: 0,
                                end_line: 0,
                                start_byte: 0,
                                end_byte: 0,
                            },
                            docstring: None,
                            visibility: Visibility::Public,
                        }
                    },
                };

                let id = self.hypergraph.add_node(trait_node)?;
                self.symbol_index.insert(trait_name.clone(), id);
                id
            };

            // Create edge: {trait} → {implementors}
            match self.hypergraph.add_hyperedge(
                [trait_id].into_iter().collect(),
                implementors,
                HyperedgeType::TraitImpl { trait_name },
                1.0,
            ) {
                Ok(_) => {},
                Err(crate::hypergraph::HypergraphError::HyperedgeAlreadyExists) => {
                    // Edge already exists, skip
                },
                Err(e) => return Err(e),
            }
        }

        // Create TypeComposition edges: {struct} → {field_types}
        for (struct_name, field_types) in struct_compositions {
            if field_types.is_empty() {
                continue;
            }

            if let Some(&struct_id) = self.symbol_index.get(&struct_name) {
                match self.hypergraph.add_hyperedge(
                    [struct_id].into_iter().collect(),
                    field_types,
                    HyperedgeType::TypeComposition { struct_name },
                    1.0,
                ) {
                    Ok(_) => {},
                    Err(crate::hypergraph::HypergraphError::HyperedgeAlreadyExists) => {
                        // Edge already exists, skip
                    },
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(())
    }
}

impl Default for HypergraphBuilder {
    fn default() -> Self {
        Self::new().expect("Failed to create HypergraphBuilder")
    }
}
