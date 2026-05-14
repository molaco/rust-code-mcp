---
name: rmc-reexport-chain
description: Trace Rust pub use re-export chains.
argument-hint: "<qualified-name-or-crate>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust re-export chain tracing

Trace `pub use` chains through facade modules. Pair with the `pub type`
audit to catch aliases masquerading as re-exports. Scope: single Item
with a long re-export chain, or a crate's facade audit.

For broader public-surface audits, use `rmc-api-surface`. For dropping
dead facade re-exports, use `rmc-refactor-plan` (Recipe 12).

## Scope — single Item or single crate

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Trace a chain

```
re_export_chain(directory=..., target=<crate>::module::Y)
```

Walks every `pub use` re-export of the canonical target up to 8 hops
with cycle detection, breadth-first. Returns `links` (one per visited
binding) with `from_module`, `visible_name`, and `depth`. Useful for
auditing the public surface — "this Item is exposed at facade depth 4?
do we need that?"

### Step 2. Detect pub-type masquerading as re-export

```
pub_use_pub_type_audit(directory=..., crate_name=X)
```

Returns every `pub type` alias in the named crate whose owning module
also carries a `pub use ... as <alias_name>` (or `pub use ::<alias_name>`)
binding. Indicates the alias may be acting as a re-export disguised as a
`pub type` declaration. The model does NOT record what the alias's RHS
resolves to, so the heuristic can't confirm — verify with
`find_definition(symbol_name=<alias>)` before acting.

## Recipes

### Recipe — "Decode a long facade chain"

When an Item is exported via 3+ hops, callers face a guess-the-canonical-path
problem. `re_export_chain(target=<canonical>)` shows each step:

```
re_export_chain(target=domain::auth::AuthError)
  → links: [
      { from_module: "domain", visible_name: "AuthError", depth: 1 },
      { from_module: "shared", visible_name: "AuthError", depth: 2 },
      { from_module: "agent",  visible_name: "AuthError", depth: 3 }
    ]
```

This reveals which crate facades pin the visibility. Combine with
`who_imports(target=domain::auth::AuthError)` to see which facade path
consumers actually use — if everyone reaches for the canonical, drop the
facades.

### Recipe — "Crate facade hygiene"

```
pub_use_pub_type_audit(crate_name=X)
```

Surfaces `pub type Y = path::Y;` that should be `pub use path::Y;`. The
aliases keep the public name but introduce an extra type-level indirection
that `pub use` would express more directly. Convert per finding:

```
// Before:
pub use foo::FooImpl;
pub type Foo = foo::FooImpl;   // <-- the audit flags this

// After (one of):
pub use foo::FooImpl as Foo;
// or keep the `pub use FooImpl;` and drop the alias.
```

## Decision frames

| Situation | Action |
|---|---|
| `pub type Y = Path;` where Y has no generic shape change | Should be `pub use Path as Y;` — drop the alias |
| `pub type Y<T> = Path<T, DefaultParam>;` (shape-changing) | Correct as `pub type` — keep |
| Re-export chain depth ≥ 4 | Audit each hop; consumers usually skip to the canonical |
| Re-export chain depth = 1 | Single facade — fine |
| Pub-type-audit hit with no matching `pub use` after verifying | False positive (different RHS); ignore |

## Pattern reference

| Use case | Invocation |
|---|---|
| Trace one Item's facade exposure | `re_export_chain(target=Y)` |
| Audit a crate's pub type aliases | `pub_use_pub_type_audit(crate_name=X)` |
| Verify alias RHS | `find_definition(symbol_name=<alias>)` after the audit |
| Find which facade hop consumers actually use | `who_imports(target=<canonical>)` |

## Output format

```
Target: <qualified_name>
Chain depth: <n>
Links:
  1. <from_module>::<visible_name> (depth 1)
  2. <from_module>::<visible_name> (depth 2)
  ...

Consumers by hop (from who_imports):
  Hop 1: <n> imports
  Hop 2: <n> imports
  Canonical: <n> imports

Recommendation: <keep all hops | drop hops {2,3} | flatten to canonical>
```

For the audit mode:

```
Crate: <X>
pub type/pub use overlaps: <n>
Per finding: <alias>, <conflicting pub use line>, verify-with-find_definition
```

## Limitations

- `pub_use_pub_type_audit` is heuristic: compares alias name to `pub use`
  bindings declared in the same module; can't confirm the RHS resolves
  to the same target. False positives when alias and `pub use` share a
  name but point to different types.
- `re_export_chain` walks up to 8 hops with cycle detection. Beyond 8,
  deeper chains aren't enumerated — increase if you have a workspace
  with extreme facade depth (rare).
- The chain only follows `pub use` re-export edges; `pub use *` glob
  re-exports are followed as well, but glob-of-glob chains may surface
  additional bindings depending on the resolver.
