---
name: rmc-imports-exports
description: Audit Rust cross-crate imports and exports.
argument-hint: "[crate-name] [workspace-path]"
allowed-tools: Read, Bash, mcp__rust-code-mcp__*
---

# Rust imports / exports audit

Audit a Rust workspace's import/export structure: who exports what, who
consumes what, where the public surface is over- or under-broad, and which
`pub` items are dead.

Two scopes:
1. **Workspace-wide** — every crate in one pass. Use for inheriting a
   codebase, comparing branches, or pre-refactor architecture audits.
2. **Single-crate** — one crate as a deep dive. Use for onboarding to a
   subsystem, verifying encapsulation discipline, or pre-refactor focus.

For naming-collision hygiene (type overlaps, module shadows), use
`rmc-type-overlaps`. For per-symbol forensics on a hot symbol, use
`rmc-symbol-forensics`. For sortable per-crate Martin metrics in isolation,
use `rmc-dependency-metric`.

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

If the schema bumped or sources changed, this rebuilds. Otherwise reuse is
sub-second.

---

## Scope 1 — workspace-wide

### Step 1. Foundation data (parallel)

Pull these three together — they're independent reads:

```
workspace_stats(directory=...)         → counts (nodes, items by kind, visibility)
crate_edges(directory=...)             → full consumer→producer matrix
dead_pub_report(directory=...)         → workspace-wide dead-pub findings
```

### Step 2. Read the architectural shape

From `crate_edges`, sort consumer→producer edges by
`total_refs_via_imports + total_refs_via_usages`:

- The producer with the highest fan-in is the "universal types" crate
  (e.g., `domain`, `core`, `model`).
- The consumer with the highest fan-out is the most coupled crate.
- Crates with zero fan-in are leaf libraries OR the binary crate
  (distinguish by checking for a `main` fn or `[[bin]]` target).
- Look for cycles. There shouldn't be any.

Aggregate the matrix into per-producer fan-in and per-consumer fan-out
totals. The shape tells you whether your crates form a clean DAG or a
coupled mesh.

### Step 3. Per-producer crate — declared vs effective surface

For each producer crate `X`:

```
get_declared_reexports(directory=..., module=X)   → declared `pub use` at root
```

Cross-check against `dead_pub_report`:

- Items in `dead_pub_report` AND in declared_reexports → **dead facade**:
  re-exports that nothing imports. Drop the `pub use` lines, demote the
  source types.
- Items in `dead_pub_report` NOT in declared_reexports → **dead canonical
  pubs**: declared `pub` at home, never imported. Demote to `pub(crate)`.
- Items in declared_reexports NOT in `dead_pub_report` → **live facade**:
  doing its job. Keep.

Optional, when visibility filtering matters:

```
get_exports(directory=..., module=X, consumer=Y)  → what consumer Y actually sees
```

### Step 4. Per-consumer crate — list dependencies

Filter `crate_edges` rows by `consumer_crate=Y`:

- The producer set tells you Y's external dependencies.
- The `unique_symbols` field per edge says how broad each dependency is.
- The `total_refs_via_usages` field says how heavy the dependency is.

High `unique_symbols` on one producer = wide coupling (uses many parts).
High `total_refs_via_usages` on a single symbol = narrow but heavy coupling
(one type drives everything).

### Step 5. Drill into hot symbols

For symbols with the highest `import_count + usage_count` in any edge:

```
who_uses_summary(directory=..., target=<qualified_name>)
```

Returns rows grouped by consumer module with Test/Other category breakdown:

- All-Test rows → fixture builder, candidate for `#[cfg(test)]` or factoring
  into dev-deps.
- All-Other rows → critical-path symbol, refactor with care.
- Mixed → standard public API.

### Step 6. Sortable Martin metric (optional, complements `crate_edges`)

```
crate_dependency_metric(directory=..., sort_by="instability", top_n=10)
```

Returns per-local-crate Ca (afferent), Ce (efferent),
`instability = Ce / (Ce + Ca)`, abstractness. `sort_by` accepts
`instability` / `item_count` / `afferent` / `efferent` / `abstractness`.

---

## Scope 2 — single crate

### Step 1. Crate-level snapshot (parallel)

```
module_tree(directory=..., krate=X, depth=2)        → top-level module structure
get_declared_reexports(directory=..., module=X)     → declared `pub use` at root
get_imports(directory=..., module=X)                → what's imported at the root
dead_pub_in_crate(directory=..., krate=X)           → pub items with no cross-crate consumer
```

### Step 2. Characterize the public surface

Cross-tabulate three sets at the crate root:

| In `module_tree` (visibility=pub) | In `get_declared_reexports` | In `dead_pub_in_crate` | Meaning |
|---|---|---|---|
| ✓ | – | – | canonical pub, live |
| – | ✓ | – | re-export, live (facade) |
| – | ✓ | ✓ | dead re-export — drop the `pub use` |
| ✓ | – | ✓ | dead canonical pub — demote to `pub(crate)` |
| ✓ | ✓ | – | re-exported AND canonical (rare) |

Look at `pub(in crate_name)` items at the root level — these are
crate-internal coordination helpers. A healthy crate has discipline:
external API as `pub`, internal API as `pub(in crate)`, hidden as
default-private.

### Step 3. Outgoing and incoming dependencies

If you have workspace `crate_edges` cached, filter:

- `consumer_crate=X` → outgoing dependencies (what X consumes)
- `producer_crate=X` → incoming dependencies (who consumes X)

Otherwise call `crate_edges(directory=...)` and filter client-side.

A crate with one consumer is single-purpose; multiple consumers means it's
a shared library. Single producer dependency means strong coupling to one
upstream.

### Step 4. Confirm canonical types are alive

For each non-dead pub item at the crate root:

```
who_uses_summary(directory=..., target=X::Type)
```

The category breakdown distinguishes:

- All-Test → demote or wrap in `#[cfg(test)]`.
- All-Other → critical-path, refactor with care.
- Mixed → legitimate API.
- Empty → either covered by a re-export elsewhere OR genuinely dead
  (cross-check with `who_imports`).

### Step 5. Examine the entry-point story

Identify entry-point functions (often `run`, `start`, `main_loop`). Run
`who_uses_summary` on each:

- One caller = single integration point. Clean.
- Many callers = utility, not entry point.
- Zero callers = either a binary's `main` or genuinely dead.

### Step 6. Method-level analysis (Layer 4)

For key types, walk their methods from `module_tree` and check fan-in:

```
who_uses_summary(directory=..., target=X::Type::method)
```

Methods with empty `who_uses` are dead-method candidates. Pre-Layer-4
these queries returned empty; post-Layer-4 they return real results, so
this step is now meaningful.

---

## Decision frames

| Finding | Action |
|---|---|
| Item in `dead_pub_report` not used by anyone | Demote to `pub(crate)` or delete |
| Re-export in `get_declared_reexports` whose target is also dead | Drop the `pub use`, demote the source |
| Producer X with multiple consumers using overlapping symbols | Candidate for shared interface extraction |
| Consumer Y pulling deeply from producer X (high unique_symbols + total_refs) | Tight coupling — consider merging or introducing a smaller interface |
| Crate X re-exports many items from crate Y (facade pattern) | Verify intent — may be over-broad if Y is also imported directly |
| Edge with `total_refs_via_usages` >> `total_refs_via_imports` | Heavy use through trait dispatch / methods (Layer 4-aware) |
| Single-consumer crate with broad API | Suspicious — consumer probably doesn't need all of it |
| Methods with empty `who_uses` | Verify (may be dispatched via trait); demote if unused |

## Pattern reference

| Signal | Pattern |
|---|---|
| Producer fan-in skewed to 1-2 type crates | Healthy DAG with universal types |
| Producers and consumers nearly symmetric | Coupled mesh |
| One crate with high fan-in AND high fan-out | Hourglass; bottleneck |
| Many `pub use` from one submodule | That submodule is the de-facto crate API |
| `dead_pub_in_crate ∩ get_declared_reexports` non-empty | Dead facade re-exports |
| `pub(in <crate>)` items in module_tree | Healthy internal-API discipline |

## Output format

Severity-ranked findings table:

```
🔴 High    — broken or contradictory state (e.g., type duplicated across
            crates with same consumer, dead canonical pub still referenced
            by a stale pub use)
🟡 Medium  — wasted surface or namespace overload
🟢 Low     — naming clarity, mechanical refactors
⚪ Info    — confirms healthy structure
```

A clean output is a markdown table per finding with: severity, location
(qualified name + file:span where available), what's wrong, recommended
action.

## Worked examples

### Workspace example — `coding-agent-bad`

17 crates, 1441 fan-in on `domain` (universal types), `agent` re-exports
11 `permissions::*` types as a facade even though direct imports of
`permissions` work (over-broad facade), `tools` has 13 dead-pub `*Tool`
types likely dispatched via a trait registry. Top severity finding:
half-finished migration around `AgentConfig` with the type duplicated in
both `agent` and `config` crates.

### Single-crate example — `tui` in `coding-agent-bad`

15 submodules, single entry point `tui::run` (one caller in
`coding-agent::interactive`), 7 dead pubs of which 3 are dead re-exports
at the crate root, sensible `pub(in tui)` discipline for crate-internal
helpers. Cleanup is small: drop dead re-exports, demote source types.

## Limitations

- `crate_edges` excludes method calls / trait dispatch from usage counts
  (Layer 4 hooks them in for `who_uses` queries, but not in edge totals).
- `dead_pub_in_crate` is computed from cross-crate consumers only —
  within-crate usage in tests/examples still flags as dead.
- `who_uses` on a popular trait can return thousands of rows; no
  pagination cursor today.
- Glob imports / renamed imports surface partially — explicit imports are
  best for accurate fan-in.
