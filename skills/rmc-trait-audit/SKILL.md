---
name: rmc-trait-audit
description: Audit a Rust trait.
argument-hint: "<trait-qualified-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust trait audit

Layer 4 sweet spot. Trait declarations and their methods are first-class
graph nodes; `x.method()` and `Type::method()` resolve back to the trait
declaration. Scope: a single trait T.

For non-trait single-symbol analysis, use `rmc-symbol-forensics`. For
workspace-wide dispatch tracing, use `rmc-call-graph`. For test-only vs
production splits, use `rmc-test-vs-prod`.

## Scope — single trait T

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Locate the trait

```
find_definition(symbol_name=T)                          → file:line
module_tree(directory=..., krate=<crate>)               → walk to T, expand methods
```

### Step 2. Identify trait methods

From `module_tree`, the trait Item has children: methods, assoc consts,
assoc types. List them.

### Step 3. Fan-in per method (parallel)

For each method `M` on T:

```
who_uses_summary(directory=..., target=<crate>::T::M)
```

Sort results by `total_count` desc.

### Step 4. Trait-level fan-in

```
who_imports(directory=..., target=<crate>::T)
```

Modules that import `T` typically either implement it or take it as a
generic bound.

### Step 5. Layer 10 trait dispatch tracing

For each method M:

```
who_calls(directory=..., target=<crate>::T::M)
```

`who_calls` returns every fn body that contains a call resolving back to
`T::M`. Pre-Layer-10 these queries either errored or returned only same-fn
references; post-Layer-10 they return workspace-wide call sites attributed
to the enclosing fn — including dispatch through generic bounds and `dyn T`
receivers. Pair with the Step 3 results to sort methods by total call-site
count and to spot the method that dominates dispatch traffic (often the
"real" core of the abstraction).

### Step 6. Trait deletion / sealing check

| Pattern | Verdict |
|---|---|
| `who_uses(T)` empty across all crates outside the defining one | Safe to delete or seal |
| `who_uses(T::M)` empty for some method M | Safe to remove M (verify trait impls aren't hardcoding it) |
| Single importer + single implementer | Trait is doing nothing — inline |
| Multiple implementers + multiple consumers | Real abstraction boundary; keep |

### Step 7. Single-implementation audit (rust-guidelines §8)

For each `pub trait` in the crate's `module_tree`:

```
who_imports(directory=..., target=<crate>::T)
```

If importer count is 1 and the trait isn't a `Send`/`Debug`-style
supertrait, it's a candidate for inlining. The trait has one job: hide an
impl that nothing else substitutes — usually deletable.

## Decision frames

| Finding | Action |
|---|---|
| Trait with one impl + one consumer | Inline; delete the trait |
| Trait with one impl + multiple consumers | Probably needed for substitution; verify generic param actually flows |
| Trait method with empty `who_uses_summary` | Remove the method (verify impls don't keep it for hardcoded reasons) |
| Trait imported by many crates, methods used by few | Probably a generic-bound trait; safe |
| Trait method has all-Test fan-in | Test-only trait method; demote to `#[cfg(test)]` |
| One method dominates `who_calls` traffic | Core method; treat others as candidate-thin API |

## Pattern reference

| Signal | Means |
|---|---|
| `who_uses(T::M)` resolves to many call sites in unrelated crates | Trait is genuine substitution boundary |
| Same call site for `T::M` and a single concrete `Type::M` | Trait dispatch may be vestigial; check if generic param flows through |
| `who_imports(T)` count >> `who_calls(T::M)` aggregate | T is used mostly as a generic bound, not for dispatch |
| Many associated consts/types but few methods | T is a marker/type-witness trait, not a behavior boundary |

## Output format

```
Trait: <crate>::T
Methods: <n> (with fan-in: <m>; dead: <k>)
Implementers: <n>
Importers: <n>
Top method by dispatch: T::<M> with <n> call sites
Verdict: <delete | seal | inline | keep | thin-out>
Recommended action: <...>
```

Per-method table:

| Method | who_uses_summary count | who_calls count | Test % | Verdict |
|---|---|---|---|---|

## Limitations

- Trait impl enumeration (every concrete `impl T for Type` body) is
  deferred (Layer 4c). `who_calls(T::M)` finds dispatched call sites;
  `functions_with_filter(self_kind=...)` audits method-shape consistency.
  Enumerating impl blocks as graph entities is not yet possible.
- Method visibility is null on Item Node; read source for visibility.
- Trait dispatch through `dyn T` may miss some sites; resolver is
  type-based.
- `who_uses` on a popular trait can return thousands of rows; no
  pagination cursor today.
