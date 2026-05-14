---
name: rmc-workspace-overview
description: First-look audit of a Rust workspace.
argument-hint: "[workspace-path]"
allowed-tools: Read, Bash, mcp__rust-code-mcp__*
---

# Rust workspace overview

First-look recipe. Use when inheriting a codebase, comparing branches, or
starting any deeper audit. Scope: workspace-wide (the whole point).

For single-crate deep dives, hand off to `rmc-crate-audit`. For architectural
rule enforcement, hand off to `rmc-architecture-rules`. For complexity-led
refactor prioritization, hand off to `rmc-complexity`.

## Scope — workspace-wide

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
index_codebase(directory=<absolute-path>)   # required for semantic_overlaps in Step 10
```

Cold builds traverse all crates via rust-analyzer. Warm runs return
`reused: true` in sub-second time.

## Workflow

### Step 1. Foundation (parallel)

Five independent reads — issue them in one round:

```
workspace_stats(directory=...)
crate_edges(directory=...)
dead_pub_report(directory=...)
overlaps(directory=...)
health_check()
```

### Step 2. Read `workspace_stats` for shape

Key fields:

- `nodes_by_kind` — workspace / crate / module / item / external_symbol counts
- `items_by_kind` — Struct / Enum / Fn / Method / Trait / TypeAlias / Const / Static / AssocConst / AssocType / Impl distribution
- `bindings_by_kind` — Declared / NamedImport / GlobImport / ExternCrateImport
- `visibility` — pub / pub_crate / restricted_to / private counts
- `pub_crate_share` — ratio of pub_crate to (pub + pub_crate)

`pub_crate_share` is the single most useful between-codebase comparison
metric. Measured 0.07 on a coupled mesh and 0.58 on its rewrite — same
crate count, very different encapsulation discipline.

### Step 3. Read `crate_edges` for architectural shape

Aggregate the matrix:

- **Per-producer fan-in** — `producer_crate` summed across all consumers
- **Per-consumer fan-out** — `consumer_crate` summed across all producers
- **Top-N edges** by `total_refs_via_imports + total_refs_via_usages`

Annotate the architecture:

- Highest fan-in producer → "universal types" crate (`domain` / `core` / `model`).
- Highest fan-out consumer → most coupled crate, often the binary or main app.
- Zero fan-in crates → leaf libraries OR the binary.
- Walk for cycles (there shouldn't be any).

If `crate_edges` is large (> 50KB), it persists to a tool-results file.
Post-process with Bash + jq/Python.

### Step 4. Read `dead_pub_report` for rot

`dead_pub_report.crates[].crate` is the canonical crate enumeration — the
most reliable list of every workspace crate, including those with zero
findings.

Vendored / library-style crates have inflated dead-pub counts (their pub
surface is "designed for general use" but consumed narrowly here). Filter
known external/vendored crates before reading.

### Step 5. Read `overlaps` for hygiene

Four buckets:
- `cross_crate_type_collisions`
- `module_shadows`
- `within_crate_type_duplicates`
- `common_fn_names` (4+ crates)

Empty `common_fn_names` is the good sign — no `init` / `run` proliferation.
Hits worth investigating: anything other than `main` or core idioms (`new`,
`default`). For collision deep-dives, hand off to `rmc-type-overlaps`.

### Step 6. Spot the gnarl

Pick the heaviest fan-in crate's main files (from Step 3 ranking) and run:

```
analyze_complexity(file_path=<path>)
```

per file. Compare `Total cyclomatic` and `Avg per function`. Cross-reference
top files with `who_uses_summary` for blast radius. For deeper complexity
work, hand off to `rmc-complexity`.

### Step 7. Output snapshot

One-page summary:

```
Crates: <n>
Items: <n>; method count <n>; struct count <n>; trait count <n>
Visibility: pub <n> / pub_crate <n> (share <r>)
Top fan-in producer: <crate> with <n> total_refs in
Top fan-out consumer: <crate> with <n> total_refs out
Dead pubs: <n> across <m> crates (excluding vendored: <n>)
Hygiene: <k> cross-crate collisions, <l> module shadows, <m> within-crate dupes
Gnarl: top complex fns in <crate>
```

### Step 8. Unsafe surface

```
unsafe_audit(directory=...)
```

Every `unsafe { ... }` block in local crates with `file`, `span`, `line_count`,
`enclosing_function_name`, and `has_safety_comment` flag. Filter to
`has_safety_comment=false` for "any undocumented unsafe?". Empty is the
healthy signal. Cross-reference each finding's `enclosing_function_name`
with `recursive_callers_count` to weight risk by blast radius — full recipe
in `rmc-unsafe-audit`.

### Step 9. Global mutable state

```
mut_static_audit(directory=...)
```

Every local `static` whose HIR type matches `static mut` / `LazyLock<...>` /
`OnceLock<...>` / `OnceCell<...>`. Inventory check: how many process-global
mutables does this workspace carry? `LazyLock<Mutex<...>>` and `OnceCell<...>`
are usually the ones to scrutinize ("should this be DI'd?"). `static mut`
matches are FFI / legacy hot spots. `lazy_static!` macro is NOT detected.
Deeper recipe in `rmc-mut-static-audit`.

### Step 10. Literal duplicates

```
semantic_overlaps(directory=..., threshold=0.95)
```

`threshold=0.95` (plus the v1.1c content-hash short-circuit at similarity 1.0)
surfaces source-byte duplicates: same enum variant pasted across error
enums, same trivial helper struct redeclared per crate. Top clusters are
dead-easy refactor wins because there is nothing to harmonize — the source
is identical. Deeper recipe in `rmc-semantic-overlaps`.

### Step 11. Optional — architectural rules

If the workspace claims a layered architecture, codify the layer rules:

```
forbidden_dependency_check(directory=..., rules=[
  { consumer: "domain*", producer: "tokio", severity: "error" },
  { consumer: "domain*", producer: "reqwest", severity: "error" },
  { consumer: "domain*", producer: "serde_json", severity: "warn" },
])
```

Empty `violations` is the pass signal. Full recipe in
`rmc-architecture-rules`.

## Decision frames

| Finding | Means |
|---|---|
| `pub_crate_share` < 0.2 | Low encapsulation discipline — bare `pub` everywhere |
| Highest fan-in crate is `core` / `domain` / `model` | Healthy DAG with universal types |
| Highest fan-in crate is utility/helper-ish | Possible god-crate, candidate for split |
| Cycles in `crate_edges` | Architectural break — investigate before any other audit |
| Dead pubs concentrated in one crate | Either vendored lib or facade rot |
| `overlaps.common_fn_names` non-empty | Possible missing trait abstraction |
| Many undocumented unsafe blocks in hot path | High-risk; add SAFETY comments or break apart |
| Many `LazyLock<Mutex<...>>` singletons | Hidden global state; review for DI candidates |
| Literal-duplicate clusters in `semantic_overlaps` | Easy refactor wins (similarity = 1.0) |

## Pattern reference

| Signal | Pattern |
|---|---|
| Healthy DAG | Producer fan-in skewed toward 1-2 type crates; consumer fan-out skewed toward 1-2 binary/integration crates |
| Coupled mesh | Producers and consumers nearly symmetric; high `unique_symbols` per edge |
| Hourglass | One crate has both high fan-in and high fan-out (it's a bottleneck for everything) |
| Empty `common_fn_names` | Healthy discipline — no `init`/`run` proliferation |
| Empty `dead_pub_report` for a crate | Either fully consumed externally or `pub(crate)`-disciplined |
| Zero unsafe + zero static mut | Clean memory-safety surface |

## Output format

Severity-ranked findings table on top of the one-page summary in Step 7:

```
🔴 High    — broken or contradictory state (cycle in crate_edges,
            undocumented unsafe in hot path, half-finished type migration)
🟡 Medium  — wasted surface (concentrated dead-pubs, low pub_crate_share,
            unmaintained facade re-exports)
🟢 Low     — naming clarity, mechanical refactors (literal duplicates,
            test-fixture duplicates)
⚪ Info    — confirms healthy structure (clean unsafe surface, empty
            common_fn_names, balanced fan-in/fan-out)
```

## Worked example — `coding-agent-bad`

17 crates. `pub_crate_share` low (many bare `pub` — 🟡). Top fan-in
`domain` with 1441 `total_refs` in (universal types crate). Top fan-out
`agent`. 89 dead pubs across the workspace; 47 in `plurimus` (vendored UI
lib — exclude). 5 cross-crate collisions, 1 module shadow, 6 within-crate
duplicates. `common_fn_names` empty (⚪ good). `unsafe_audit` empty (⚪).
`mut_static_audit` surfaced a handful of `LazyLock` singletons in the agent
crate (🟡 worth a DI review). `semantic_overlaps(threshold=0.95)` found six
1.0-similarity clusters of the unit `Error` variant duplicated across
`ToolResultKind` / `StopReason` / `FinishReason` enums (🟢 mechanical
refactor).

## Limitations

- `crate_edges` does not include method calls / trait dispatch in usage
  counts — coupling figures undercount for trait-heavy designs.
- `mut_static_audit` misses `lazy_static!` macro expansions; combine with
  grep for that case.
- `analyze_complexity` returns file-level aggregates, not per-fn scores;
  for per-fn ranking use `rmc-complexity` workaround.
- Vendored crates inflate dead-pub counts — always check before acting on
  `dead_pub_report` headline numbers.
