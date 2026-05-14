---
name: rmc-method-api
description: Audit a Rust type's methods.
argument-hint: "<type-qualified-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust method-API audit

Layer 4 nests methods, assoc consts, and assoc types as children of their
host types in `module_tree`. This unlocks per-method analysis that
pre-Layer 4 errored. Scope: a single type and its method API surface.

For trait-specific dispatch analysis (across all impls), use
`rmc-trait-audit`. For single-symbol forensics, use `rmc-symbol-forensics`.
For test-vs-prod splits on a method family, use `rmc-test-vs-prod`.

## Scope — single type (and its method API surface)

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Pull the type's full surface

```
module_tree(directory=..., krate=<crate>, depth=4)
```

Walk to the type. Children are methods, assoc consts, assoc types. Their
`kind` field disambiguates: `Method`, `AssocConst`, `AssocType`.

### Step 2. Per-method fan-in (parallel)

For each child:

```
who_uses_summary(directory=..., target=<crate>::<path>::<Type>::<method>)
```

Run all in parallel (independent reads). Sort by `total_count` desc.

### Step 3. Read the breakdown

| Pattern | Verdict |
|---|---|
| Empty `who_uses` | Dead method. Layer 4 finally surfaces these. |
| All-Test rows | Test-only helper |
| All-Other rows | Critical path |
| Mixed | Legitimate API |

### Step 4. Inherent vs trait method distinction

`module_tree` shows both as children. Their `parent_id` differs:

- Inherent method → parent is a struct/enum Item.
- Trait method → parent is a Trait Item OR an `impl Trait for Type` Item.

For trait dispatch, `who_uses` resolves back to the trait declaration,
not the impl. To find concrete impl callers, search by the impl's
qualified name (Layer 4 nests these).

### Step 5. Method-naming consistency check

Scan `module_tree` outputs for naming patterns:

- Every constructor `new` vs some `from` / `create` / `with`?
- Error type conversions: `from_io`, `from_parse`, etc., consistent?
- Mutators: `set_*` vs `update_*` vs bare verbs?

Subjective but worth noting in code review.

### Step 6. Function-level call graph (within file)

Layer 4 doesn't unlock cross-file fn-to-fn graphs. For within-file flow:

```
get_call_graph(file_path=<path>)
```

Parser-based; gives function-to-function edges within one file. Use as a
complement to method-level usages across files. For workspace-wide
fn→fn analysis, use `rmc-call-graph`.

## Decision frames

| Finding | Action |
|---|---|
| Method with empty `who_uses_summary` | Verify (may be dispatched via trait); demote or delete |
| Method on trait with empty `who_uses` | Either trait method is dead OR all dispatch goes through `Type::method` directly |
| Method on impl block named `new` with all-Test consumers | Test-only constructor; gate with `#[cfg(test)]` |
| Method-naming inconsistency across types | Style cleanup, low priority but easy |
| One method dominates fan-in | That's the "real" API; others may be candidate-thin |

## Pattern reference

| Signal | Means |
|---|---|
| `who_uses(Type::method)` empty pre-Layer-4, populated post-Layer-4 | Layer 4 successfully surfaces method calls |
| Trait method has more `who_uses` than any impl method | Dispatch is mostly trait-level (good substitution) |
| Trait method has fewer `who_uses` than concrete impl methods | Most callers go through concrete types — trait may be vestigial |
| Many `with_*` constructors all with low fan-in | Builder-pattern variants — consider consolidating |

## Output format

Per type:

```
Type: <crate>::<path>::<Type>
Methods: <n> (inherent: <i>, trait: <t>)
Assoc consts: <c>; Assoc types: <a>
Dead methods: <k> (list below)
Top method by fan-in: <Type>::<m> with <n> callers
```

Per-method table:

| Method | Kind | total_count | Test % | Verdict |
|---|---|---|---|---|

## Limitations

- Method visibility is null on the Item Node — Layer 4 doesn't attach
  Declared bindings to methods. Read `Node.file + Node.span` for source
  to determine method visibility.
- `who_uses` on a trait method resolves to the trait declaration; concrete
  impl callers must be queried via the impl's qualified name.
- Trait dispatch through `dyn T` may miss some sites; resolver is
  type-based. Use `rmc-call-graph` Layer 10 for dispatch tracing.
- Inherent impls of foreign types (methods on dep-crate types) aren't
  extracted by the indexer.
