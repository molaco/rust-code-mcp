# Future MCP Tools

Tool ideas for rust-code-mcp, designed to be agent-friendly (simple inputs, actionable outputs).

---

## Rust-Analyzer IDE Methods Available

These are methods from `ra_ap_ide` that can power new tools:

### High-Value Methods

| Method | What it does | MCP Tool Idea |
|--------|--------------|---------------|
| `goto_definition()` | Jump to where symbol is defined | find_definition (semantic) |
| `goto_implementation()` | Find trait implementations | find_implementations |
| `goto_type_definition()` | Jump to type of variable | find_type |
| `find_all_refs()` | All usages of a symbol | find_references (semantic) |
| `call_hierarchy()` | Incoming/outgoing calls | get_call_hierarchy |
| `hover()` | Type info, docs, signature | get_symbol_info |
| `signature_help()` | Function signature at call site | get_signature |
| `symbol_search()` | Fuzzy search across workspace | search_symbols |
| `inlay_hints()` | Type annotations, param names | get_inlay_hints |

### Code Understanding Methods

| Method | MCP Tool Idea |
|--------|---------------|
| `view_hir()` | explain_code - show HIR representation |
| `view_mir()` | show_mir - mid-level IR |
| `expand_macro()` | expand_macro - show macro expansion |
| `view_crate_graph()` | show_dependencies - crate structure |
| `parent_module()` / `child_modules()` | navigate_modules |

### Test & Run Methods

| Method | MCP Tool Idea |
|--------|---------------|
| `discover_test_roots()` | list_tests |
| `runnables()` | find_runnable - find main/tests/benches |

### Refactoring Methods

| Method | MCP Tool Idea |
|--------|---------------|
| `rename()` | rename_symbol (with preview) |
| `completions()` | get_completions |

---

## Agent-Friendly Tools (New)

Designed for simple inputs and actionable outputs.

---

### Code Quality Tools

#### `find_dead_code`
Find unused functions, structs, and constants with no references.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of unused items with `{ name, kind, file, line }`

---

#### `find_unsafe_blocks`
Audit all `unsafe` blocks with surrounding context.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of unsafe blocks with `{ file, line, code_snippet, enclosing_function }`

---

#### `find_panics`
Find all potential panic points: `unwrap()`, `expect()`, `panic!()`, `unreachable!()`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of panic points with `{ file, line, kind, code_snippet }`

---

#### `find_todo_comments`
Find all TODO, FIXME, HACK, and XXX comments.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of comments with `{ file, line, kind, text }`

---

#### `find_public_without_docs`
Find public items (functions, structs, traits) missing documentation.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of undocumented items with `{ name, kind, file, line }`

---

#### `find_long_functions`
Find functions exceeding a line threshold.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |
| `threshold` | number | Max lines before flagging (default: 50) |

**Returns:** List of long functions with `{ name, file, line, length }`

---

#### `find_deeply_nested`
Find code blocks with excessive nesting depth.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |
| `max_depth` | number | Max nesting depth (default: 4) |

**Returns:** List of deeply nested blocks with `{ file, line, depth, code_snippet }`

---

#### `find_complexity_outliers`
Find functions with cyclomatic complexity above threshold.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |
| `threshold` | number | Max complexity (default: 10) |

**Returns:** List of complex functions with `{ name, file, line, complexity }`

---

### Duplication Detection Tools

#### `find_duplicates`
Find semantically similar code chunks (potential duplicates).

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |
| `threshold` | number | Similarity threshold 0.0-1.0 (default: 0.85) |
| `limit` | number | Max pairs to return (default: 50) |

**Returns:** List of duplicate pairs with `{ chunk_a: { file, line, code }, chunk_b: { file, line, code }, similarity_score }`

---

#### `find_similar_functions`
Find functions that are semantically similar to each other.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |
| `threshold` | number | Similarity threshold (default: 0.80) |

**Returns:** Clusters of similar functions with `{ functions: [{ name, file, line }], avg_similarity }`

---

### Rust-Analyzer Based Tools

#### `find_trait_implementors`
Find all types implementing a given trait.

| Parameter | Type | Description |
|-----------|------|-------------|
| `trait_name` | string | Name of the trait |
| `directory` | string | Path to the Rust project |

**Returns:** List of implementors with `{ type_name, file, line, impl_block }`

---

#### `find_type_usages`
Find all places a type is used (fields, parameters, returns, bounds).

| Parameter | Type | Description |
|-----------|------|-------------|
| `type_name` | string | Name of the type |
| `directory` | string | Path to the Rust project |

**Returns:** List of usages with `{ file, line, context, usage_kind }`

---

#### `expand_macro`
Show macro expansion at a specific location.

| Parameter | Type | Description |
|-----------|------|-------------|
| `file` | string | Path to the file |
| `line` | number | Line number |
| `column` | number | Column number |

**Returns:** `{ original, expanded }`

---

#### `find_generic_instantiations`
Find all concrete types a generic function/struct is instantiated with.

| Parameter | Type | Description |
|-----------|------|-------------|
| `symbol_name` | string | Name of the generic item |
| `directory` | string | Path to the Rust project |

**Returns:** List of instantiations with `{ concrete_types, file, line }`

---

### Async/Concurrency Tools

#### `find_async_functions`
Find all async functions with their await points.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of async functions with `{ name, file, line, await_count, await_locations }`

---

#### `find_blocking_in_async`
Find potentially blocking calls inside async functions (std::fs, std::thread::sleep, etc.).

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of blocking calls with `{ file, line, call, enclosing_async_fn }`

---

#### `find_spawn_points`
Find all task spawn points (tokio::spawn, std::thread::spawn, rayon, etc.).

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of spawn points with `{ file, line, spawn_type, code_snippet }`

---

### Error Handling Tools

#### `find_error_handlers`
Find all Result/Option handling patterns.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |
| `pattern` | string | Optional: filter by pattern (unwrap, expect, match, ?) |

**Returns:** List of error handling sites with `{ file, line, pattern, code_snippet }`

---

#### `find_unhandled_results`
Find places where Result is ignored (not unwrapped, matched, or propagated).

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of ignored results with `{ file, line, function_call }`

---

### Documentation Tools

#### `explain_function`
Get a comprehensive summary of a function.

| Parameter | Type | Description |
|-----------|------|-------------|
| `file` | string | Path to the file |
| `function_name` | string | Name of the function |

**Returns:** `{ signature, docstring, calls, called_by, complexity, loc, parameters, return_type }`

---

#### `explain_module`
Get a summary of a module's contents and purpose.

| Parameter | Type | Description |
|-----------|------|-------------|
| `file` | string | Path to mod.rs or module file |

**Returns:** `{ public_items, private_items, imports, exports, submodules }`

---

### Dependency Analysis Tools

#### `find_circular_deps`
Find circular dependencies between modules.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of cycles with `{ cycle: [module_a, module_b, ...], files }`

---

#### `module_coupling`
Analyze coupling between modules.

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Path to the Rust project |

**Returns:** List of module pairs with `{ module_a, module_b, coupling_score, shared_types, calls }`

---

#### `external_dep_usage`
Find where external crate dependencies are used.

| Parameter | Type | Description |
|-----------|------|-------------|
| `crate_name` | string | Name of the external crate |
| `directory` | string | Path to the Rust project |

**Returns:** List of usages with `{ file, line, imported_item, usage_context }`

---

## Implementation Priority

### High Value, Low Effort
1. `find_dead_code` - rust-analyzer has reference info
2. `find_panics` - simple AST pattern match
3. `find_todo_comments` - regex scan
4. `find_duplicates` - leverage existing vector store
5. `find_unsafe_blocks` - AST pattern match

### High Value, Medium Effort
6. `find_trait_implementors` - rust-analyzer supports this
7. `find_public_without_docs` - combine visibility + docstring check
8. `find_async_functions` - AST scan + await point extraction
9. `explain_function` - combine existing tools
10. `find_long_functions` - simple line counting

### Medium Value, Higher Effort
11. `find_blocking_in_async` - needs known-blocking-calls list
12. `find_circular_deps` - graph analysis on imports
13. `expand_macro` - rust-analyzer macro expansion
14. `find_generic_instantiations` - complex type analysis

---

## What Makes a Good Agent Tool

| Good | Bad |
|------|-----|
| Single clear input -> single clear output | Requires multi-step orchestration |
| Returns actionable results | Returns raw data needing interpretation |
| Fast (< 30 seconds) | Long-running analysis |
| Self-contained | Needs agent to combine multiple calls |
