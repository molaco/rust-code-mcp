---
name: rmc-symbol-forensics
description: Deep dive on one Rust symbol.
argument-hint: "<qualified-symbol-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust symbol forensics

Single-symbol deep-dive. Works for structs, enums, traits, fns, methods,
consts, type aliases, assoc consts, assoc types. Scope: one symbol Y by
qualified name.

For trait-specific analysis (methods + dispatch sites), hand off to
`rmc-trait-audit`. For fn-level call graphs, hand off to `rmc-call-graph`.
For refactor decisions, hand off to `rmc-refactor-plan`. If you don't yet
have the qualified name, start with `rmc-find-symbol`.

## Scope — single symbol Y (qualified name)

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Locate

```
find_definition(symbol_name=<short_name>)         → file:line
```

Or, if `module_tree` was already pulled, use `Node.file + Node.span`
directly.

### Step 2. Render declaration

```
read_file_content(file_path=<file>)               → context around the span
```

Widen by ~10 lines for readable context.

### Step 3. Reverse lookups (parallel)

```
who_imports(directory=..., target=<qualified_name>)
who_uses(directory=..., target=<qualified_name>)
who_uses_summary(directory=..., target=<qualified_name>)
```

- `who_imports` — every `use` statement bringing Y into scope.
- `who_uses` — every non-import reference (file:byte-range hits).
- `who_uses_summary` — aggregated by consumer module with Test/Other category
  breakdown.

### Step 4. Render call sites with context

For each `who_uses` hit:

```
read_file_content(file_path=<file>)
```

Slice `[start - 200, end + 200]` for context.

### Step 5. RA cross-reference (catches things `who_uses` misses)

```
find_references(symbol_name=<short_name>)
```

`find_references` is broader scope — local var refs, lifetime annotations,
and other RA-tracked things. `who_uses` is structural and aggregated. Use
both when verifying "is X really unused?".

### Step 6. Cross-crate fan-in summary

Group `who_imports` by consumer crate:

| Consumer crate | Importer count |
|---|---|
| crate_a | 3 |
| crate_b | 1 |

A symbol with many crates importing it is widely-used; refactor with care.

### Step 7. Method-level fan-in (Layer 4 unlocks)

If Y is a type, walk its methods from `module_tree` and run:

```
who_uses(directory=..., target=Y::method)
```

Pre-Layer-4 these queries errored. Post-Layer-4 they return real results,
including trait dispatch.

## Decision frames

| Finding | Verdict |
|---|---|
| `who_uses` empty + `who_imports` empty + `find_references` empty | Safe to delete |
| `who_uses` empty + `who_imports` non-empty | Imported but never referenced — possibly used as a generic bound; investigate |
| `who_uses_summary` 100% Test | Test fixture; demote or `#[cfg(test)]` |
| `who_uses_summary` 100% Other | Critical path; high refactor risk |
| Single consumer module | Tightly coupled to one place; consider co-locating |
| Many consumer crates | Workspace-shared API; avoid breaking changes |

## Pattern reference

| Signal | Means |
|---|---|
| `who_uses` empty but `find_references` populated | Symbol used in a context the hypergraph doesn't index (macro-introduced, cfg-gated) |
| `who_uses` Read >> Write | Read-mostly API — encapsulation healthy |
| `who_uses` Write-heavy | Diffuse invariants; many writers means brittle state |
| `who_imports` count >> `who_uses` count | Symbol imported for trait bounds / re-export only |
| Same module appears in both `who_imports` and `who_uses_summary` with high Test ratio | Local fixture builder |

## Output format

Structured findings, often a single verdict per invocation:

```
Symbol: <qualified_name>
Declaration: <file:line>
Importers: <n> across <m> crates
Non-import refs: <n> across <m> modules (<Test>%/<Other>%)
RA refs: <n>
Verdict: <safe-to-delete | test-only | critical-path | shared-API | imported-as-bound>
Recommended action: <...>
```

If invoked on a type, append per-method fan-in rows.

## Limitations

- Method visibility is null on the Item Node — Layer 4 doesn't attach
  Declared bindings to methods. Read `Node.file + Node.span` for source
  to determine method visibility.
- Trait dispatch through `dyn T` may miss some sites; the resolver is
  type-based. For trait-aware dispatch tracing, use Layer 10 via
  `rmc-call-graph`.
- Macro-expanded refs sometimes don't surface in `who_uses` — fall back
  to `find_references` and inspect.
- `who_uses` on a popular trait can return thousands of rows; there is no
  pagination cursor today.
