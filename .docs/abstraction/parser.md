# parser — Abstract Logic

## Module: mod.rs
**Purpose:** Parses Rust source files into structured symbols and a complete `ParseResult` bundle (symbols + call graph + imports + type references).

1. **Construct a parser bound to a Rust edition** -> `RustParser::new()`, `RustParser::with_edition()`
2. **Read a file and dispatch to source-level parsing** -> `RustParser::parse_file()`, `RustParser::parse_file_complete()`
3. **Parse source text into an AST and harvest just the symbol list** -> `RustParser::parse_source()`
4. **Parse source once and assemble the full ParseResult (symbols, calls, imports, type refs)** -> `RustParser::parse_source_complete()`
5. **Label each symbol kind with a stable string** -> `SymbolKind::as_str()`
6. **Walk the AST recursively, classifying items into typed `Symbol`s** -> `extract_symbols_recursive()`, `extract_item_symbols()`
7. **Derive shared per-symbol metadata (visibility, docstring, source range)** -> `extract_visibility()`, `extract_docstring()`, `node_to_range()`, `line_of_offset()`

## Module: call_graph.rs
**Purpose:** Builds and queries an in-memory caller→callee edge map extracted from a Rust AST.

1. **Create or seed a graph from raw source or an existing AST** -> `CallGraph::new()`, `CallGraph::build()`, `CallGraph::build_with_edition()`, `CallGraph::build_from_ast()`
2. **Extract outgoing call edges by walking each function/method body** -> `extract_calls_from_expr()`, `extract_call_target()`
3. **Mutate the graph by adding caller→callee edges** -> `CallGraph::add_call()`
4. **Query the graph for forward, reverse, and aggregate views** -> `CallGraph::get_callees()`, `CallGraph::get_callers()`, `CallGraph::all_functions()`, `CallGraph::edge_count()`, `CallGraph::has_call()`

## Module: imports.rs
**Purpose:** Extracts `use` declarations from Rust source into a flat list of `Import` records and summarizes external crate dependencies.

1. **Entry points that parse source (with optional edition) before extraction** -> `extract_imports()`, `extract_imports_with_edition()`
2. **Walk top-level `use` items in an existing AST** -> `extract_imports_from_ast()`
3. **Recursively flatten nested `use` trees, glob imports, and renames into `Import` records** -> `extract_use_tree()`
4. **Reduce imports to a deduplicated list of root crate names** -> `get_external_dependencies()`

## Module: type_references.rs
**Purpose:** Locates every type usage site in a Rust file and tags each with its syntactic context (function param/return, struct field, impl block, generic argument).

1. **Entry points that parse source (with optional edition) before extraction** -> `build_type_references()`, `build_type_references_with_edition()`
2. **Walk an AST's functions, structs, and impls to collect contextualized type references** -> `build_type_references_from_ast()`
3. **Recursively descend into a single type expression, recording path types and their generic arguments** -> `extract_types_from_type()`
4. **Compute 1-indexed source line numbers for byte offsets** -> `line_of_offset()`
5. **Query helpers for filtering and grouping the collected references** -> `find_type_references()`, `group_by_line()`
