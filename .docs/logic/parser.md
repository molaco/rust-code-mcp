# parser — Detailed Logic

## Module: mod.rs

### `SymbolKind::as_str(&self) -> &str`
**Call graph:** (none — pure pattern match)
**Steps:**
1. Match on `self` and return a static string literal corresponding to the variant (e.g., `Function {..}` -> `"function"`).
2. Cover all variants: Function, Struct, Enum, Trait, Impl, Module, Const, Static, TypeAlias.

### `RustParser::new() -> Result<Self, Box<dyn Error>>`
**Call graph:** (none — constructor)
**Steps:**
1. Construct a `RustParser` with `edition` set to `Edition::Edition2021`.
2. Wrap in `Ok(...)` to honor the fallible signature (kept for future-proofing).

### `RustParser::with_edition(edition: Edition) -> Self`
**Call graph:** (none — constructor)
**Steps:**
1. Build a `RustParser` directly using the supplied `edition`.

### `RustParser::parse_file(&mut self, path: &Path) -> Result<Vec<Symbol>, Box<dyn Error>>`
**Call graph:** parse_file -> fs::read_to_string, parse_source
**Steps:**
1. Read the file contents at `path` to a `String` via `fs::read_to_string`.
2. Delegate to `parse_source` with the loaded source text.
3. Propagate any IO or parse error via `?`.

### `RustParser::parse_source(&mut self, source: &str) -> Result<Vec<Symbol>, Box<dyn Error>>`
**Call graph:** parse_source -> SourceFile::parse, parse.tree, extract_symbols_recursive
**Steps:**
1. Parse the source string with `SourceFile::parse(source, self.edition)`.
2. Obtain the AST tree via `parse.tree()`.
3. Initialize an empty `Vec<Symbol>` accumulator.
4. Call `extract_symbols_recursive(&file, source, &mut symbols)` to populate it.
5. Return the symbols wrapped in `Ok`.

### `RustParser::parse_file_complete(&mut self, path: &Path) -> Result<ParseResult, Box<dyn Error>>`
**Call graph:** parse_file_complete -> fs::read_to_string, parse_source_complete
**Steps:**
1. Read the file at `path` to a `String`.
2. Delegate to `parse_source_complete` with the source text.

### `RustParser::parse_source_complete(&mut self, source: &str) -> Result<ParseResult, Box<dyn Error>>`
**Call graph:** parse_source_complete -> SourceFile::parse, parse.tree, extract_symbols_recursive, CallGraph::build_from_ast, extract_imports_from_ast, type_references::build_type_references_from_ast
**Steps:**
1. Parse the source once with `SourceFile::parse(source, self.edition)` and grab the AST tree.
2. Initialize an empty `Vec<Symbol>` and call `extract_symbols_recursive` to populate it.
3. Build the call graph by passing the same AST to `CallGraph::build_from_ast`.
4. Extract imports from the AST via `extract_imports_from_ast`.
5. Extract type references via `type_references::build_type_references_from_ast` (needs `source` for line lookups).
6. Bundle all four collections into a `ParseResult` and return wrapped in `Ok`.

### `line_of_offset(source: &str, offset: usize) -> usize` (private helper)
**Call graph:** (none — string iteration)
**Steps:**
1. Slice `source` from byte 0 up to `offset.min(source.len())` to avoid OOB.
2. Iterate characters and count occurrences of `'\n'`.
3. Add 1 to convert to a 1-indexed line number.

### `extract_visibility(vis: Option<ast::Visibility>) -> Visibility` (private helper)
**Call graph:** (none — string match)
**Steps:**
1. If `vis` is `None`, return `Visibility::Private`.
2. Otherwise stringify the visibility node text.
3. If text equals `"pub"` return `Public`; if it starts with `"pub(crate)"` return `Crate`.
4. If it starts with `"pub("` (other restriction), return `Restricted(text)`.
5. Fallback returns `Public`.

### `extract_docstring<N: HasDocComments>(node: &N) -> Option<String>` (private helper)
**Call graph:** (none — iterator chain on `doc_comments`)
**Steps:**
1. Iterate `node.doc_comments()` to collect their textual content.
2. For each comment, strip a `///` or `//!` prefix, then trim whitespace.
3. Collect into `Vec<String>`; return `None` if empty.
4. Otherwise join them with newlines and wrap in `Some`.

### `node_to_range(node: &dyn AstNode, source: &str) -> Range` (private helper)
**Call graph:** node_to_range -> AstNode::syntax, line_of_offset
**Steps:**
1. Read the node's `text_range()` to obtain start/end byte offsets.
2. Convert each offset to a 1-indexed line via `line_of_offset`.
3. Return a `Range` carrying both line numbers and byte offsets.

### `extract_symbols_recursive(file: &SourceFile, source: &str, symbols: &mut Vec<Symbol>)` (private)
**Call graph:** extract_symbols_recursive -> file.items, extract_item_symbols
**Steps:**
1. Iterate over top-level items of the file.
2. For each item, delegate symbol extraction to `extract_item_symbols`.

### `extract_item_symbols(item: &ast::Item, source: &str, symbols: &mut Vec<Symbol>)` (private)
**Call graph:** extract_item_symbols -> node_to_range, extract_visibility, extract_docstring, extract_item_symbols (recursion)
**Steps:**
1. Match on the item variant (Fn, Struct, Enum, Trait, Impl, Module, Const, Static, TypeAlias).
2. For Fn: capture name and async/unsafe/const tokens to build `SymbolKind::Function {..}`.
3. For Struct/Enum/Trait/Const/Static/TypeAlias: capture the name and emit a `Symbol` with the corresponding kind.
4. For Impl: stringify `self_ty()` and optional `trait_()`, push an `Impl` symbol named `"impl Trait for Type"` (or `"impl Type"`), then iterate `assoc_item_list().assoc_items()` and emit each method as a `Function` symbol.
5. For Module: emit a `Module` symbol, then recurse into `item_list().items()` calling `extract_item_symbols` on each inner item.
6. Other item kinds are ignored via the `_ => {}` arm.

---

## Module: call_graph.rs

### `CallGraph::new() -> Self`
**Call graph:** (none — constructor)
**Steps:**
1. Initialize `edges` as an empty `HashMap<String, HashSet<String>>`.

### `CallGraph::build(source: &str) -> Self`
**Call graph:** build -> build_with_edition
**Steps:**
1. Forward to `build_with_edition` with `Edition::Edition2021` as the default edition.

### `CallGraph::build_with_edition(source: &str, edition: Edition) -> Self`
**Call graph:** build_with_edition -> SourceFile::parse, parse.tree, build_from_ast
**Steps:**
1. Parse the source with the supplied edition via `SourceFile::parse`.
2. Acquire the syntax tree.
3. Delegate to `build_from_ast` to construct the graph.

### `CallGraph::build_from_ast(file: &SourceFile) -> Self`
**Call graph:** build_from_ast -> CallGraph::new, file.items, extract_calls_from_expr
**Steps:**
1. Create a fresh empty graph.
2. Iterate top-level items in the file.
3. For each `Fn`, take its name as the caller, then if it has a body, call `extract_calls_from_expr` on the body's syntax to record outgoing edges.
4. For each `Impl`, walk associated items; for every `AssocItem::Fn` with a body, call `extract_calls_from_expr` using the method name as caller.
5. Return the populated graph.

### `CallGraph::add_call(&mut self, caller: String, callee: String)`
**Call graph:** add_call -> HashMap::entry, HashSet::insert
**Steps:**
1. Look up `caller` in `edges`, inserting a new empty `HashSet` if missing.
2. Insert `callee` into that set (deduplicating multiple calls).

### `CallGraph::get_callees(&self, caller: &str) -> Vec<&str>`
**Call graph:** (none — map/iter)
**Steps:**
1. Look up `caller` in `edges`.
2. If found, map the set's `String`s to `&str` and collect into a `Vec`.
3. Otherwise return an empty vector.

### `CallGraph::get_callers(&self, callee: &str) -> Vec<&str>`
**Call graph:** (none — iter chain)
**Steps:**
1. Iterate every `(caller, callees)` pair in `edges`.
2. Keep only entries whose set contains `callee`.
3. Collect the caller names as `&str` into a `Vec`.

### `CallGraph::all_functions(&self) -> HashSet<&str>`
**Call graph:** (none — iter)
**Steps:**
1. Initialize an empty `HashSet`.
2. For each edge, insert the caller and every callee into the set.
3. Return the union set of all named functions appearing in the graph.

### `CallGraph::edge_count(&self) -> usize`
**Call graph:** (none — sum)
**Steps:**
1. Iterate over the `HashSet` values in `edges`.
2. Sum each set's `len()` and return the total.

### `CallGraph::has_call(&self, caller: &str, callee: &str) -> bool`
**Call graph:** (none — map/set lookup)
**Steps:**
1. Look up `caller` in `edges`.
2. If found, check whether the set contains `callee`; otherwise return `false`.

### `extract_calls_from_expr(node, caller, graph)` (private)
**Call graph:** extract_calls_from_expr -> node.descendants, ast::CallExpr::cast, extract_call_target, ast::MethodCallExpr::cast, CallGraph::add_call
**Steps:**
1. Walk every descendant syntax node of `node`.
2. If a descendant is `CALL_EXPR`, cast to `ast::CallExpr` and resolve via `extract_call_target`; on success, add an edge `caller -> callee`.
3. If a descendant is `METHOD_CALL_EXPR`, cast to `ast::MethodCallExpr` and use its `name_ref()` text as the callee, adding an edge `caller -> method_name`.

### `extract_call_target(call: &ast::CallExpr) -> Option<String>` (private)
**Call graph:** (none — AST walk)
**Steps:**
1. Get the call's `expr()` (the callee expression).
2. Match on it; only handle `Expr::PathExpr`.
3. Walk to the path's segments and take the last one.
4. Read its `name_ref()` text; return as `Some(String)` or `None` if any step fails.

---

## Module: imports.rs

### `extract_imports(source: &str) -> Vec<Import>`
**Call graph:** extract_imports -> extract_imports_with_edition
**Steps:**
1. Forward to `extract_imports_with_edition` using `Edition::Edition2021`.

### `extract_imports_with_edition(source: &str, edition: Edition) -> Vec<Import>`
**Call graph:** extract_imports_with_edition -> SourceFile::parse, parse.tree, extract_imports_from_ast
**Steps:**
1. Parse the source string with the requested edition.
2. Take the AST tree and delegate to `extract_imports_from_ast`.

### `extract_imports_from_ast(file: &SourceFile) -> Vec<Import>`
**Call graph:** extract_imports_from_ast -> file.items, extract_use_tree
**Steps:**
1. Initialize an empty `Vec<Import>`.
2. Iterate top-level items, matching `ast::Item::Use(use_item)`.
3. For each use item, call `extract_use_tree(use_item.use_tree(), &mut imports, "".to_string())` to recurse.
4. Return the accumulated import list.

### `extract_use_tree(use_tree, imports, prefix)` (private, recursive)
**Call graph:** extract_use_tree -> tree.path, tree.star_token, tree.use_tree_list, tree.rename, extract_use_tree (recursion)
**Steps:**
1. Bail if `use_tree` is `None`.
2. Stringify the tree's `path()`; if a non-empty `prefix` exists, join with `"::"`.
3. If the tree has a `star_token`, push an `Import { is_glob: true }` and return.
4. If the tree has a `use_tree_list` (e.g., `{a, b}`), recurse into each subtree with the current `path` as the new prefix and return.
5. Otherwise, capture an optional `rename()` (`as Foo`) name.
6. If the resolved path is non-empty, push an `Import { path, is_glob: false, items: rename.map(|r| vec![r]).unwrap_or_default() }`.

### `get_external_dependencies(imports: &[Import]) -> Vec<String>`
**Call graph:** (none — iter chain through HashSet)
**Steps:**
1. For each import, split its path on `"::"` and take the first component.
2. Collect components into a `HashSet<String>` to deduplicate.
3. Convert the set into a `Vec<String>` for return.

---

## Module: type_references.rs

### `build_type_references(source: &str) -> Vec<TypeReference>` (crate-visible)
**Call graph:** build_type_references -> build_type_references_with_edition
**Steps:**
1. Forward to `build_type_references_with_edition` using `Edition::Edition2021`.

### `build_type_references_with_edition(source: &str, edition: Edition) -> Vec<TypeReference>` (crate-visible)
**Call graph:** build_type_references_with_edition -> SourceFile::parse, parse.tree, build_type_references_from_ast
**Steps:**
1. Parse the source with the supplied edition.
2. Delegate to `build_type_references_from_ast` with the AST and source string.

### `build_type_references_from_ast(file: &SourceFile, source: &str) -> Vec<TypeReference>` (crate-visible)
**Call graph:** build_type_references_from_ast -> file.items, extract_types_from_type, line_of_offset
**Steps:**
1. Initialize an empty `Vec<TypeReference>`.
2. Iterate top-level items, matching on `Fn`, `Struct`, and `Impl`.
3. For `Fn`: read the function's name, then walk parameters and (if present) return type, calling `extract_types_from_type` with `FunctionParameter` or `FunctionReturn` context.
4. For `Struct`: read the struct's name, then iterate its `field_list`; for `RecordFieldList` use each field's name, for `TupleFieldList` use the index as the field name, then call `extract_types_from_type` with `StructField` context.
5. For `Impl`: stringify `self_ty()` and optional `trait_()`; push a synthetic `TypeReference` with `ImplBlock { trait_name }` context for the impl target; then walk the impl's associated functions and extract parameter and return types just like `Fn`.
6. Return the accumulated references.

### `line_of_offset(source: &str, offset: usize) -> usize` (private helper)
**Call graph:** (none — string iteration)
**Steps:**
1. Slice `source[..offset.min(source.len())]`.
2. Count `'\n'` occurrences and add 1 for a 1-indexed line.

### `extract_types_from_type(ty, source, refs, context)` (private, recursive)
**Call graph:** extract_types_from_type -> path_ty.path, path.segments, segment.name_ref, segment.generic_arg_list, ref_ty.ty, extract_types_from_type (recursion), line_of_offset
**Steps:**
1. Match on the `ast::Type` variant.
2. For `PathType`: take the last path segment; if it has a `name_ref`, push a `TypeReference` with that name, the supplied `context`, and computed line number.
3. Still in `PathType`: if the segment carries a `generic_arg_list`, iterate generic arguments; for each `GenericArg::TypeArg(type_arg)` recurse on the inner type with `TypeUsageContext::GenericArgument`.
4. For `RefType`: unwrap the inner type and recurse with the same context (preserving the original usage site).
5. Other type variants are ignored via the `_ => {}` arm.

### `find_type_references<'a>(references: &'a [TypeReference], type_name: &str) -> Vec<&'a TypeReference>`
**Call graph:** (none — iter filter)
**Steps:**
1. Iterate the references slice.
2. Keep only those whose `type_name` equals the supplied `type_name`.
3. Collect borrowed references into a `Vec`.

### `group_by_line(references: &[TypeReference]) -> HashMap<usize, Vec<&TypeReference>>`
**Call graph:** (none — iter into HashMap)
**Steps:**
1. Initialize an empty `HashMap<usize, Vec<&TypeReference>>`.
2. For each reference, insert (or extend) an entry keyed by `reference.line` with the reference appended to its bucket.
3. Return the grouped map.
