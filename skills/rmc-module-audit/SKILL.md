---
name: rmc-module-audit
description: Audit one Rust module.
argument-hint: "<crate::path::module> [consumer-crate]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust module audit

A crate's submodule under audit. Same shape as `rmc-crate-audit` but at
finer granularity. For per-symbol forensics, hand off to
`rmc-symbol-forensics`. For facade-vs-canonical hygiene at the crate root,
use `rmc-api-surface`.

## Scope — single module

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Pull module data (parallel)

```
get_imports(directory=..., module=<crate::path::module>)
get_dependencies(file_path=<file_path>)
get_exports(directory=..., module=<crate::path::module>, consumer=<other>)
get_reexports(directory=..., module=<crate::path::module>, consumer=<other>)
get_declared_reexports(directory=..., module=<crate::path::module>)
```

`get_imports` is module-level (use / extern crate edges in the binding
scope). `get_dependencies` is file-level — use when you don't have a clean
module path.

### Step 2. Internal structure

Walk `module_tree` into the module path:

```
module_tree(directory=..., krate=<crate>)
```

For parser-driven function-call relationships within the module's files:

```
get_call_graph(file_path=<file_path>)
```

### Step 3. What does the module re-export?

Two flavors:

- `get_declared_reexports(module=...)` — every `pub use` declared at this
  module, regardless of who can reach it.
- `get_reexports(module=..., consumer=...)` — `pub use` reachable from the
  named consumer, visibility-filtered.

Empty `get_declared_reexports` is informative — the module has no facade.

### Step 4. Verify exports match expectations

```
get_exports(directory=..., module=..., consumer=<external_crate>)
```

vs the `pub` items reachable from `module_tree`. Items that appear in
`module_tree` as `pub` but NOT in `get_exports(consumer=external)` are
leaking through `pub(crate)` and not actually crossing the crate boundary —
a visibility-discipline signal.

## Decision frames

| Finding | Action |
|---|---|
| `get_imports` shows wildcard imports (`use foo::*`) | Flag — explicit imports preferred for review |
| `get_declared_reexports` non-empty + facade dead | Drop the `pub use` (recipe in `rmc-api-surface`) |
| `get_exports(consumer=X)` empty for X already in `crate_edges` | Visibility filter trimming everything — likely a `pub(in <crate>)` boundary |
| Module imports many crates | Coupling smell — verify each import is justified (`rmc-imports-exports`) |
| Module is pure facade (imports == re-exports) | Question whether the indirection earns its keep |

## Pattern reference

| Signal | Means |
|---|---|
| Module imports nothing except the declaring crate | Pure leaf module |
| Module imports many cross-crate types | Coordination layer / glue code |
| Module imports + re-exports the same set | Pure facade module |
| `get_declared_reexports` empty | Module has no facade; everything at canonical paths |
| `pub` items in `module_tree` missing from `get_exports(external)` | Items are `pub(crate)`-scoped despite the `pub` keyword |

## Output format

Severity-ranked findings:

```
🔴 High    — broken or contradictory state (re-exports a deleted item,
            facade with no consumers)
🟡 Medium  — wasted indirection (pure facade with one canonical alternative)
🟢 Low     — naming clarity, wildcard imports
⚪ Info    — confirms healthy structure (clean leaf module, balanced
            import/export ratio)
```

## Limitations

- `get_imports` resolves module-level binding scope; macro-introduced
  imports may be missed.
- `get_call_graph` is parser-driven and file-scoped — it does not cross
  module boundaries. For workspace-wide call graphs use `rmc-call-graph`.
- `get_exports` visibility filtering depends on the consumer being a
  valid crate name in the workspace; pass a non-existent name and you
  get an empty result (not an error).
