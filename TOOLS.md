# rust-code-mcp Tools Reference

Complete reference for all MCP tools provided by rust-code-mcp.

## Overview

| Tool | Category | Description |
|------|----------|-------------|
| [`search`](#search) | Query | Hybrid keyword + semantic search |
| [`get_similar_code`](#get_similar_code) | Query | Find semantically similar code |
| [`read_file_content`](#read_file_content) | Query | Read file contents |
| [`find_definition`](#find_definition) | Analysis | Locate symbol definitions by name |
| [`find_references`](#find_references) | Analysis | Find all usages of a symbol by name |
| [`get_dependencies`](#get_dependencies) | Analysis | List imports for a file |
| [`get_call_graph`](#get_call_graph) | Analysis | Show function call relationships |
| [`analyze_complexity`](#analyze_complexity) | Analysis | Calculate code complexity metrics |
| [`index_codebase`](#index_codebase) | Index | Manually trigger indexing |
| [`health_check`](#health_check) | Index | Check system status |
| [`clear_cache`](#clear_cache) | Index | Clear corrupted cache/index files |
| [`build_hypergraph`](#build_hypergraph) | Graph: Build | Build/reuse persisted workspace hypergraph |
| [`get_imports`](#get_imports) | Graph: Imports/Exports | List `use`/extern-crate imports of a module |
| [`get_exports`](#get_exports) | Graph: Imports/Exports | Items visible to a consumer module |
| [`get_reexports`](#get_reexports) | Graph: Imports/Exports | `pub use` subset of get_exports |
| [`get_declared_reexports`](#get_declared_reexports) | Graph: Imports/Exports | Every `pub use` declared in a module |
| [`who_imports`](#who_imports) | Graph: Reverse Lookup | Every importer of a symbol |
| [`who_uses`](#who_uses) | Graph: Reverse Lookup | Every non-import reference (file:byte hits) |
| [`who_uses_summary`](#who_uses_summary) | Graph: Reverse Lookup | who_uses aggregated per consumer module |
| [`who_calls`](#who_calls) | Graph: Call Graph | Every fn-body reference to a target fn |
| [`calls_from`](#calls_from) | Graph: Call Graph | Outgoing references from a caller fn body |
| [`call_graph`](#call_graph) | Graph: Call Graph | Bounded recursive descent of call edges |
| [`callers_in_crate`](#callers_in_crate) | Graph: Call Graph | who_calls filtered by caller's crate |
| [`recursive_callers_count`](#recursive_callers_count) | Graph: Call Graph | Reverse BFS counting transitive callers |
| [`dead_pub_in_crate`](#dead_pub_in_crate) | Graph: Structure | `pub` items with no cross-crate consumer |
| [`dead_pub_report`](#dead_pub_report) | Graph: Structure | Workspace-wide dead-pub aggregate |
| [`crate_edges`](#crate_edges) | Graph: Structure | Cross-crate consumer→producer edges |
| [`overlaps`](#overlaps) | Graph: Structure | Workspace name-collision/shadow report |
| [`module_tree`](#module_tree) | Graph: Structure | Recursive module/item tree dump |
| [`workspace_stats`](#workspace_stats) | Graph: Structure | Workspace counters (nodes/items/bindings) |
| [`forbidden_dependency_check`](#forbidden_dependency_check) | Graph: Audit | Architectural-rule check over crate edges |
| [`enum_variants`](#enum_variants) | Graph: Audit | Enumerate variants of an enum |
| [`item_attributes`](#item_attributes) | Graph: Audit | Outer attributes + doc-comment lines for an item |
| [`items_with_attribute`](#items_with_attribute) | Graph: Audit | Items in a crate matching an attribute pattern |
| [`pub_use_pub_type_audit`](#pub_use_pub_type_audit) | Graph: Audit | Heuristic `pub type` re-export audit |
| [`re_export_chain`](#re_export_chain) | Graph: Audit | Walk `pub use` re-export chain of a target |
| [`crate_dependency_metric`](#crate_dependency_metric) | Graph: Audit | Robert Martin instability + abstractness per crate |
| [`function_signature`](#function_signature) | Graph: Signatures | Recorded FunctionSignature for a function |
| [`functions_with_filter`](#functions_with_filter) | Graph: Signatures | Functions in a crate matching a signature filter |
| [`unsafe_audit`](#unsafe_audit) | Graph: Safety | Audit every `unsafe { ... }` block in local crates |
| [`mut_static_audit`](#mut_static_audit) | Graph: Safety | Audit `static mut`/`LazyLock`/`OnceLock`/`OnceCell` |
| [`similar_to_item`](#similar_to_item) | Graph: Semantic | Find semantic neighbors of a hypergraph Item via vector embeddings |
| [`semantic_overlaps`](#semantic_overlaps) | Graph: Semantic | Workspace-wide audit: cluster semantically-similar Items via vector embeddings |

---

## Query Tools

### search

Hybrid search combining BM25 keyword matching with semantic vector similarity (RRF fusion).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `keyword` | string | Yes | Search query |
| `directory` | string | Yes | Project root directory |

**Example:**
```json
{
  "keyword": "parse rust source",
  "directory": "/path/to/project"
}
```

**Returns:** Ranked list of matching code chunks with scores, file paths, symbol names, line numbers, and preview.

---

### get_similar_code

Find code snippets semantically similar to a query using vector embeddings.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | string | Yes | Code snippet or natural language query |
| `directory` | string | Yes | Directory containing the codebase |
| `limit` | integer | No | Number of results (default: 5) |

**Example:**
```json
{
  "query": "function that reads configuration from file",
  "directory": "/path/to/project",
  "limit": 3
}
```

**Returns:** Similar code snippets ranked by semantic similarity score (0-1).

---

### read_file_content

Read the content of a file.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to read |

**Example:**
```json
{
  "file_path": "/path/to/project/src/main.rs"
}
```

**Returns:** Full file contents as text.

---

## Analysis Tools

### find_definition

Find where a Rust symbol (function, struct, trait, const, etc.) is defined. Uses rust-analyzer's semantic analysis for accurate results.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `symbol_name` | string | Yes | Name of the symbol to find |
| `directory` | string | Yes | Project root directory containing Cargo.toml |

**Example:**
```json
{
  "symbol_name": "RustParser",
  "directory": "/path/to/project"
}
```

**Returns:** Definition location(s) with file path, line, column, and symbol name.

**Example output:**
```
Found 1 definition(s) for 'RustParser':
/path/to/project/src/parser/mod.rs:121:12 (RustParser)
```

**Notes:**
- First query triggers lazy loading of rust-analyzer (~120ms)
- Subsequent queries are instant (<10ms)
- Only searches local project code (not dependencies)

---

### find_references

Find all places where a symbol is used (calls, type references, etc.). Uses rust-analyzer's semantic analysis.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `symbol_name` | string | Yes | Name of the symbol to find references for |
| `directory` | string | Yes | Project root directory containing Cargo.toml |

**Example:**
```json
{
  "symbol_name": "parse_file",
  "directory": "/path/to/project"
}
```

**Returns:** All reference locations with file path, line, and column.

**Example output:**
```
Found 21 reference(s) for 'RustParser':
/path/to/project/src/indexing/indexer_core.rs:64:20 (reference)
/path/to/project/src/indexing/indexer_core.rs:88:13 (reference)
/path/to/project/src/parser/mod.rs:121:12 (RustParser)
...
```

---

### get_dependencies

Get import dependencies for a Rust source file.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to analyze |

**Example:**
```json
{
  "file_path": "/path/to/project/src/parser/mod.rs"
}
```

**Returns:** List of all imports in the file.

**Example output:**
```
Dependencies for '/path/to/project/src/parser/mod.rs':

Imports (19):
- std::fs
- std::path::Path
- ra_ap_syntax::ast::self
- ra_ap_syntax::AstNode
...
```

---

### get_call_graph

Get the call graph showing function call relationships for a specific symbol.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to analyze |
| `symbol_name` | string | No | Specific symbol to get call graph for |

**Example:**
```json
{
  "file_path": "/path/to/project/src/parser/mod.rs",
  "symbol_name": "parse_file"
}
```

**Returns:** Functions called by the specified symbol.

**Example output:**
```
Call graph for '/path/to/project/src/parser/mod.rs':

Symbol: parse_file

Calls (2):
  -> read_to_string
  -> parse_source
```

---

### analyze_complexity

Analyze code complexity metrics including LOC, cyclomatic complexity, and function counts.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `file_path` | string | Yes | Path to the file to analyze |

**Example:**
```json
{
  "file_path": "/path/to/project/src/parser/mod.rs"
}
```

**Returns:** Complexity metrics for the file.

**Example output:**
```
Complexity analysis for '/path/to/project/src/parser/mod.rs':

=== Code Metrics ===
Total lines:           619
Non-empty lines:       552
Comment lines:         73
Code lines (approx):   479

=== Symbol Counts ===
Functions:             21
Structs:               4
Traits:                0

=== Complexity ===
Total cyclomatic:      45
Avg per function:      2.14
Function calls:        69
```

---

## Index Tools

### index_codebase

Manually index a codebase directory. Uses incremental indexing with Merkle tree change detection.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Absolute path to codebase directory |
| `force_reindex` | boolean | No | Force full reindex even if already indexed (default: false) |

**Example:**
```json
{
  "directory": "/path/to/project",
  "force_reindex": false
}
```

**Returns:** Indexing statistics including files indexed, chunks created, and timing.

**Example output:**
```
Indexing stats:
- Indexed files: 0 (no changes)
- Total chunks: 0
- Unchanged files: 131
- Skipped files: 0
- Time: 78.100539ms (< 10ms change detection)

Background sync: enabled (5-minute interval)
```

**Notes:**
- First indexing takes longer (parsing + embedding generation)
- Subsequent runs use Merkle tree to detect changes (~10ms)
- Background sync automatically re-indexes every 5 minutes

---

### health_check

Check the health status of the code search system (BM25, Vector store, Merkle tree).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | No | Project directory to check (checks system-wide if not provided) |

**Example:**
```json
{
  "directory": "/path/to/project"
}
```

**Returns:** Health status of all system components.

**Example output:**
```
System Status: HEALTHY

{
  "overall": "healthy",
  "bm25": {
    "status": "healthy",
    "message": "BM25 search operational",
    "latency_ms": 0
  },
  "vector": {
    "status": "healthy",
    "message": "Vector store operational (921 vectors)",
    "latency_ms": 1
  },
  "merkle": {
    "status": "healthy",
    "message": "Merkle snapshot exists (19159 bytes)"
  }
}
```

**Status levels:**
- **Healthy**: All components operational
- **Degraded**: One search engine down OR Merkle snapshot missing
- **Unhealthy**: Both BM25 and Vector search are down

---

### clear_cache

Clear corrupted cache, index, and vector store files. Use this to fix `Failed to open MetadataCache` errors. Clears the metadata cache, tantivy index, and vector store for the named project (or all projects when `directory` is omitted).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | No | Project directory to clear cache for. If omitted, clears every cached project. |

**Example:**
```json
{
  "directory": "/path/to/project"
}
```

**Returns:** Plain-text status listing the directories cleared and any errors encountered.

**Notes:**
- Safe to run while the MCP server is up; the next `index_codebase` call rebuilds from scratch.
- Does NOT clear the persisted hypergraph snapshot — call `build_hypergraph` with `force_rebuild: true` for that.

---

## Hypergraph Tools

The hypergraph layer is a separate read-side query system backed by an LMDB snapshot built once per workspace fingerprint. Every tool below requires that `build_hypergraph` has run at least once for the workspace; if the snapshot is missing, the call returns `invalid_params` with the message `no snapshot at <directory> — call build_hypergraph first`.

### Build & Lifecycle

#### build_hypergraph

Build or reuse a persisted workspace hypergraph snapshot (HIR-driven, `no_deps=true`). Cold rebuild is roughly 5-18s depending on workspace size; reuse of an existing snapshot is essentially free.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root (directory containing Cargo.toml) |
| `force_rebuild` | boolean | No | Force a rebuild even if a snapshot for the current fingerprint already exists (default: false) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "force_rebuild": false
}
```

**Returns:**
```json
{
  "graph_id": "...",
  "workspace_root": "/path/to/workspace",
  "fingerprint": "...",
  "node_count": 1234,
  "binding_count": 5678,
  "usage_count": 9012,
  "reused": true,
  "snapshot_path": "/.../snapshot/.../db"
}
```

**Notes:**
- Runs `loader::load` + the full extract pass + LMDB writes synchronously on a blocking thread.
- `reused: true` means the existing snapshot's fingerprint matched and no rebuild was needed.

---

### Imports / Exports / Re-exports

#### get_imports

List `use` / extern-crate imports declared in a module from the persisted hypergraph.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `module` | string | Yes | Module qualified name (e.g. `my_crate::sub::module`) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "module": "my_crate::indexer"
}
```

**Returns:** `BindingsListResponse { module, bindings: [{ visible_name, namespace, kind, visibility, from_module?, target?, target_kind? }] }`.

---

#### get_exports

List items declared in or re-exported from a module that are visible from a given consumer module.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `module` | string | Yes | Module to enumerate exports from (qualified name) |
| `consumer` | string | Yes | Consumer module from whose viewpoint visibility is checked |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "module": "my_crate::api",
  "consumer": "my_crate::tests"
}
```

**Returns:** `BindingsListResponse` with `module`, `consumer`, and the visible-to-consumer bindings list.

**Notes:** A crate name passed where a module is expected is transparently promoted to that crate's root module.

---

#### get_reexports

List re-exports (the subset of `get_exports` that came via `pub use`) visible from a given consumer module.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `module` | string | Yes | Module to enumerate re-exports from |
| `consumer` | string | Yes | Consumer module from whose viewpoint visibility is checked |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "module": "my_crate",
  "consumer": "consumer_crate"
}
```

**Returns:** `BindingsListResponse` containing only the `pub use` bindings reachable from `consumer`.

---

#### get_declared_reexports

List every explicit `pub use` (or `pub(crate)` / `pub(in path)` / `pub(super)`) declared in a module, regardless of whether it is reachable from any specific consumer. Use this to audit a module's declared re-export surface; for visibility-filtered re-exports use `get_reexports` instead.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `module` | string | Yes | Module to enumerate explicit `pub use` declarations from |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "module": "my_crate"
}
```

**Returns:** `BindingsListResponse` listing each declared re-export with its `visibility` field reflecting the actual visibility modifier.

---

### Reverse Lookup

#### who_imports

Find every workspace module that imports the given symbol (matched by qualified name).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the symbol whose importers you want |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::Foo"
}
```

**Returns:** `BindingsListResponse` with one binding per importer (each carrying the `from_module` of the importer).

**Notes:** The target may be any node kind (Item, Module, ExternalSymbol).

---

#### who_uses

List every non-import reference to the given symbol (file path + byte range + Read/Write/Test/Other category). Complements `who_imports`, which only enumerates `use` edges.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the symbol whose non-import references you want |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::module::do_thing"
}
```

**Returns:**
```json
{
  "target": "my_crate::module::do_thing",
  "usages": [
    { "file": "src/foo.rs", "start": 1024, "end": 1032, "category": "Read", "consumer_module": "my_crate::foo", "consumer_function": "my_crate::foo::caller" }
  ]
}
```

**Notes:** Cross-crate method calls and trait dispatch are NOT included (Layer 4 limitation).

---

#### who_uses_summary

Aggregation rollup of `who_uses`: every non-import reference grouped by consumer module, with total count plus per-category breakdown (Read / Write / Test / Other).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the symbol to summarize |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::module::do_thing"
}
```

**Returns:** `{ target, rows: [{ consumer_module, total, read, write, test, other }] }`.

**Notes:** Same caveat as `who_uses` — cross-crate method calls / trait dispatch are NOT included.

---

### Call Graph (Layer 10)

#### who_calls

Layer 10 call graph: every non-import reference to the target function whose call site sits inside another function body. References in const initializers, type aliases, and other non-function scopes are excluded — use `who_uses` to see all reference sites.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the target function whose callers you want |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::module::do_thing"
}
```

**Returns:**
```json
{
  "target": "my_crate::module::do_thing",
  "call_sites": [
    { "caller": "my_crate::other::caller_fn", "file": "src/other.rs", "start": 512, "end": 520, "category": "Read" }
  ]
}
```

**Notes:** Calls from closures attribute to the enclosing fn.

---

#### calls_from

Layer 10 call graph: every non-import reference made from the body of the caller function. References in const initializers, type aliases, and other non-function scopes are excluded.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `caller` | string | Yes | Qualified name of the caller function whose outgoing references you want |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "caller": "my_crate::module::caller_fn"
}
```

**Returns:** `{ caller, call_sites: [{ callee, file, start, end, category }] }`.

**Notes:** Calls from closures attribute to the enclosing fn.

---

#### call_graph

Bounded recursive descent over outgoing call edges from a `root` function. `depth` defaults to 3 and is capped at 8 (deeper trees rarely fit usefully in a single response).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `root` | string | Yes | Qualified name of the root function to descend from |
| `depth` | integer | No | Max recursion depth (default 3, capped at 8) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "root": "my_crate::main",
  "depth": 3
}
```

**Returns:**
```json
{
  "root": "my_crate::main",
  "depth": 3,
  "tree": {
    "fn_qualified_name": "my_crate::main",
    "crate_name": "my_crate",
    "callees": [],
    "truncated_at_cycle": false,
    "truncated_at_depth": false
  }
}
```

**Notes:**
- `truncated_at_cycle = true` means the fn was already expanded earlier in the traversal — its callees are visible elsewhere in the tree.
- `truncated_at_depth = true` means depth ran out at this node and there were unvisited callees.

---

#### callers_in_crate

`who_calls(target)` filtered to call sites whose *caller fn* lives in the named crate. Useful for asking "which fns inside crate X call Y?" regardless of which crate Y lives in.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the target function |
| `krate` | string | Yes | Qualified name of the crate to filter callers by (filters the *caller's* crate, not the target's) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "shared::log",
  "krate": "my_app"
}
```

**Returns:** `{ target, crate, call_sites: [...] }` — same call-site shape as `who_calls`.

---

#### recursive_callers_count

Reverse BFS from `target`: counts distinct caller fns reachable backward up to `depth` hops. Counts *fns*, not call sites — a fn that calls target 5 times counts as 1 caller.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the target function whose transitive callers you want to count |
| `depth` | integer | No | Max BFS depth in caller hops (default 3, capped at 8) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::do_thing",
  "depth": 3
}
```

**Returns:**
```json
{
  "direct_callers": 4,
  "transitive_callers": 27,
  "depth_reached": 3,
  "truncated_at_depth": false
}
```

**Notes:** `depth=0` returns zeros; `depth=1` is just the direct caller count.

---

### Workspace Structure / Audits

#### dead_pub_in_crate

Scan a local crate for `pub` items with no cross-crate importer or reference — candidates for downgrading to `pub(crate)`.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `krate` | string | Yes | Qualified name of the local crate to scan |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "krate": "my_crate"
}
```

**Returns:** `{ "crate": "my_crate", "findings": [{ qualified_name, item_kind, declared_visibility, file?, span? }] }`.

**Notes:** Conservative — may miss items used only through public type signatures.

---

#### dead_pub_report

Run `dead_pub_in_crate` over every local crate in the workspace and return a single aggregated report. Each finding includes file path + byte span so callers can navigate directly to the declaration.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |

**Example:**
```json
{ "directory": "/path/to/workspace" }
```

**Returns:** `{ workspace, total_findings, crates: [{ "crate": ..., findings: [...] }] }`.

---

#### crate_edges

All cross-crate consumer→producer edges in the workspace, with the symbols carrying each edge (sorted by total ref count desc).

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |

**Example:**
```json
{ "directory": "/path/to/workspace" }
```

**Returns:** `{ edges: [{ consumer_crate, producer_crate, symbols: [...], total_refs }] }`.

**Notes:** Cross-crate method calls and trait method dispatch are NOT captured in usage counts — Layer 4 doesn't extract impl-block items as Item nodes, so `usage_count` reflects only references to module-level items.

---

#### overlaps

Workspace-wide name-collision report: cross-crate type collisions, module names that shadow another crate, within-crate type duplicates, and fn names that appear in 4+ crates.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |

**Example:**
```json
{ "directory": "/path/to/workspace" }
```

**Returns:** A report struct with sections for type collisions, module shadows, intra-crate duplicates, and high-fan-out fn names.

---

#### module_tree

Recursive module/item tree dump rooted at the specified crate.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `krate` | string | Yes | Crate qualified name |
| `depth` | integer | No | Max depth below the crate root (omit to walk the full tree) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "krate": "my_crate",
  "depth": 3
}
```

**Returns:** `{ tree: ModuleTreeNode }` — a recursive struct of nested modules and their items.

---

#### workspace_stats

Workspace-wide counters: nodes by kind, items by `ItemKind`, bindings by `BindingKind`, declared-binding visibility breakdown, and `pub_crate / total_items` encapsulation ratio.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |

**Example:**
```json
{ "directory": "/path/to/workspace" }
```

**Returns:** A `WorkspaceStats` struct serialized as JSON; counters are nested by kind.

---

### Architectural Rules & Audits

#### forbidden_dependency_check

Architectural-rule check: a pure filter over `crate_edges`. Each rule has glob-style `consumer` and `producer` patterns (with `*` wildcards), plus optional `except` (consumer-side override), `severity`, and `message`. Returns one violation per (rule × matching edge), each with sample_symbol/unique_symbols/total_refs for the offending edge.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `rules` | array of `ForbiddenDependencyRule` | Yes | Architectural rules to enforce |

Each rule:
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `consumer` | string | Yes | Glob pattern matched against the consumer crate name (e.g. `domain*`) |
| `producer` | string | Yes | Glob pattern matched against the producer crate name (e.g. `tokio`) |
| `except` | string | No | Consumer-side glob exception |
| `severity` | string | No | Severity tag passed through to violations (e.g. `error` / `warn`) |
| `message` | string | No | Human-readable rationale, passed through unchanged |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "rules": [
    { "consumer": "domain*", "producer": "tokio", "severity": "error", "message": "domain crates must be runtime-agnostic" }
  ]
}
```

**Returns:**
```json
{
  "rule_count": 1,
  "violation_count": 2,
  "violations": [
    { "rule": { "consumer": "domain*", "producer": "tokio" }, "edge": { "consumer_crate": "domain_x", "producer_crate": "tokio" }, "sample_symbol": "tokio::spawn", "unique_symbols": 5, "total_refs": 17 }
  ]
}
```

**Notes:** Same caveat as `crate_edges`: cross-crate method calls / trait dispatch are NOT counted.

---

#### enum_variants

Enumerate the variants of an enum: returns one row per variant with `display_name`, `qualified_name`, and `(file, byte span)` so callers can navigate to the declaration. Use this with `who_uses(MyEnum::SomeVariant)` to investigate per-variant fan-in.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Enum's qualified name (e.g. `my_crate::ErrorKind`) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::ErrorKind"
}
```

**Returns:** `{ enum_qualified_name, variant_count, variants: [{ display_name, qualified_name, file?, span? }] }`.

---

#### item_attributes

Outer attributes and doc-comment lines recorded for the Item at `target`. Returns the trimmed source text of each `#[...]` attribute (e.g. `#[derive(Debug, Clone)]`, `#[must_use]`, `#[non_exhaustive]`, `#[inline]`) and each doc-comment line as `/// ...` (one entry per line). Source order preserved. Empty list when the item has no attributes or its AST source can't be resolved.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the item (e.g. `my_crate::Foo`) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::Foo"
}
```

**Returns:** `{ target, item_kind?, file?, span?, attribute_count, attributes: ["#[derive(Debug, Clone)]", "/// docs line", ...] }`.

---

#### items_with_attribute

Find every Item in the named crate whose attribute list has at least one entry that anchor-matches `attribute_pattern`. The match is case-sensitive and tested as a **prefix** against each attribute string OR as a prefix against the **body** of a `///` doc-comment. Each result row carries `match_location: "attr"` or `"doc"` so callers can filter visually.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `crate_name` | string | Yes | Crate qualified name to scan |
| `attribute_pattern` | string | Yes | Substring to anchor-match against each attribute / doc-comment body |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "crate_name": "my_crate",
  "attribute_pattern": "#[must_use]"
}
```

**Returns:** `{ "crate", attribute_pattern, match_count, items: [{ qualified_name, item_kind?, matched_attribute, match_location, file?, span? }] }`.

**Notes:**
- Empty pattern returns no results.
- Useful for `#[must_use]` / `#[non_exhaustive]` / `#[inline]` audits, finding items missing a required derive, or scanning doc-comment text.

---

#### pub_use_pub_type_audit

Heuristic audit: every `pub type` alias in the named crate whose owning module also carries a `pub use ... as <alias_name>` (or `pub use ::<alias_name>`) binding. Indicates the alias may be acting as a re-export disguised as a `pub type` declaration.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `crate_name` | string | Yes | Crate qualified name to scan |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "crate_name": "my_crate"
}
```

**Returns:** `{ "crate", finding_count, findings: [{ alias_qualified_name, file?, span?, suspicious_pub_use_visible_name, suspicious_pub_use_target? }] }`.

**Notes:** The model does NOT record what an alias's RHS resolves to, so this query cannot confirm the `pub use` and `pub type` point at the same target — verify with `find_definition` before acting.

---

#### re_export_chain

Walk every `pub use` re-export of `target` (and every re-export of those re-exports) up to 8 hops with cycle detection. Returns one link per visited binding, breadth-first. Useful for auditing the public surface of a type.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the canonical declaration whose re-export chain you want to walk |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::module::Token"
}
```

**Returns:**
```json
{
  "canonical": "my_crate::module::Token",
  "link_count": 4,
  "links": [
    { "from_module": "my_crate", "visible_name": "Token", "depth": 1 }
  ]
}
```

---

#### crate_dependency_metric

Per-local-crate Robert Martin instability metric plus an abstractness ratio. `efferent` (Ce) = distinct outgoing producer crates; `afferent` (Ca) = distinct incoming consumer crates; `instability = Ce / (Ce + Ca)` (0 = max stable, 1 = max unstable). `abstractness = (traits + pub_type_aliases) / total_items`. Both metrics are NaN-guarded — degenerate counts return 0.0.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `top_n` | integer | No | Cap on returned rows after sorting (default: all rows) |
| `sort_by` | string | No | Sort key applied before slicing: `instability`, `item_count`, `afferent`, `efferent`, `abstractness` (all descending) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "sort_by": "instability",
  "top_n": 10
}
```

**Returns:**
```json
{
  "crate_count": 12,
  "metrics": [
    { "crate_id": "<64-char-hex>", "crate_name": "my_crate", "efferent": 5, "afferent": 2, "instability": 0.71, "abstractness": 0.18, "item_count": 142 }
  ]
}
```

**Notes:** `crate_id` is rendered as a 64-char hex string. Unknown `sort_by` values produce an `invalid_params` error.

---

### Function Signatures (Phase 5)

#### function_signature

Return the recorded `FunctionSignature` for a function (free fn, inherent assoc fn, trait declaration fn). Carries `is_async`, `self_param` (Owned/Ref/RefMut, or null for free fns), `params` (name + stringified type + by_ref + mutability), `return_type`, and generic type parameters with their declaration-site trait bounds.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `target` | string | Yes | Qualified name of the function (e.g. `crate::module::fn_name` or `crate::Type::method`) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::do_thing"
}
```

**Returns:**
```json
{
  "target": "my_crate::do_thing",
  "signature": {
    "is_async": false,
    "self_param": null,
    "params": [{ "name": "input", "type_string": "&str", "by_ref": true, "mutability": "Shared" }],
    "return_type": "Result<(), Error>",
    "generics": []
  }
}
```

**Notes:**
- Type strings come from RA's `HirDisplay` rendered against the function's owning crate; anonymous lifetimes (`'_`) are suppressed by default.
- Allocator/hasher type parameters (`, Global>`, `, RandomState>`, `, BuildHasherDefault<...>>`) and `LazyLock`/`OnceLock` init-fn pointer parameters are stripped from rendered types.
- `signature: null` when the target isn't a fn or extraction skipped it.
- `trait_bounds` reflects the parameter's declaration-site bounds only — where-clause bounds added later are NOT included (RA limitation).

---

#### functions_with_filter

Every local function in the named crate whose recorded `FunctionSignature` matches every `Some` field of the filter. Substring matches (`has_param_type`, `returns_type_pattern`) are case-sensitive against `HirDisplay` strings. `self_kind` accepts `"none"` | `"owned"` | `"ref"` | `"ref_mut"` — `"none"` matches free fns and assoc fns without self.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |
| `krate` | string | Yes | Crate qualified name to scope the search |
| `min_param_count` | integer | No | Minimum non-self param count |
| `has_param_type` | string | No | Substring that must appear in at least one param's stringified type |
| `returns_type_pattern` | string | No | Substring that must appear in the function's stringified return type |
| `is_async` | boolean | No | `true` to require `async fn`, `false` to require non-async |
| `self_kind` | string | No | `"none"` \| `"owned"` \| `"ref"` \| `"ref_mut"` |
| `limit` | integer | No | Cap on returned matches after slicing (default: 50) |
| `offset` | integer | No | Offset into the (sorted) match list (default: 0) |
| `summary` | boolean | No | When `true`, drops the `signature` payload from each match (default: false) |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "krate": "my_crate",
  "returns_type_pattern": "Result<",
  "is_async": true,
  "limit": 20
}
```

**Returns:**
```json
{
  "crate": "my_crate",
  "total_match_count": 42,
  "offset": 0,
  "limit": 20,
  "match_count": 20,
  "matches": [
    { "target": "my_crate::do_thing", "qualified_name": "my_crate::do_thing", "signature": {} }
  ]
}
```

**Notes:**
- Sorted by qualified name. Trait-impl method bodies are NOT included.
- Compare `total_match_count` to `offset + match_count` to detect "more pages exist".
- `summary: true` is useful when the full payload exceeds the MCP token budget.
- Same `HirDisplay` trim as `function_signature` (allocator/hasher/init-fn dropped).

---

### Safety Audits

#### unsafe_audit

Phase 6: query-time audit of every `unsafe { ... }` block in the workspace's local crates. Walks each `.rs` file's syntax tree (no semantic analysis beyond enclosing-fn lookup), returning per-block: workspace-relative file path, byte span of the unsafe expression (curlies included), source line count, enclosing function (NodeId rendered as 64-char hex + qualified name when resolvable, null for unsafe blocks in const initializers / trait bounds / closures-without-fn-parent), and a `has_safety_comment` heuristic flag.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |

**Example:**
```json
{ "directory": "/path/to/workspace" }
```

**Returns:**
```json
{
  "directory": "/path/to/workspace",
  "finding_count": 3,
  "findings": [
    {
      "file": "src/foo.rs",
      "span": [1024, 1100],
      "line_count": 4,
      "enclosing_function": "<64-char-hex>",
      "enclosing_function_name": "my_crate::do_unsafe_thing",
      "has_safety_comment": true
    }
  ]
}
```

**Notes:**
- `has_safety_comment` is true when `SAFETY` appears as a substring in any of the 5 source lines preceding the `unsafe` keyword.
- Live computation; nothing cached — per-invocation cost is dominated by the workspace load (~2-3s).
- Sorted by `(file, span)`.

---

#### mut_static_audit

Phase 7 Path B (v10): type-aware audit of every local `static` item that matches a known global-mutable-state pattern. Reads the static's HIR type via `HirDisplay` (no source-text regex) and classifies against `static mut`, `LazyLock<...>`, `OnceLock<...>`, `OnceCell<...>`. A single static matching multiple patterns produces one finding per pattern.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root |

**Example:**
```json
{ "directory": "/path/to/workspace" }
```

**Returns:**
```json
{
  "directory": "/path/to/workspace",
  "finding_count": 5,
  "findings": [
    {
      "item": "<64-char-hex>",
      "qualified_name": "my_crate::CONFIG",
      "matched_pattern": "LazyLock<...>",
      "type_string": "LazyLock<Mutex<Foo>>",
      "file": "src/config.rs",
      "span": [200, 260]
    }
  ]
}
```

**Notes:**
- `type_string` is post-processed via the same `HirDisplay` trim as `function_signature` — e.g. `LazyLock<Mutex<Foo>, fn() -> Mutex<Foo>>` becomes `LazyLock<Mutex<Foo>>` (init-fn pointer dropped).
- Sorted by `(qualified_name, matched_pattern)`.
- Limitation: the `lazy_static!` macro is NOT detected — its expansion produces a generated wrapper type whose name doesn't contain `LazyLock`. Use `items_with_attribute` or grep to cover that case.

---

### Semantic

#### similar_to_item

Find semantic neighbors of a hypergraph Item using vector embeddings. Resolves `target` (qualified name) via the persisted hypergraph, reads its source bytes from the recorded `(file, span)`, then runs `vector_only_search` using that source as the query. Returns ranked matches above `threshold`, capped at `limit`, optionally filtered by `item_kind`. Self-match (the seed's own chunk) is dropped automatically via line-range overlap with the seed's byte span.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root (directory containing Cargo.toml) |
| `target` | string | Yes | Qualified name of the seed Item (function, struct, enum, etc.) |
| `limit` | integer | No | Max number of results (default: 10) |
| `threshold` | number | No | Minimum cosine similarity score (0.0-1.0). Results below are dropped. Default: 0.0 |
| `item_kind` | string | No | Restrict result kind, matching the chunk's `symbol_kind` ("function", "struct", "enum", "trait", etc.). Case-insensitive. |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "target": "my_crate::auth::AuthError",
  "limit": 10,
  "threshold": 0.8,
  "item_kind": "Struct"
}
```

**Returns:**
```json
{
  "seed": {
    "qualified_name": "my_crate::auth::AuthError",
    "file": "src/auth/error.rs",
    "span": [200, 540],
    "item_kind": "Struct"
  },
  "limit": 10,
  "threshold": 0.8,
  "item_kind_filter": "Struct",
  "match_count": 2,
  "matches": [
    {
      "similarity": 0.8731,
      "symbol_name": "ProviderError",
      "symbol_kind": "struct",
      "file": "src/provider/error.rs",
      "line_start": 88,
      "line_end": 142,
      "preview": "pub struct ProviderError {\n    kind: ProviderErrorKind,\n    source: Box<dyn Error + Send + Sync>,"
    }
  ]
}
```

**Notes:**
- **Prerequisites: BOTH `build_hypergraph` AND `index_codebase` must have run for the workspace.** This tool bridges the hypergraph (Item → file/span) with the vector store (chunk embeddings).
- The seed's own chunk is dropped via line-range overlap, not file-path-only — so the seed file's other items can still appear as matches.
- Useful for finding "what looks like X?" — duplicate error types, parser variants, builder patterns, conversion functions.
- Embeddings encode lexical+syntactic patterns more than logical intent. Tune `threshold` (start ≈ 0.80) to filter noise.
- v0.1: single-target lookup only. Pairwise scan / clustering is not implemented.

#### semantic_overlaps

Workspace-wide semantic-overlap audit. Enumerates Items (optionally scoped to a crate / item_kind), embeds each one's source via `vector_only_search`, builds a similarity graph above `threshold` (default 0.85), and either returns deduplicated pairs or single-linkage clusters of transitively-similar items. The workspace-scale counterpart to `similar_to_item`: where `similar_to_item` answers *"given X, what's like X?"*, `semantic_overlaps` answers *"what's duplicated that I don't know about?"*.

Clusters are returned sorted by `avg_similarity` descending — high-similarity small clusters appear first, large noisy clusters last.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `directory` | string | Yes | Workspace root (directory containing Cargo.toml) |
| `crate_name` | string | No | Optional crate qualified name to scope the scan. Default: all local crates. |
| `item_kind` | string | No | Optional item-kind filter ("Function" \| "Struct" \| "Enum" \| "Trait" \| "Method" \| ...). Case-insensitive. Default: all kinds. |
| `threshold` | number | No | Minimum cosine similarity (0.0-1.0). Default: 0.85 (good balance of recall vs noise at workspace scale; drop to 0.80 for crate-scoped scans where chaining is less of a problem; raise to 0.90+ for very strict "definitely duplicate" signal). |
| `max_pairs` | integer | No | Cap on returned pairs OR cluster member count. Default: 50. |
| `max_cluster_size` | integer | No | Drop clusters whose member count exceeds this cap (single-linkage chaining produces large noisy clusters). Default: 15. Set to 0 to disable. |
| `output_mode` | string | No | `"pairs"` (raw similarity edges) or `"clusters"` (single-linkage groups). Default: `"clusters"`. |
| `skip_test_chunks` | boolean | No | Drop matches whose qualified name contains `::tests::`. Default: true. |
| `cross_crate_only` | boolean | No | Drop pairs whose two items share a crate. Default: false. |

**Example:**
```json
{
  "directory": "/path/to/workspace",
  "crate_name": "my_crate",
  "item_kind": "Function",
  "threshold": 0.85,
  "output_mode": "clusters"
}
```

**Returns (clusters mode, default):**
```json
{
  "scope": {
    "directory": "/path/to/workspace",
    "crate_name": "my_crate",
    "item_kind": "Function",
    "seed_count": 142
  },
  "threshold": 0.85,
  "pair_count": 23,
  "output_mode": "clusters",
  "clusters": [
    {
      "members": [
        { "qualified_name": "my_crate::a::parse_x", "item_kind": "Fn", "file": "src/a.rs", "span": [120, 480] },
        { "qualified_name": "my_crate::b::parse_y", "item_kind": "Fn", "file": "src/b.rs", "span": [50, 410] },
        { "qualified_name": "my_crate::c::parse_z", "item_kind": "Fn", "file": "src/c.rs", "span": [10, 360] }
      ],
      "avg_similarity": 0.91,
      "min_similarity": 0.87,
      "size": 3,
      "truncated": false
    }
  ]
}
```

**Returns (pairs mode):**
```json
{
  "scope": { "directory": "...", "seed_count": 142 },
  "threshold": 0.85,
  "pair_count": 23,
  "output_mode": "pairs",
  "pairs": [
    {
      "a": { "qualified_name": "my_crate::a::parse_x", "item_kind": "Fn", "file": "src/a.rs", "span": [120, 480] },
      "b": { "qualified_name": "my_crate::b::parse_y", "item_kind": "Fn", "file": "src/b.rs", "span": [50, 410] },
      "similarity": 0.92
    }
  ]
}
```

**Use cases:**
- Offline duplicate-detection / refactor planning: find functions that should be unified, structs that should share a common type, etc.
- Audit a crate boundary: pass `cross_crate_only: true` to surface items duplicated across crates.
- Audit a specific kind: pass `item_kind: "Function"` (or "Struct") to scope the scan.

**Notes / limitations:**
- **Prerequisites: BOTH `build_hypergraph` AND `index_codebase` must have run for the workspace.** Bridges the hypergraph (Item → file/span) with the vector store (chunk embeddings).
- Latency is **seconds-to-minutes** at workspace scale (one vector search per seed Item; ~150ms per search). Scope with `crate_name` for an interactive call.
- Single-linkage clustering can chain through outliers — one bridging pair can pull two distant clusters together. Tighten `threshold` to mitigate.
- Self-matches (chunk whose `(file, line range)` overlaps the seed's span) are dropped automatically — same logic as `similar_to_item`.
- Test fixtures dominate noise; `skip_test_chunks` is on by default.
- v1.0: synchronous per-seed loop, no embedding cache. Repeated runs re-embed every item.

**Notes from validation:**
- Use `threshold: 0.80` for crate-scoped scans (chaining is less of a problem at small scale).
- Use the default `0.85` (or higher) for workspace-wide scans — 0.80 produces useless mega-clusters via single-linkage chaining.
- `cross_crate_only: true` is a strong noise filter when auditing a workspace.
- `max_cluster_size: 15` (default) drops chained mega-clusters; bump it up if you want to inspect them, set to 0 to disable entirely.

---

## Architecture

The legacy code-search/analysis tools (search, find_*, get_*, analyze_complexity, index_codebase, health_check, clear_cache) and the hypergraph tools have separate architectures.

### Search & Analysis (legacy)

```
Query Tools                    Analysis Tools              Index Tools
     |                              |                           |
     v                              v                           v
+---------+                  +-------------+            +------------+
| search  |                  | find_def    |            | index_cb   |
| similar |                  | find_ref    |            | health     |
| read    |                  | deps/graph  |            | clear_cache|
+---------+                  | complexity  |            +------------+
     |                       +-------------+                  |
     v                              |                  v
+------------+                      v                  +-------------+
| Hybrid     |              +--------------+           | Incremental |
| Search     |              | Semantic     |           | Indexer     |
| (BM25+Vec) |              | Service      |           +-------------+
+------------+              | (ra_ap_ide)  |                  |
     |                      +--------------+                  v
     v                              |                  +-------------+
+------------+                      |                  | Merkle Tree |
| Tantivy    |              +-------+-------+          | (Changes)   |
| LanceDB    |              |               |          +-------------+
+------------+              v               v
                     +--------+      +--------+
                     | Parser |      | Files  |
                     +--------+      +--------+
```

### Hypergraph (build_hypergraph + 21 graph tools)

```
build_hypergraph (one-time per fingerprint, ~5-18s cold)
        |
        v
+-----------------------------+
| HIR-driven extraction       |  loader::load + extract pass
| (rust-analyzer, no_deps)    |  on a blocking thread
+-----------------------------+
        |
        v
+-----------------------------+
| LMDB persistence            |  Snapshot keyed by workspace fingerprint;
| (snapshot path on disk)     |  reused across MCP calls until fingerprint changes.
+-----------------------------+
        |
        v
+-----------------------------+
| Read-side MCP tools         |  get_imports / get_exports / who_uses /
| (<10ms server-side per call)|  who_calls / call_graph / dead_pub_* /
+-----------------------------+  crate_edges / module_tree / ... etc.
```

## Performance

| Operation | Typical Latency |
|-----------|-----------------|
| search | 10-50ms |
| get_similar_code | 20-100ms |
| find_definition (first) | ~120ms (loads IDE) |
| find_definition (cached) | <10ms |
| find_references | 10-200ms |
| index_codebase (no changes) | ~10ms |
| index_codebase (full) | 5-30s depending on size |
| build_hypergraph (cold) | 5-18s (workspace-size dependent) |
| build_hypergraph (reused snapshot) | ~10ms |
| Hypergraph read tool (cached snapshot) | <10ms server-side |
| unsafe_audit / mut_static_audit | dominated by workspace load (~2-3s) — live, not cached |
