---
name: rmc-api-surface
description: Audit a Rust crate's public API.
argument-hint: "<crate-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust API surface audit

Catch over-broad facades, accidental internals exposure, and dead public
surface. Scope: single crate.

For workspace-wide imports/exports, use `rmc-imports-exports`. For
refactor-specific recipes (drop facade, demote pub), use `rmc-refactor-plan`.

## Scope — single crate

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Pull the surface (parallel)

```
get_declared_reexports(directory=..., module=<crate_root>)
get_exports(directory=..., module=<crate_root>, consumer=<external_crate>)
module_tree(directory=..., krate=<crate>)            → filter to pub items
dead_pub_in_crate(directory=..., krate=<crate>)
```

### Step 2. Build the four-way table

| Source | Meaning |
|---|---|
| `module_tree` (visibility=pub) | What's `pub` at canonical site |
| `get_declared_reexports` | What's re-exported via `pub use` at the crate root |
| `get_exports(consumer=<other>)` | What's actually visible from outside (visibility-filtered) |
| `dead_pub_in_crate` | Pub items with no cross-crate consumer |

### Step 3. Detect facade-vs-canonical traffic

For each declared re-export, look up canonical-path traffic:

```
who_imports(directory=..., target=<canonical_path>)
```

Most importers reaching for the canonical path means the facade isn't
being used. Drop it.

### Step 4. Detect accidentally-exposed internals

Items in `get_declared_reexports` that the team thought were `pub(crate)`.
These usually slip in via `pub use submodule::*` patterns.

### Step 5. Detect pub items hiding behind a facade that don't need to be pub

If a `pub use` chain can become `pub(crate) use`, the source can be
`pub(crate)`. `dead_pub_in_crate` already finds these.

### Step 6. Empty results as signals

| Empty result | Means |
|---|---|
| `get_declared_reexports([])` | Crate has no facade — everything at canonical paths. Intentional design. |
| `dead_pub_in_crate([])` | No dead pubs — disciplined. |
| `overlaps.common_fn_names([])` | No `init` / `run` proliferation — good hygiene. |

## Decision frames

| Finding | Action |
|---|---|
| Re-export declared at root, target dead in `dead_pub_in_crate` | Drop `pub use`, demote source |
| Pub item at canonical site, dead in `dead_pub_in_crate` | Demote to `pub(crate)` |
| Item in `get_declared_reexports` that looks crate-internal | Probably accidentally exposed; demote |
| Re-export AND canonical declaration of same item | Pick one path; drop the other |
| `get_exports(consumer=X)` smaller than `get_declared_reexports` | Visibility filter is trimming `pub(crate)` / `pub(in <crate>)` items (intentional discipline) |

## Pattern reference

| Signal | Means |
|---|---|
| `pub use foo::*` at crate root | Likely over-broad facade; audit each item |
| Crate's `get_declared_reexports` ⊆ `dead_pub_in_crate` | Entire facade is dead — drop wholesale |
| `get_exports(consumer=X)` smaller than `get_declared_reexports` | Visibility filter trimming `pub(crate)` / `pub(in <crate>)` items |
| Every facade item has `who_imports` only on the facade path | Facade is earning its keep |
| Every facade item has `who_imports` only on canonical path | Facade is dead weight |

## Output format

```
Crate: <crate>
Declared re-exports: <n>
Effective exports to <consumer>: <m>
Dead pubs: <k>
Dead facade re-exports (intersection): <j>

Recommended:
  - Drop pub use for: <list>
  - Demote to pub(crate): <list>
  - Already healthy: <list of items earning their pub keyword>
```

Severity-ranked findings:

```
🔴 High    — Crate-internal type accidentally exposed via pub use
🟡 Medium  — Dead facade re-export
🟡 Medium  — Dead canonical pub
🟢 Low     — Over-broad pub use glob (audit each item)
⚪ Info    — Healthy facade; intentional no-facade design
```

## Worked example — `tui` in `coding-agent-bad`

15 submodules. 7 dead pubs of which 3 are dead re-exports at the crate
root (`RunState`, `InvalidTransition`, `RunnerWakeError` — all
re-exported AND dead). Sensible `pub(in tui)` discipline for
crate-internal helpers. Cleanup: drop three `pub use` lines, demote
source types to `pub(crate)`.

## Limitations

- `get_exports(consumer=X)` requires X to be a valid crate name in the
  workspace. Pass a non-existent name and you get an empty result, not
  an error.
- `pub use submodule::*` glob re-exports surface as one row per
  re-exported item, not one row per glob — verify by reading source if
  the glob is the actual smell.
- `dead_pub_in_crate` counts cross-crate consumers only; within-crate
  test usage still flags as dead.
