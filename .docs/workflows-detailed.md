# Workflows — detailed reference

Expanded specifications for every workflow surfaced in `.docs/workflows.md`. Each entry follows the same template as `workflow-imports-exports.md` and `workflow-type-overlaps.md`: brief description, scopes (where they exist), prerequisites, step-by-step procedure with code blocks, decision frames, worked examples, and pattern reference.

The two pre-existing detailed workflows are referenced in their own sections rather than duplicated here:
- **W7 Cross-crate imports/exports** — see `.docs/workflow-imports-exports.md`.
- **W9 Type overlaps & naming hygiene** — see `.docs/workflow-type-overlaps.md`.

---

## Prerequisites (shared by all workflows)

```
build_hypergraph(directory=<absolute-path>)
```

- Cold builds traverse all crates via rust-analyzer.
- Warm runs return `reused: true` in sub-second time.
- A schema bump auto-invalidates old snapshots — `force_rebuild=false` still cold-rebuilds correctly when `SCHEMA_VERSION` has changed.

For workflows using `search` or `get_similar_code`, also build the vector index:

```
index_codebase(directory=<absolute-path>)
```

Verify infrastructure once with `health_check` if anything looks stale.

All read tools are independent — batch them in parallel whenever a step calls more than one.

---

## W1 — Symbol lookup ("I have a clue, I need a qualified name")

You can't call hypergraph queries (`who_uses`, `who_imports`, etc.) without a qualified name. This workflow bridges from a string, file path, or vague description to that qualified name.

### Scope — single workflow (no scope variants)

### Step 1. Pick the entry-point tool by what you have

| Starting point | Tool |
|---|---|
| Free-text fragment, log message, doc string | `search(keyword=<string>)` |
| A symbol name, no path | `find_definition(symbol_name=<short_name>)` |
| Vague description ("function that parses JSON") | `get_similar_code(query=<description>)` |
| File path, no symbol yet | `read_file_content(file_path=...)` |
| Crate name + general area | `module_tree(directory=..., krate=<crate>)` |

### Step 2. Confirm the location

`search` and `get_similar_code` may return multiple candidates. Use `find_definition` on the most promising name to get a clean `file:line` for each candidate.

```
find_definition(symbol_name=HybridSearch)     → file:line list
read_file_content(file_path=...)              → confirm declaration
```

### Step 3. Derive the qualified name

The qualified name is `<crate>::<module_path>::<item_name>`. From the file path, derive the module path:

- `crates/<crate>/src/<a>/<b>/foo.rs` → `<crate>::a::b::foo`
- `crates/<crate>/src/<a>/<b>/mod.rs` → `<crate>::a::b`
- `crates/<crate>/src/lib.rs` → `<crate>` (root)

For inner items (methods, impl items), append `::<TypeName>::<method>`.

Confirm by walking `module_tree` for that crate to a sufficient depth and finding the item:

```
module_tree(directory=..., krate=<crate>, depth=4)
```

Match `display_name + parent_path` against your derived qualified name. If `module_tree` shows it, the qualified name is correct.

### Step 4. Promote to structural queries

Once you have the qualified name, drop the RA-driven tools and use the hypergraph:

```
who_imports(directory=..., target=<qualified_name>)
who_uses_summary(directory=..., target=<qualified_name>)
```

### Decision frames

| Situation | Tool to start with |
|---|---|
| You're sure the symbol exists, just need the path | `find_definition` |
| You don't know if anything matches | `search` |
| You want concept-level matches (rename-tolerant) | `get_similar_code` |
| You're poking at an unfamiliar crate | `module_tree` then walk |

### Pattern reference

| If you see... | Means |
|---|---|
| `find_definition` returns multiple hits | Same name in multiple crates — likely a `cross_crate_type_collision` |
| `find_definition` returns nothing but `search` finds string hits | Symbol may be macro-generated or named differently than the search query |
| `get_similar_code` returns clusters of similar fns | Possible refactor candidate (see W13) |

---

## W2 — Workspace overview ("what is this codebase?")

First-look recipe. Use when inheriting a codebase, comparing branches, or starting any deeper audit.

### Scope — workspace-wide (the whole point)

### Step 1. Foundation (parallel)

```
workspace_stats(directory=...)
crate_edges(directory=...)
dead_pub_report(directory=...)
overlaps(directory=...)
health_check()
```

Five independent reads — issue them in one round.

### Step 2. Read `workspace_stats` for shape

Key fields:

- `nodes_by_kind` (workspace / crate / module / item / external_symbol counts)
- `items_by_kind` — Struct / Enum / Fn / Method / Trait / TypeAlias / Const / Static / AssocConst / AssocType / Impl distribution
- `bindings_by_kind` — Declared / NamedImport / GlobImport / ExternCrateImport
- `visibility` — pub / pub_crate / restricted_to / private counts
- `pub_crate_share` — ratio of pub_crate to (pub + pub_crate)

`pub_crate_share` is the single most useful between-codebase comparison metric. We've measured 0.07 on a coupled mesh and 0.58 on its rewrite — same crate count, very different discipline.

### Step 3. Read `crate_edges` for architectural shape

Aggregate the matrix:

- Per-producer fan-in (`producer_crate` summed across all consumers)
- Per-consumer fan-out (`consumer_crate` summed across all producers)
- Top-N edges by `total_refs_via_imports + total_refs_via_usages`

Annotate the architecture:

- Highest fan-in producer → "universal types" crate (`domain` / `core` / `model`).
- Highest fan-out consumer → most coupled crate, often the binary or main app crate.
- Zero fan-in crates → leaf libraries OR the binary.
- Detect cycles (there shouldn't be any).

If `crate_edges` is large (> 50KB), it persists to a tool-results file. Post-process with Bash + jq/Python.

### Step 4. Read `dead_pub_report` for rot

```
dead_pub_report.crates[]                      → per-crate dead-pub findings
```

Use `crates[].crate` as the canonical crate enumeration — most reliable list of all crates in the workspace, even those with zero findings.

Vendored / library-style crates have inflated dead-pub counts — their pub surface is "designed for general use" but consumed narrowly here. Filter known external/vendored crates before reading.

### Step 5. Read `overlaps` for hygiene

Four buckets:
- `cross_crate_type_collisions`
- `module_shadows`
- `within_crate_type_duplicates`
- `common_fn_names` (4+ crates)

Empty `common_fn_names` is the good sign — no `init` / `run` proliferation. Hits worth investigating: anything other than `main` or core idioms (`new`, `default`).

### Step 6. Spot the gnarl

Pick the heaviest fan-in crate's main files (from §3 ranking) and run:

```
analyze_complexity(file_path=<path>)
```

per file. Compare `Total cyclomatic` and `Avg per function` across files (the tool returns file-level aggregates, not per-fn scores — see W15 for the workaround). Cross-reference top files with `who_uses_summary` for blast radius.

### Step 7. Output snapshot

Produce a one-page summary:

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

Returns every `unsafe { ... }` block in the workspace's local crates with `file`, `span`, `line_count`, `enclosing_function_name`, and a `has_safety_comment` flag. Filter to `has_safety_comment=false` for the "any undocumented unsafe?" question. Empty result is the healthy signal — most workspaces should have zero or single-digit undocumented unsafe. Cross-reference each finding's `enclosing_function_name` with `recursive_callers_count` to weight risk by blast radius (W20).

### Step 9. Global mutable state

```
mut_static_audit(directory=...)
```

Returns every local `static` whose HIR type matches `static mut` / `LazyLock<...>` / `OnceLock<...>` / `OnceCell<...>`. Inventory check: how many process-global mutables does this workspace carry? `LazyLock<Mutex<...>>` and `OnceCell<...>` patterns are usually the ones to scrutinize ("should this be DI'd?"). `static mut` matches are FFI / legacy hot spots. The `lazy_static!` macro is NOT detected — see W21 limitations.

### Step 10. Literal duplicates

```
semantic_overlaps(directory=..., threshold=0.95)
```

`threshold=0.95` (and the v1.1c content-hash short-circuit at similarity 1.0) surfaces source-byte duplicates: same enum variant pasted across different error enums, the same trivial helper struct redeclared per crate, etc. Top clusters are dead-easy refactor wins because there is literally nothing to harmonize — the source is identical. (See W13 for tighter recipes.)

### Step 11. Optional: architectural rules

If the workspace claims a layered architecture, codify the layer rules:

```
forbidden_dependency_check(directory=..., rules=[
  { consumer: "domain*", producer: "tokio", severity: "error" },
  { consumer: "domain*", producer: "reqwest", severity: "error" },
  { consumer: "domain*", producer: "serde_json", severity: "warn" },
])
```

Returns `violation_count` and per-violation rows with `consumer_crate`, `producer_crate`, `sample_symbol`, `unique_symbols`, `total_refs`. Empty `violations` is the pass signal. (Detail in W17.)

### Worked example (`coding-agent-bad`)

17 crates, 1441 fan-in on `domain` (universal types crate), `agent` is heaviest fan-out consumer. `pub_crate_share` low — many bare `pub`. 89 dead pubs (47 in `plurimus`, a vendored UI lib — exclude). 5 cross-crate collisions, 1 module shadow, 6 within-crate duplicates. `common_fn_names` empty (good). `unsafe_audit` had no findings (clean). `mut_static_audit` surfaced a handful of `LazyLock` singletons in the agent crate (worth a DI review). `semantic_overlaps(threshold=0.95)` found six 1.0-similarity clusters of the unit `Error` variant duplicated across `ToolResultKind` / `StopReason` / `FinishReason` enums — Recipe 13.2 in action.

### Decision frames

| Finding | Means |
|---|---|
| `pub_crate_share` < 0.2 | Low encapsulation discipline — bare `pub` everywhere |
| Highest fan-in crate is `core` / `domain` / `model` | Healthy DAG with universal types |
| Highest fan-in crate is utility/helper-ish | Possible god-crate, candidate for split |
| Cycles in `crate_edges` | Architectural break — investigate before proceeding |
| Dead pubs concentrated in one crate | Either vendored lib or facade rot |
| `overlaps.common_fn_names` non-empty | Possible missing trait abstraction |

### Pattern reference

| Signal | Pattern |
|---|---|
| Healthy DAG | Producer fan-in skewed toward 1-2 type crates; consumer fan-out skewed toward 1-2 binary/integration crates |
| Coupled mesh | Producers and consumers nearly symmetric; high `unique_symbols` per edge |
| Hourglass | One crate has both high fan-in and high fan-out (it's a bottleneck for everything) |
| Empty `common_fn_names` | Healthy discipline — no `init`/`run` proliferation |
| Empty `dead_pub_report` for a crate | Either fully consumed externally or `pub(crate)`-disciplined |

---

## W3 — Crate-level audit ("dissect crate X")

Deep dive on one crate. Cousin of W2 but scoped to a single crate.

### Scope — single crate

### Step 1. Crate snapshot (parallel)

```
module_tree(directory=..., krate=X, depth=2)
get_declared_reexports(directory=..., module=X)
dead_pub_in_crate(directory=..., krate=X)
get_imports(directory=..., module=X)
```

### Step 2. Characterize structure from `module_tree`

Default to `depth=2` to see "what submodules and root-level items exist". Bump to `depth=3` to expand items inside each submodule. Full-depth produces methods (Layer 4), but trees can be huge — a 15-submodule crate produced 72KB at depth=3.

Look for:
- Submodule count (1-2 = focused; 10+ = potentially overloaded)
- Root-level item count (high count = facade or god module)
- `pub(in <crate>)` items (internal API discipline signal — healthy)
- Visibility distribution at the root (pub vs `pub(crate)` vs private)

### Step 3. Cross-tabulate the public surface

Build a table at the crate root using three sources:

| In `module_tree` (visibility=pub) | In `get_declared_reexports` | In `dead_pub_in_crate` | Verdict |
|---|---|---|---|
| ✓ | – | – | canonical pub, live |
| – | ✓ | – | re-export, live (facade) |
| – | ✓ | ✓ | dead re-export — drop the `pub use` |
| ✓ | – | ✓ | dead canonical pub — demote to `pub(crate)` |
| ✓ | ✓ | – | re-exported AND canonical (rare; usually drop one) |

### Step 4. Outgoing/incoming dependencies

If you have workspace `crate_edges` cached, filter:

- `consumer_crate=X` → outgoing dependencies
- `producer_crate=X` → incoming dependencies

Otherwise call `crate_edges(directory=...)` and filter client-side.

A crate with one consumer is single-purpose; multiple consumers means it's a shared library. Single producer dependency means strong upstream coupling.

### Step 5. Complexity scan

For each `src/*.rs` file:

```
analyze_complexity(file_path=<path>)
```

Cross-reference top hits with `who_uses_summary` to prioritize. (See W15.)

### Step 6. Confirm canonical types are alive

For each non-dead pub item at the crate root:

```
who_uses_summary(directory=..., target=X::Type)
```

The category breakdown distinguishes:
- All-Test → demote or wrap in `#[cfg(test)]`
- All-Other → critical-path, refactor with care
- Mixed → legitimate API
- Empty → either covered by a re-export elsewhere OR genuinely dead

### Step 7. Method-level analysis (Layer 4)

For key types, walk their methods from `module_tree` and check fan-in:

```
who_uses_summary(directory=..., target=X::Type::method)
```

Empty → dead-method candidate. All-Test → test-only helper. All-Other → critical path.

### Step 8. Decision frames

| Finding | Action |
|---|---|
| Dead re-exports at crate root | Drop the `pub use`, demote source to `pub(crate)` |
| Dead canonical pubs | Demote to `pub(crate)` |
| Single-consumer crate with narrow API | Healthy — single integration point |
| Single-consumer crate with broad API | Suspicious — consumer probably doesn't need all of it |
| High submodule count + facade re-exports | Likely god-crate; consider split |
| `pub(in <crate>)` items present | Good crate-internal discipline signal |

### Worked example (`tui` in `coding-agent-bad`)

15 submodules, single entry point `tui::run` (one caller in `coding-agent::interactive`), 7 dead pubs of which 3 are dead re-exports at the crate root, sensible `pub(in tui)` discipline for crate-internal helpers. Cleanup is small: drop dead re-exports, demote source types.

---

## W4 — Module-level audit

A crate's submodule under audit. Same shape as W3 but at finer granularity.

### Scope — single module

### Step 1. Pull module data (parallel)

```
get_imports(directory=..., module=<crate::path::module>)
get_dependencies(file_path=<file_path>)
get_exports(directory=..., module=<crate::path::module>, consumer=<other>)
get_reexports(directory=..., module=<crate::path::module>, consumer=<other>)
get_declared_reexports(directory=..., module=<crate::path::module>)
```

`get_imports` is module-level (use/extern crate edges in the binding scope).
`get_dependencies` is file-level — use when you don't have a clean module path.

### Step 2. Internal structure

```
module_tree(directory=..., krate=<crate>) walked into the module path
```

Or use the file-level `get_call_graph` for parser-driven function call relationships within the module's files:

```
get_call_graph(file_path=<file_path>)
```

### Step 3. What does the module re-export?

Two flavors:
- `get_declared_reexports(module=...)` — every `pub use` declared at this module, regardless of who can reach it.
- `get_reexports(module=..., consumer=...)` — `pub use` reachable from the named consumer, visibility-filtered.

Empty `get_declared_reexports` is informative — the module has no facade.

### Step 4. Verify exports match expectations

```
get_exports(directory=..., module=..., consumer=<external_crate>)
```

vs the `pub` items reachable from `module_tree`. Items that show up in `module_tree` as `pub` but not in `get_exports(consumer=external)` are leaking through `pub(crate)` and not actually crossing the crate boundary.

### Step 5. Decision frames

| Finding | Action |
|---|---|
| `get_imports` shows wildcard imports (`use foo::*`) | Flag — explicit imports preferred for review |
| `get_declared_reexports` non-empty + facade dead | Drop the `pub use` (W12 has the recipe) |
| `get_exports(consumer=X)` empty for `consumer=X` already in `crate_edges` | Visibility filter trimming everything — likely an `pub(in <crate>)` boundary |
| Module imports many crates | Coupling smell — run W7 to verify each import is justified |

### Pattern reference

| Signal | Means |
|---|---|
| Module imports nothing except the declaring crate | Pure leaf module |
| Module imports many cross-crate types | Coordination layer / glue code |
| Module imports + re-exports the same set | Pure facade module |

---

## W5 — Symbol forensics ("dissect Item Y")

Single-symbol deep-dive. Works for structs, enums, traits, fns, methods, consts, type aliases, assoc consts, assoc types.

### Scope — single symbol Y (qualified name)

### Step 1. Locate

```
find_definition(symbol_name=<short_name>)         → file:line
```

Or, if `module_tree` was already pulled, use `Node.file + Node.span` directly.

### Step 2. Render declaration

```
read_file_content(file_path=<file>)             → render context around the span
```

Widen by ~10 lines for readable context.

### Step 3. Reverse lookups (parallel)

```
who_imports(directory=..., target=<qualified_name>)
who_uses(directory=..., target=<qualified_name>)
who_uses_summary(directory=..., target=<qualified_name>)
```

`who_imports` lists every `use` statement bringing Y into scope.
`who_uses` lists every non-import reference (file:byte-range hits).
`who_uses_summary` aggregates by consumer module with Test/Other category breakdown.

### Step 4. Render call sites with context

For each `who_uses` hit:

```
read_file_content(file_path=<file>)             → slice [start - 200, end + 200]
```

### Step 5. RA cross-reference (catches things who_uses misses)

```
find_references(symbol_name=<short_name>)
```

`find_references` is broader scope — it includes local var refs, lifetime annotations, and other RA-tracked things. `who_uses` is structural and aggregated. Use both when verifying "is X really unused?".

### Step 6. Cross-crate fan-in summary

Group `who_imports` by consumer crate:

| Consumer crate | Importer count |
|---|---|
| crate_a | 3 |
| crate_b | 1 |

A symbol with many crates importing it is widely-used; refactor with care.

### Step 7. Method-level fan-in (Layer 4 unlocks)

```
who_uses(directory=..., target=Type::method)
```

Pre-Layer-4 these queries errored. Post-Layer-4 they return real results, including trait dispatch.

### Decision frames

| Finding | Verdict |
|---|---|
| `who_uses` empty + `who_imports` empty + `find_references` empty | Safe to delete |
| `who_uses` empty + `who_imports` non-empty | Imported but never referenced — possibly used as a generic bound; investigate |
| `who_uses_summary` 100% Test | Test fixture; demote or `#[cfg(test)]` |
| `who_uses_summary` 100% Other | Critical path; high refactor risk |
| Single consumer module | Tightly coupled to one place; consider co-locating |
| Many consumer crates | Workspace-shared API; avoid breaking changes |

### Pattern reference

| Signal | Means |
|---|---|
| `who_uses` empty but `find_references` populated | Symbol used in a context the hypergraph doesn't index (macro-introduced, cfg-gated) |
| `who_uses` Read >> Write | Read-mostly API — encapsulation healthy |
| `who_uses` Write-heavy | Diffuse invariants; many writers means brittle state |

---

## W6 — Trait-specific analysis

Layer 4 sweet spot. Trait declarations and their methods are first-class graph nodes; `x.method()` and `Type::method()` resolve back to the trait declaration.

### Scope — single trait T

### Step 1. Locate the trait

```
find_definition(symbol_name=T)                    → file:line
module_tree(directory=..., krate=<crate>)  → walk to the trait, expand methods
```

### Step 2. Identify trait methods

From `module_tree`, the trait Item has children: methods, assoc consts, assoc types. List them.

### Step 3. Fan-in per method (parallel)

For each method `M`:

```
who_uses_summary(directory=..., target=<crate>::T::M)
```

Sort results by `total_count` desc.

### Step 4. Trait-level fan-in

```
who_imports(directory=..., target=<crate>::T)
```

Modules that import `T` typically either implement it or take it as a generic bound.

### Step 4.5. Layer 10 trait dispatch tracing

```
who_calls(directory=..., target=<crate>::T::M)
```

For each method M on the trait, `who_calls` returns every fn body that contains a call resolving back to `T::M`. Pre-Layer-10 these queries either errored or returned only same-fn references; post-Layer-10 they return workspace-wide call sites attributed to the enclosing fn — including dispatch through generic bounds and `dyn T` receivers. Pair with `who_uses_summary` from Step 3 to sort methods by total call-site count and to spot the case where one method dominates the dispatch traffic (often the "real" core of the abstraction).

### Step 5. Trait deletion / sealing check

| Pattern | Verdict |
|---|---|
| `who_uses(T)` empty across all crates outside the defining one | Safe to delete or seal |
| `who_uses(T::M)` empty for some method M | Safe to remove M (verify trait impls aren't hardcoding it) |
| Single importer + single implementer | Trait is doing nothing — inline (rust-guidelines §8) |
| Multiple implementers + multiple consumers | Real abstraction boundary; keep |

### Step 6. Single-implementation audit (rust-guidelines §8)

For each `pub trait` in `module_tree`:

```
who_imports(directory=..., target=<crate>::T)
```

If importer count is 1 and the trait isn't a `Send`/`Debug`-style supertrait, it's a candidate for inlining. The trait has one job: hide an impl that nothing else substitutes — usually deletable.

### Step 7. Decision frames

| Finding | Action |
|---|---|
| Trait with one impl + one consumer | Inline; delete the trait |
| Trait with one impl + multiple consumers | Probably needed for substitution; verify |
| Trait method with empty `who_uses_summary` | Remove the method (verify impls don't keep it for hardcoded reasons) |
| Trait imported by many crates, methods used by few | Probably a generic-bound trait; safe |
| Trait method has all-Test fan-in | Test-only trait method; demote to `#[cfg(test)]` |

### Pattern reference

| Signal | Means |
|---|---|
| `who_uses(T::M)` resolves to many call sites in unrelated crates | Trait is genuine substitution boundary |
| Same call site for `T::M` and a single concrete `Type::M` | Trait dispatch may be vestigial; check if generic param flows through |

---

## W7 — Cross-crate imports/exports analysis

**See `.docs/workflow-imports-exports.md` for the full detailed workflow.**

Summary: workspace-wide and single-crate scopes; uses `crate_edges` + `dead_pub_report` + `get_declared_reexports` to audit producer/consumer relationships, hot symbols, dead facades, and tight coupling.

Key recipes (from that doc):
- Decompose `crate_edges` into per-producer fan-in / per-consumer fan-out / top-N edges.
- `dead_pub_report ∩ get_declared_reexports = dead facade` (drop the `pub use`, demote source).
- Hot-symbol drilldown via `who_uses_summary`.

### Sortable metric — `crate_dependency_metric`

```
crate_dependency_metric(directory=..., sort_by="afferent", top_n=10)
```

Per-local-crate Robert Martin metric: `afferent` (Ca = distinct incoming consumer crates), `efferent` (Ce = distinct outgoing producer crates), `instability = Ce / (Ce + Ca)` (0 = max stable, 1 = max unstable), `abstractness = (traits + pub_type_aliases) / total_items`. `sort_by` accepts `instability` / `item_count` / `afferent` / `efferent` / `abstractness` (all descending), and `top_n` slices the head. Use as a high-level overview before drilling into `crate_edges` filtered by a specific crate — the metric ranking surfaces the architectural core (low instability) and the workhorses (high efferent), and the abstractness ratio flags facade-style crates.

Cross-link: see W23 for the dedicated workflow.

---

## W8 — Refactor planning workflows

Practical recipes for specific refactor questions. Each is a short composition of existing tools.

### Scope — task-specific (one symbol, one decision at a time)

### Recipe 8.1 — "Should I downgrade X from `pub` to `pub(crate)`?"

```
who_imports(directory=..., target=X)
who_uses(directory=..., target=X)
```

If both are empty cross-crate (all consumers in the same crate as X), demote. `dead_pub_report` likely already flagged it.

### Recipe 8.2 — "Is it safe to delete X?"

```
who_uses(directory=..., target=X)
who_imports(directory=..., target=X)
find_references(symbol_name=X)
```

Empty everywhere = delete. `find_references` catches things `who_uses` doesn't (local var shadows, lifetimes, doc comments).

### Recipe 8.3 — "Should I move X to a different crate?"

```
who_uses_summary(directory=..., target=X)
```

Look at consumer module distribution. Move X to where most callers live, OR factor X's deps upstream so no callers need to import sideways.

### Recipe 8.4 — "Is this `pub use` facade earning its keep?"

```
get_declared_reexports(directory=..., module=<crate_root>)
```

For each item, run `who_imports(target=<canonical_path>)`. If most importers reach for the canonical path and few or none use the facade, drop the `pub use`.

### Recipe 8.5 — "Should I make this trait sealed?"

```
who_uses(directory=..., target=T)
who_imports(directory=..., target=T)
```

filtered to importers outside the defining crate. If implementers are all internal and external use is purely consumption, seal it.

### Recipe 8.6 — "Do crate-private types leak through pub APIs?"

```
get_exports(directory=..., module=<crate_root>, consumer=<other>)
module_tree(directory=..., krate=<crate>) filtered to pub items
```

Diff: items reachable from external consumers vs items declared `pub` at canonical sites. Mismatches are the leaks.

### Recipe 8.7 — "What's the minimum viable refactor target?"

```
crate_edges(directory=...)
```

Filter to a target `(consumer, producer)` pair. If `unique_symbols` is small (1-3), that small set is your refactor target — extract or relocate just those.

### Recipe 8.8 — "Test-only helpers I can move to dev-deps?"

```
who_uses_summary(directory=..., target=<helper>)
```

Filter to rows where `category_breakdown` is all Test. Those helpers are dev-deps candidates.

### Recipe 8.9 — "Verify a refactor didn't widen the API"

Pre-refactor:

```
get_declared_reexports(directory=..., module=<crate_root>) → JSON A
dead_pub_report(directory=...) → JSON B
```

Post-refactor: same calls. Diff JSON. New entries in declared_reexports that weren't there before = API widened. Lost entries in dead_pub_report = items now used (good). New entries in dead_pub_report = items now dead (consider removing).

### Recipe 8.10 — "Find duplicate logic worth extracting"

One call returns ranked clusters of semantically-similar fns in crate X:

```
semantic_overlaps(directory=..., crate_name=X, item_kind="Function")
```

For each cluster:
  1. Inspect `members` — qualified names, files, spans, and `avg_similarity` / `min_similarity`.
  2. Run `who_uses_summary(target=<member>)` per member to verify they're called and to plan migration order.
  3. Top clusters by `avg_similarity` are the best extraction targets.

Cross-reference: workspace-scale via `semantic_overlaps(directory=...)` with no `crate_name` filter; cross-crate-only via `cross_crate_only=true` to surface the case where the same logic is duplicated across crates. Detail in W13.

### Recipe 8.11 — "Which complex files have the highest blast radius?"

```
analyze_complexity(file_path=<path>)             → file-level cyclomatic aggregates
get_call_graph(file_path=<path>)                 → identify high-out-degree fns inside the file
who_uses(directory=..., target=<crate>::<fn>)    → per candidate fn, fan-in
```

Sort files by `Total cyclomatic` desc, then within hot files use `get_call_graph` to find the dispatch hubs, then weight by fan-in. (See W15 for the file-level vs per-fn caveat.)

### Recipe 8.12 — "Find dead facade re-exports" (high leverage)

```
get_declared_reexports(directory=..., module=<crate_root>)
dead_pub_in_crate(directory=..., krate=<crate>)
```

Items appearing in BOTH = dead facade branches. Drop the `pub use` line, demote source to `pub(crate)`.

Spotted on `tui` in `coding-agent-bad`: `RunState`, `InvalidTransition`, `RunnerWakeError` were all re-exported AND dead.

### Recipe 8.13 — "Detect half-finished migrations" (high leverage)

```
overlaps(directory=...)                     → cross_crate_type_collisions
```

For each collision, run `who_uses_summary` on both qualified names. Look for `consumer_qualified_name` overlap between the two row sets — a consumer module that imports BOTH versions is converting between them, usually the trace of a migration that was started by duplicating instead of moving.

Spotted on `coding-agent-bad`: `AgentConfig` in `agent::config` and `config` crates, both used by `coding-agent::compose`.

### Decision frames (cross-recipe)

| Refactor goal | Trust signal |
|---|---|
| Demote pub → pub(crate) | `dead_pub_report` flag + cross-crate `who_imports` empty |
| Delete | `who_uses` ∪ `who_imports` ∪ `find_references` all empty |
| Move to different crate | `who_uses_summary` clusters in a different crate |
| Seal trait | Outside-crate `who_imports(T)` = 0 |
| Drop facade re-export | Both ends in `dead_pub_in_crate` |

---

## W9 — Hygiene audits / type overlaps

**See `.docs/workflow-type-overlaps.md` for the full detailed workflow.**

Summary: workspace-wide and single-crate scopes; uses `overlaps` + `crate_edges` + `who_uses_summary` to detect cross-crate name collisions, module shadows, within-crate duplicates, and common fn names that hint at missing abstractions.

Key recipes (from that doc):
- Same-name type used by same consumer = migration debt (HIGH severity).
- Module shadow + dep on shadowed crate = real bug; without dep = footgun.
- Test-fixture heuristic on within-crate duplicates: names like `Mock*`/`Fake*`/`Stub*` in `tests`/`unit`/`common` modules are mechanical refactors.
- Empty `common_fn_names` is the good signal.

---

## W10 — Test vs production analysis (Test/Other category split)

Layer 8 categorizes every reference as `Read` / `Write` / `Test` / `Other`. The Test split is the load-bearing one for "what's only used by tests?"

### Scope — single symbol or symbol family

### Step 1. Pull the breakdown

```
who_uses_summary(directory=..., target=<qualified_name>)
```

Each row has a `category_breakdown` with `Test` and `Other` counts (Read/Write counts also included in some shapes).

### Step 2. Classify

| Pattern | Verdict |
|---|---|
| All rows 100% Test | Test fixture / builder. Demote to `#[cfg(test)]` or move to dev-deps. |
| All rows 100% Other (Test=0) | Production-only. Critical path. High refactor risk. |
| Mixed Test + Other | Legitimate API. Both tested and used. |
| Test >> Other | Either under-used in production or over-tested in isolation. |

### Step 3. Read vs Write encapsulation check

Some payloads include `Read` and `Write` sub-counts. Many readers + few writers = good encapsulation. Many writers = diffuse invariants — often the symbol's value flows through too many places.

### Step 4. Targeted recipes

#### Recipe 10.1 — "Test-only constructor audit"

For each `Type::new` (and `with_*`/`from_*` constructors), run `who_uses_summary`. Rows 100% Test = builder used only by tests. Move to test fixtures.

#### Recipe 10.2 — "Production-only methods"

Filter `module_tree` to methods. For each, `who_uses_summary`. All-Other rows = critical-path. Annotate as "high touch risk" in PR descriptions.

#### Recipe 10.3 — "Mostly-tested public API"

For pub items, `who_uses_summary`. Test >> Other (e.g. 30 Test, 2 Other) usually means the symbol is tested in isolation but barely consumed in production — under-used or over-tested.

### Decision frames

| Finding | Action |
|---|---|
| 100% Test fan-in for a `pub` item | Demote to `pub(crate)` + `#[cfg(test)]` |
| 100% Test fan-in for a `pub` constructor | Move to a test-fixtures crate / `tests/common` |
| 100% Test for an entire trait's methods | Trait is test-only; consider deleting and using concrete types in tests |
| Heavy Write counts on a shared type | Diffuse invariants; consider `&mut self` API audit |

### Pattern reference

| Signal | Means |
|---|---|
| `Test=N, Other=0` for Type::new | Constructor used only by tests |
| `Test=0, Other=N` for a trait method | Production-only API; refactor with care |
| `Test=N, Other=M, Read=X, Write=Y` with `Y >> X` | Many writers — invariants are diffuse |

---

## W11 — Method-aware workflows (Layer 4)

Layer 4 nests methods, assoc consts, and assoc types as children of their host types in `module_tree`. This unlocks per-method analysis that pre-Layer 4 errored.

### Scope — single type (and its method API surface)

### Step 1. Pull the type's full surface

```
module_tree(directory=..., krate=<crate>, depth=4)
```

Walk to the type. Children are methods, assoc consts, assoc types. Their `kind` field disambiguates: `Method`, `AssocConst`, `AssocType`.

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

For trait dispatch, `who_uses` resolves back to the trait declaration, not the impl. To find concrete impl callers, search by the impl's qualified name (Layer 4 nests these).

### Step 5. Method-naming consistency check

Scan `module_tree` outputs for naming patterns:
- Every constructor `new` vs some `from`/`create`/`with`?
- Error type conversions: `from_io`, `from_parse`, etc., consistent?
- Mutators: `set_*` vs `update_*` vs bare verbs?

Subjective but worth noting in code review.

### Step 6. Function-level call graph (within file)

Layer 4 doesn't unlock cross-file fn-to-fn graphs. For within-file flow:

```
get_call_graph(file_path=<path>)
```

Parser-based; gives function-to-function edges within one file. Use as a complement to method-level usages across files.

### Decision frames

| Finding | Action |
|---|---|
| Method with empty `who_uses_summary` | Verify (may be dispatched via trait); demote or delete |
| Method on trait with empty `who_uses` | Either trait method is dead OR all dispatch goes through `Type::method` directly |
| Method on impl block named `new` with all-Test consumers | Test-only constructor; gate with `#[cfg(test)]` |
| Method-naming inconsistency across types | Style cleanup, low priority but easy |

### Pattern reference

| Signal | Means |
|---|---|
| `who_uses(Type::method)` empty pre-Layer-4, populated post-Layer-4 | Layer 4 successfully surfaces method calls |
| Trait method has more `who_uses` than any impl method | Dispatch is mostly trait-level (good substitution) |
| Trait method has fewer `who_uses` than concrete impl methods | Most callers go through concrete types — trait may be vestigial |

---

## W12 — API surface auditing

Catch over-broad facades, accidental internals exposure, and dead public surface.

### Scope — single crate

### Step 1. Pull the surface (parallel)

```
get_declared_reexports(directory=..., module=<crate_root>)
get_exports(directory=..., module=<crate_root>, consumer=<external_crate>)
module_tree(directory=..., krate=<crate>) filtered to pub items
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

Most importers reaching for the canonical path means the facade isn't being used. Drop it.

### Step 4. Detect accidentally-exposed internals

Items in `get_declared_reexports` that the team thought were `pub(crate)`. These usually slip in via `pub use submodule::*` patterns.

### Step 5. Detect pub items hiding behind a facade that don't need to be pub

If `pub use` chain can become `pub(crate) use`, source can be `pub(crate)`. `dead_pub_in_crate` already finds these.

### Step 6. Empty results as signals

| Empty result | Means |
|---|---|
| `get_declared_reexports([])` | Crate has no facade — everything at canonical paths. Intentional design. |
| `dead_pub_in_crate([])` | No dead pubs — disciplined. |
| `overlaps.common_fn_names([])` | No `init`/`run` proliferation — good hygiene. |

### Decision frames

| Finding | Action |
|---|---|
| Re-export declared at root, target dead in `dead_pub_in_crate` | Drop `pub use`, demote source |
| Pub item at canonical site, dead in `dead_pub_in_crate` | Demote to `pub(crate)` |
| Item in `get_declared_reexports` that looks crate-internal | Probably accidentally exposed; demote |
| Re-export AND canonical declaration of same item | Pick one path; drop the other |

### Pattern reference

| Signal | Means |
|---|---|
| `pub use foo::*` at crate root | Likely over-broad facade; audit each item |
| Crate's `get_declared_reexports` ⊆ `dead_pub_in_crate` | Entire facade is dead — drop wholesale |
| `get_exports(consumer=X)` smaller than `get_declared_reexports` | Visibility filter trimming `pub(crate)` | `pub(in <crate>)` items |

---

## W13 — Semantic similarity-driven analysis

Find duplicate or near-duplicate logic that's named differently, split across crates, or simply pasted in two places. Three vector-backed tools cover three different question shapes; pick the right one before reaching for any of them.

### Scope — function bodies, type bodies, and Item-level semantic search across the workspace

### Three vector tools, three jobs

| Tool | Seed shape | Scope | When to use |
|---|---|---|---|
| `get_similar_code(query)` | Free-text natural-language description ("function that parses JSON") | Chunk-level matches across the indexed codebase | You don't know the symbol or its name. Useful when starting from a log message, a doc string, or a bug-report fragment. |
| `similar_to_item(target, limit, threshold, item_kind)` | A single qualified-name Item | Item-level semantic neighbors via vector embeddings | You already have ONE Item and want neighbors. ~100-300ms per call. Investigation tool: "what's like X?" |
| `semantic_overlaps(directory, ...)` | No seed — workspace-wide | Pairs or clusters of transitively-similar Items | Audit / refactor planning. v1.1 caches per-Item embeddings in LMDB; first scan pays the full embed cost, subsequent scans on unchanged code are essentially free. |

`get_similar_code` doesn't know about Item nodes — it returns chunk previews and you have to bridge yourself. `similar_to_item` resolves the seed via the hypergraph (`target` → `(file, span)`), reads its source, and runs `vector_only_search` against the indexed corpus. `semantic_overlaps` enumerates Items in scope, embeds each Item's source bytes (cached), runs an in-memory pairwise cosine scan, and returns either deduplicated `pairs` or single-linkage `clusters`. The three tools answer progressively bigger questions and require progressively more setup.

### Prerequisites

`build_hypergraph` must have run for any tool that takes a qualified-name seed (`similar_to_item`, `semantic_overlaps`). `index_codebase` must additionally have run for `get_similar_code` and `similar_to_item` (they share the vector store). `semantic_overlaps` v1.1 embeds Item source directly and caches in the snapshot's LMDB env — it does NOT require a fresh `index_codebase`.

### Step 1. Pick the tier

```
Do you have a qualified name?
├── No → get_similar_code(query=<free text>)
└── Yes
    ├── You want neighbors of one specific Item → similar_to_item(target=Y)
    └── You want a workspace-wide audit → semantic_overlaps(directory=...)
```

Cross-cuts:
- If the question is "find me one needle by description", `get_similar_code` is the right tier — it doesn't require a hypergraph build.
- If the question is "is X duplicated?", `similar_to_item(target=X)` is the cheapest — single embed + vector-only search.
- If the question is "what's duplicated that I don't know about?", `semantic_overlaps` is the only tool that answers it without manually seeding every candidate.

### Step 2. Run the appropriate tool

Free-text:

```
get_similar_code(directory=..., query=<description>, limit=10)
```

Returns ranked chunk previews. Each row carries `symbol_name`, `symbol_kind`, `file`, `line_start/end`, and a short preview. Bridge to a qualified name via `find_definition` or `module_tree` walking when you need to drop into the hypergraph.

Single-seed:

```
similar_to_item(directory=..., target=<qualified_name>, limit=10, threshold=0.80, item_kind="Function")
```

Returns ranked vector matches above `threshold`, capped at `limit`, optionally filtered by `item_kind` (case-insensitive: `"function"` / `"struct"` / `"enum"` / `"trait"` / etc.). Self-match is dropped via line-range overlap. Tune `threshold` from 0.80 (start permissive) upward.

Workspace audit:

```
semantic_overlaps(directory=..., crate_name=<optional>, item_kind=<optional>,
                  threshold=0.85, output_mode="clusters", max_pairs=50,
                  max_cluster_size=15, skip_test_chunks=true, cross_crate_only=false)
```

Returns either `pairs` (raw similarity edges, deduplicated) or `clusters` (single-linkage groups), sorted by `avg_similarity` desc. Test fixtures dominate noise and are dropped by default.

### Step 3. Verify (when the tool returns candidates)

Vector similarity is necessary but not sufficient. For each candidate cluster member or returned match:

```
who_uses_summary(directory=..., target=<qualified_name>)
```

Filters out dead candidates and quantifies migration cost. If the candidate isn't in the hypergraph (e.g. macro-generated), fall back to `find_definition(symbol_name=...)` and `read_file_content` at the file:span.

```
read_file_content(file_path=...)
```

at each candidate's span. Inspect manually — embeddings encode lexical+syntactic patterns more than logical intent, and a high cosine score on two similarly-shaped fns can still hide diverged behavior.

### Step 4. Targeted recipes

#### Recipe 13.1 — "Find duplicate logic worth extracting"

```
semantic_overlaps(directory=..., crate_name=X, item_kind="Function", threshold=0.80)
```

Crate-scoped scans tolerate a lower threshold because chaining is bounded by the crate's smaller item count. For each top cluster:

1. Read `members` — qualified names, files, spans, `avg_similarity`, `min_similarity`.
2. Run `who_uses_summary(target=<member>)` per member to verify they're called and to plan migration order (members with higher fan-in dictate signature constraints).
3. Top clusters by `avg_similarity` are the best extraction targets. Cross-reference with `analyze_complexity(file_path=<member.file>)` to prioritize: similarity × complexity × blast radius is the refactor-priority scoring.

This replaces the older "cluster `get_similar_code` results manually" recipe — `semantic_overlaps` does the cluster step natively.

#### Recipe 13.2 — "Type-1 clone detection (literal duplicates)"

```
semantic_overlaps(directory=..., threshold=0.95)
```

`threshold=0.95` plus the v1.1c content-hash short-circuit (Items whose source bytes hash identically get `similarity = 1.0` directly, no cosine call) surfaces literal duplicates. The cheapest refactor wins — no semantic harmonization needed because the source is the same.

Real example from `coding-agent-bad`: the unit `Error` variant duplicated as a unit variant in 6 different crates' error enums (`ToolResultKind::Error`, `StopReason::Error`, `FinishReason::Error`, etc.). All collapse to a 1.0-similarity cluster. The fix is not "extract a function" but "introduce a shared unit-error trait or factor out the variant", and the cluster makes the case visible at a glance.

#### Recipe 13.3 — "Convergent enum design"

```
semantic_overlaps(directory=..., item_kind="EnumVariant", threshold=0.95)
```

Variants whose source bytes hash identically get clustered. Catches the case where the same logical state (`Idle` / `Done` / `Error` / `Pending`) was modeled as separate variants on different enums — the very signal that two enums should share a base or one is redundant.

Cross-link with `enum_variants(target=<each enum>)` (W24) to get the full variant list per host enum and `who_uses(target=<crate>::Enum::Variant)` for fan-in per variant.

#### Recipe 13.4 — "Same-shape struct detection across crates"

```
semantic_overlaps(directory=..., item_kind="Struct", cross_crate_only=true, threshold=0.85)
```

`cross_crate_only=true` drops same-crate pairs (≈76% of pairs in our measured workspaces) — the remaining clusters are structurally similar structs that live in different crates and probably should not.

Real example: `TokenUsage` defined separately in 3 crates (an HTTP-client crate, a chat-completion crate, and a token-budget crate) — all carrying `prompt_tokens: u32`, `completion_tokens: u32`, `total_tokens: u32`. The cluster surfaces them as a single `domain::TokenUsage` extraction candidate.

#### Recipe 13.5 — "Refactor candidate ranking"

```
semantic_overlaps(directory=..., crate_name=X)
```

Returns clusters sorted by `avg_similarity` descending. Top clusters are the highest-confidence extraction targets. Combine with:

```
analyze_complexity(file_path=<member.file>)             → file-level cyclomatic
who_uses_summary(directory=..., target=<member>)        → fan-in
```

Score by `avg_similarity × complexity × fan_in` — the cluster whose members are both gnarly and widely-used yields the largest payoff. Use `output_mode="pairs"` instead of `"clusters"` when you need raw edges for migration planning (each pair is a single concrete decision: "merge A into B or vice versa?").

#### Recipe 13.6 — "Naming-convention enforcement"

```
semantic_overlaps(directory=..., cross_crate_only=true, threshold=0.85)
```

Cross-crate clusters whose members carry different names but similar source signal a naming inconsistency: `now_ms` in one crate, `now_ts` in another, `unix_now_secs` in a third. The cluster surfaces them; rename to a single convention before extracting.

### Decision frames

| Situation | Tier / parameters |
|---|---|
| You don't know the symbol's name | `get_similar_code` |
| You have one symbol, want neighbors | `similar_to_item(target=Y, threshold=0.80)` |
| You want a workspace audit | `semantic_overlaps(directory=...)` |
| Crate-scoped scan | `semantic_overlaps(crate_name=X, threshold=0.80)` (tighter chaining at small scale) |
| Workspace-wide scan | `semantic_overlaps(directory=..., threshold=0.85)` (default; 0.80 produces useless mega-clusters via single-linkage chaining) |
| "Is anything duplicated literally?" | `semantic_overlaps(threshold=0.95)` (content-hash short-circuit at 1.0) |
| "Is anything duplicated across crate boundaries?" | `semantic_overlaps(cross_crate_only=true)` (drops 76% of pairs in our measured workspaces) |
| Want raw edges for migration planning | `output_mode="pairs"` (better than `"clusters"` for pairwise decisions) |
| Want grouped signal for extraction planning | `output_mode="clusters"` (default; better for "extract one helper for these three fns") |

### Pattern reference

| Pattern | Invocation |
|---|---|
| Crate audit | `semantic_overlaps(crate_name=X, item_kind="Function", threshold=0.80)` |
| Workspace audit | `semantic_overlaps(directory=..., threshold=0.85)` |
| Type-1 clones | `semantic_overlaps(threshold=0.95)` |
| Cross-crate structs | `semantic_overlaps(item_kind="Struct", cross_crate_only=true)` |
| Variant convergence | `semantic_overlaps(item_kind="EnumVariant", threshold=0.95)` |
| Single-seed lookup | `similar_to_item(target=Y, threshold=0.80)` |
| Free-text needle | `get_similar_code(query="function that parses JSON")` |

### Limitations

- Single-linkage clustering can chain through outliers — one bridging pair can pull two distant clusters together. The `max_cluster_size=15` default drops the worst chains; bump it to inspect them or set to 0 to disable. Tightening `threshold` is the principled mitigation.
- The embedder is `fastembed:all-MiniLM-L6-v2:dim384:v1`. Cache key is `(NodeId, content_hash, embedder_version)`; switching the embedding model invalidates every cached entry.
- First-scan latency is seconds-to-minutes at workspace scale (each Item is embedded once); subsequent scans on unchanged code are near-instant because vectors are reused. Edits to an Item flip its `content_hash` → next call re-embeds just that Item.
- Embeddings encode lexical+syntactic patterns more than logical intent. A high cosine score on two similarly-shaped fns can still hide diverged behavior — verify with `read_file_content` before deduping.
- `semantic_overlaps` does NOT subsume `overlaps.cross_crate_type_collisions`. The latter is name-equality / structure-only (catches `Foo` declared in two crates regardless of body), and the former is content-similar (catches different-name look-alikes). They're complementary: collisions for naming hygiene, semantic overlaps for refactor planning.
- v1.1 supports only single-linkage clustering — no HDBSCAN, no k-means, no density-based variants. Streaming partial results is also not implemented; the tool returns the full payload or nothing.

---

## W14 — Function-level call graphs (workspace-wide)

Layer 10 makes fn-body call edges first-class graph data. Five tools cover incoming, outgoing, recursive descent, crate-scoped filter, and a transitive-caller count. The older `get_call_graph` (parser, single-file) is now the within-file fallback.

### Scope — single fn or single file

### Layer 10 (workspace-wide) vs `get_call_graph` (within-file)

| Question | Tool |
|---|---|
| "Who calls fn Y, anywhere?" | `who_calls(target=Y)` |
| "What does fn Y call, anywhere?" | `calls_from(caller=Y)` |
| "What's reachable from Y up to depth N?" | `call_graph(root=Y, depth=N)` |
| "Who in crate X calls Y?" | `callers_in_crate(target=Y, krate=X)` |
| "How many distinct fns transitively call Y?" | `recursive_callers_count(target=Y, depth=N)` |
| "What does this single file look like internally?" | `get_call_graph(file_path=...)` (parser fallback) |

Layer 10 is HIR-resolved and includes calls through generic bounds and `dyn T` receivers; the parser-driven `get_call_graph` only sees the AST in one file and misses cross-file edges entirely.

### Step 1. Pull workspace-wide callers

```
who_calls(directory=..., target=<crate>::Y)
```

Returns every fn body containing a call to Y. Each row carries `caller` (the enclosing fn's qualified name), `file`, byte `start` / `end`, and `category` (Read/Write/Test/Other). References in const initializers, type aliases, and other non-function scopes are excluded — use `who_uses` to see all reference sites. Calls from closures attribute to the enclosing fn.

This is the workspace-wide upgrade of the older "filter `who_uses` to call sites manually" recipe.

### Step 2. Pull workspace-wide callees

```
calls_from(directory=..., caller=<crate>::Y)
```

Every outgoing reference made from Y's body. Same row shape as `who_calls` but with `callee` instead of `caller`. Use to inventory Y's downstream surface — "what does this fn touch?" — before extracting helpers.

### Step 3. Bounded recursive call tree

```
call_graph(directory=..., root=<crate>::Y, depth=3)
```

Bounded recursive descent over outgoing call edges from `root`. `depth` defaults to 3 and is capped at 8. Returns a tree where each node carries `fn_qualified_name`, `crate_name`, `callees`, `truncated_at_cycle` (the fn was already expanded earlier in the traversal — its callees are visible elsewhere in the tree), and `truncated_at_depth` (depth ran out at this node and there were unvisited callees).

Useful for "what does this fn ultimately reach?" without composing many `calls_from` calls. Pick depth deliberately: depth=2 keeps the tree readable; depth=8 is the cap and produces large outputs on hub fns.

### Step 4. Crate-scoped audit

```
callers_in_crate(directory=..., target=<crate>::Y, krate=<other_crate>)
```

`who_calls(target=Y)` filtered to call sites whose *caller fn* lives in the named crate. Use to verify a crate boundary holds — e.g. "no fn in `domain` should call into `agent::orchestrator`" → `callers_in_crate(target=agent::orchestrator::*, krate=domain)` should return zero.

### Step 5. Blast-radius integer

```
recursive_callers_count(directory=..., target=<crate>::Y, depth=8)
```

Reverse BFS counting distinct transitive caller fns up to `depth` hops. Returns `direct_callers`, `transitive_callers`, `depth_reached`, `truncated_at_depth`. Counts *fns*, not call sites — a fn that calls Y five times counts as 1 caller.

Replaces the older "count rows in `who_uses_summary`" heuristic (which counted module-level consumers and missed transitive reach). Useful as a single integer to weight refactor risk: a fn with `transitive_callers=200` is a critical-path hub; deleting or changing its signature has 200-fn fallout. Pair with `unsafe_audit` (W20) and `mut_static_audit` (W21) to score which findings sit on hot paths.

### Step 6. Within-file structure (parser fallback)

```
get_call_graph(file_path=<path>)
```

Parser-based, single-file: returns fn-to-fn edges within the file's AST. Use when:
- You don't have a hypergraph build (e.g. analyzing a non-workspace dependency drop, or a snapshot where `build_hypergraph` would take too long).
- You want fn-arity-style structural analysis on a single file (find dispatch hubs by out-degree).
- You want to corroborate Layer 10 results — the parser sees what's syntactically there, Layer 10 sees what HIR resolved.

Compose: `get_call_graph(file_path=<file_with_complex_fn>)` for internal structure, then `who_calls(target=<crate>::<fn>)` for external callers — together you get the full picture for one fn.

### Decision frames

| Goal | Tool |
|---|---|
| Workspace-wide caller list | `who_calls` |
| Workspace-wide callee list | `calls_from` |
| "What's reachable from here?" | `call_graph(depth=2 or 3)` |
| "Does this caller crate respect the rule?" | `callers_in_crate` |
| Single integer for refactor scoring | `recursive_callers_count` |
| Within-file structure (no hypergraph) | `get_call_graph` |

Depth budget for `call_graph`:
- depth=1 → direct callees only (cheaper than `calls_from` if you also want cycle/depth-truncation flags).
- depth=2 → readable in JSON for most fns.
- depth=3 → default; balance of recall vs payload.
- depth≥6 → hub fns produce large outputs; use sparingly.

### Pattern reference

| Pattern | Invocation |
|---|---|
| List every caller of Y | `who_calls(target=Y)` |
| List every callee of Y | `calls_from(caller=Y)` |
| Reachability map up to depth 3 | `call_graph(root=Y, depth=3)` |
| Crate boundary check | `callers_in_crate(target=Y, krate=X)` |
| Refactor blast radius | `recursive_callers_count(target=Y, depth=8)` |
| File-internal structure | `get_call_graph(file_path=<file>)` |

### Limitations

- Trait dispatch via dynamic calls is a static-resolution heuristic (Layer 10 follows HIR's resolved callee; runtime polymorphism through `dyn Trait` may be incomplete depending on how RA resolved the receiver type).
- No enum-of-fn-pointers tracking — a `match` arm dispatching to one of N fn pointers reads as a load, not as N call edges.
- Macro-expanded calls may not surface — `println!("{}", foo())` resolves the inner `foo()` call, but a custom macro whose expansion contains a call may be invisible if the call isn't visible in the post-expansion HIR.
- `get_call_graph` is parser-only and misses cross-file calls entirely; use Layer 10 for workspace-wide questions.

---

## W15 — Complexity-driven prioritization

Find the gnarly code, then rank by blast radius.

### Scope — workspace or single crate

### Step 1. Find files with high aggregate complexity

```
analyze_complexity(file_path=<path>)
```

Returns **file-level aggregates** (not per-function): total LOC, function/struct/trait counts, **total cyclomatic**, **avg cyclomatic per function**, and total function-call count. There is no cognitive metric and no per-function score in the output.

Heuristics for prioritization at the file level:
- High `Total cyclomatic` (≥ 50 in a single file) → file probably contains at least one gnarly fn.
- High `Avg per function` (≥ 5) → branching is spread across the file; whole-file refactor candidate.
- High `Function calls` relative to function count → tight intra-file coupling; cross-reference with `get_call_graph` to see hubs.

Per-function thresholds (cyclomatic ≥ 10 / 15 / 25 from rust-guidelines §4) are NOT directly checkable with this tool — you'd need a parser-level walk. For now, use file-level aggregates to triage which files to read.

### Step 2. Identify the actual gnarly fns within a hot file

`analyze_complexity` doesn't tell you which fn is gnarly. To find it:

```
get_call_graph(file_path=<file>)
```

Functions with high out-degree (calling many helpers) are the dispatch hubs — usually where the complexity concentrates. Cross-check by reading the source (`read_file_content`) at those fns.

### Step 3. Cross-reference with usage

For each candidate fn:

```
who_uses_summary(directory=..., target=<crate>::<fn>)
```

Compute `out_degree × total_count` as a rough blast-radius-weighted refactor priority.

### Step 4. Pre-/post-snapshot to verify simplifications

Before refactor: `analyze_complexity(file_path=...)` → record total + avg cyclomatic.
After refactor: same call → compare.
Drops in total cyclomatic or avg-per-fn confirm the refactor reduced complexity.

### Decision frames

| Finding | Action |
|---|---|
| File `Total cyclomatic` ≥ 50 + high fan-in on a fn inside it | Top refactor priority |
| File `Avg per function` ≥ 5 + many functions | Whole-file refactor / split file |
| File `Total cyclomatic` ≥ 50 + fan-in concentrated in tests | Probably test-heavy; lower priority |
| `Function calls` >> function count | Tight intra-file coupling — extract helpers |

### Pattern reference

| Signal | Means |
|---|---|
| `Avg per function` close to 1, but total very high | Many simple fns — refactor target is file structure, not individual fns |
| One fn with high out-degree in `get_call_graph` + file has high cyclomatic | That fn is likely the gnarl |
| Clean call graph + still high cyclomatic | Complexity is in match arms / nested if — read source |

### Limitations

- No per-function cyclomatic — you must combine `analyze_complexity` (file-level) with `get_call_graph` (structural) and visual inspection.
- No cognitive complexity metric — only cyclomatic.
- No type-complexity metric — generics with many params, deeply-nested type aliases, etc., are invisible to this tool.

---

## W16 — Snapshot / branch comparison

Verify a refactor didn't break invariants, or compare two snapshots over time.

### Scope — two snapshots (typically two branches or two timestamps)

### Step 1. Build both snapshots

```
build_hypergraph(directory=<path_to_branch_1>)
build_hypergraph(directory=<path_to_branch_2>)
```

Independent — issue in parallel.

### Step 2. Pull paired data

For each metric of interest, call the same tool against both directories:

```
workspace_stats(directory=<path_1>) → JSON A1
workspace_stats(directory=<path_2>) → JSON A2

dead_pub_report(directory=<path_1>) → JSON B1
dead_pub_report(directory=<path_2>) → JSON B2

crate_edges(directory=<path_1>) → JSON C1
crate_edges(directory=<path_2>) → JSON C2

get_declared_reexports(directory=<path_1>, module=<root>) → JSON D1
get_declared_reexports(directory=<path_2>, module=<root>) → JSON D2
```

Run all reads in parallel.

### Step 3. Diff

| Diff target | What to compare |
|---|---|
| `workspace_stats` | Item counts, visibility distribution, `pub_crate_share` trend |
| `dead_pub_report` | Per-crate dead-pub count delta |
| `crate_edges` | Per `(consumer, producer)` edge: `unique_symbols` delta, `total_refs` delta |
| `get_declared_reexports` | New entries = API widened; lost entries = API narrowed |
| `module_tree` per crate | Item count delta, depth delta |
| `analyze_complexity` per file | Per-fn cyclomatic delta |

### Step 4. Targeted recipes

#### Recipe 16.1 — "Verify a refactor didn't widen the API"

```
get_declared_reexports(module=<root>) → before
… refactor …
get_declared_reexports(module=<root>) → after
```

New entries = widened API. Investigate each before merging.

#### Recipe 16.2 — "Dead-pub trend"

`dead_pub_report` per branch; compare counts. Trend up = facades or pub surface decaying. Trend down = active demotion / cleanup.

#### Recipe 16.3 — "Edge weight changes"

`crate_edges` per branch. Per `(consumer, producer)` compare `unique_symbols` and `total_refs`. New high-weight edges = new coupling. Lost edges = cleaned-up coupling.

#### Recipe 16.4 — "Method count by type"

`workspace_stats.items_by_kind.Method` trend. Up = adding methods (Layer 4 captures the count). Down = removing or consolidating.

#### Recipe 16.5 — "Complexity trend"

`analyze_complexity` per branch on the same files. Per-fn cyclomatic delta. Negative deltas confirm refactors landed. Positive deltas may be regressions.

### Decision frames

| Finding | Means |
|---|---|
| `pub_crate_share` increased | Encapsulation discipline improved |
| `dead_pub_report` count decreased | Active demotion / cleanup happening |
| New entries in `get_declared_reexports` post-refactor | API widened — investigate intent |
| `crate_edges` row added with high `total_refs` | New coupling introduced |
| `crate_edges` cycle appeared | Architectural break — block merge |
| Per-fn cyclomatic delta positive | Possible regression |

### Pattern reference

| Signal | Means |
|---|---|
| Many `dead_pub_report` entries appear together | Refactor removed callers but didn't demote source |
| `workspace_stats.items_by_kind.Method` jumped 100+ in one PR | Big impl-block addition; investigate |
| `crate_edges` added new edge between previously unrelated crates | Architectural change; verify intent |

---

## W17 — Architectural rule enforcement

Declarative crate-edge rule check. CI-friendly: rules are passed as a list, the tool returns concrete violations with `consumer_crate`, `producer_crate`, `sample_symbol`, `unique_symbols`, `total_refs`. Empty `violations` is the pass signal.

### Scope — workspace-wide, declarative rule check

### Step 1. Define rules

Each rule has a glob-style `consumer` pattern and `producer` pattern (with `*` wildcards), plus optional `except` (consumer-side override), `severity`, and `message`:

```
rules = [
  { consumer: "domain*",    producer: "tokio",      severity: "error", message: "domain crates must be runtime-agnostic" },
  { consumer: "domain*",    producer: "serde_json", severity: "warn"  },
  { consumer: "domain*",    producer: "reqwest",    severity: "error" },
  { consumer: "domain*",    producer: "hyper",      severity: "error" },
  { consumer: "domain*",    producer: "bevy*",      severity: "error" },
]
```

Glob semantics: `*` matches zero or more characters in the crate name. `consumer="domain*"` catches `domain`, `domain_core`, `domain_types`, etc.

### Step 2. Run the check

```
forbidden_dependency_check(directory=..., rules=[...])
```

Returns:

```json
{
  "rule_count": 5,
  "violation_count": 2,
  "violations": [
    { "rule": { "consumer": "domain*", "producer": "tokio" },
      "edge": { "consumer_crate": "domain_x", "producer_crate": "tokio" },
      "sample_symbol": "tokio::spawn", "unique_symbols": 5, "total_refs": 17 }
  ]
}
```

One violation per (rule × matching edge). The tool is a pure filter over `crate_edges` — same data, declarative shape, no extra graph cost.

### Step 3. Triage

For each violation:
- `read_file_content(file=<sample_call_site>)` at the span (use `who_imports(target=<sample_symbol>)` to surface the actual file:line).
- Confirm whether the import is legitimate (e.g. an integration test in the domain crate) or a real layering break.
- For legitimate cases, add an `except` clause to the rule (or narrow the `consumer` glob) and re-run.
- For real breaks, fix the import — either move the offending code out of the domain crate or factor the dependency through an abstraction.

### Step 4. Targeted recipes

#### Recipe 17.1 — "Layered architecture audit (DAG enforcement)"

For each layer pair where the lower layer must not consume from the upper:

```
forbidden_dependency_check(rules=[
  { consumer: "domain*",      producer: "agent*",       severity: "error" },
  { consumer: "domain*",      producer: "tui*",         severity: "error" },
  { consumer: "agent*",       producer: "tui*",         severity: "error" },
])
```

Empty `violations` confirms the layer DAG holds. Any non-empty result is a layering break.

#### Recipe 17.2 — "Async boundary check"

The domain crate must not import async runtimes:

```
forbidden_dependency_check(rules=[
  { consumer: "domain*", producer: "tokio",   severity: "error" },
  { consumer: "domain*", producer: "futures", severity: "error" },
  { consumer: "domain*", producer: "async-*", severity: "error" },
])
```

Real example from `coding-agent-bad`: `domain` imported `tokio::sync::Mutex` because a refactor never finished — surfaced as a single violation with `unique_symbols=1`.

#### Recipe 17.3 — "Domain crate framework hygiene"

Domain crates should be framework-agnostic:

```
forbidden_dependency_check(rules=[
  { consumer: "domain*", producer: "bevy*",     severity: "error" },
  { consumer: "domain*", producer: "reqwest",   severity: "error" },
  { consumer: "domain*", producer: "hyper",     severity: "error" },
  { consumer: "domain*", producer: "axum",      severity: "error" },
  { consumer: "domain*", producer: "actix-web", severity: "error" },
])
```

### Decision frames

| Question | Answer |
|---|---|
| Should this rule live in CI? | Yes — `violation_count > 0` is a non-zero exit code candidate. |
| Should it live in-IDE? | Cheap enough (filter over `crate_edges`) to run on every save. |
| How to handle "partial" rules? | Use `except` for consumer-side overrides (e.g. domain crate may import `serde` but not `serde_json` — express as two rules with the broader rule narrowed). |
| Multiple rules contradicting? | The check evaluates each rule independently — overlapping rules each produce their own violation rows. Keep rules orthogonal. |

### Pattern reference

| Use case | Rule shape |
|---|---|
| Layered DAG | `consumer: "lower*", producer: "upper*"` |
| Async-free domain | `consumer: "domain*", producer: "tokio"` |
| Framework-free domain | `consumer: "domain*", producer: "bevy*"` |
| Forbid binary→library reverse | `consumer: "lib*", producer: "bin*"` |

### Limitations

- Only crate-level edges. Can't enforce "domain modules must not import …" within a single crate — for that, drop to `get_imports(module=...)` and hand-roll a check.
- No cycle detection. `crate_edges` lists forward edges; the check is filter-only. For cycles, walk `crate_edges` manually (a small `python -c` script over the persisted JSON does the job).
- Same caveat as `crate_edges`: cross-crate method calls / trait dispatch are NOT counted in `total_refs`. A consumer that imports a trait but only uses it via method dispatch may register `total_refs=0` while still violating the rule.
- Glob is `*`-only — no `?`, no character classes, no negation in the pattern itself (use `except` for that).

---

## W18 — Attribute-driven audits

Per-Item attributes (and `///` doc-comment lines) are first-class graph data. Two tools cover the per-item view and the workspace-wide search.

### Scope — single Item or workspace-wide attribute audit

### Step 1. Per-item attribute fingerprint

```
item_attributes(directory=..., target=<crate>::Y)
```

Returns the trimmed source text of every `#[...]` attribute (e.g. `#[derive(Debug, Clone)]`, `#[must_use]`, `#[non_exhaustive]`, `#[inline]`) and every `///` doc-comment line in source order. Useful for rendering context around an Item without reading the full file.

### Step 2. Workspace-wide attribute search

```
items_with_attribute(directory=..., crate_name=X, attribute_pattern="#[must_use]")
```

Anchored prefix match on each attribute string OR on the body of a `///` doc-comment. Each result row carries `match_location: "attr"` or `"doc"` so callers can filter visually. Case-sensitive.

`attribute_pattern` is a substring anchored as a prefix — e.g. `"#[deprecated"` matches `#[deprecated]`, `#[deprecated(note = "...")]`, but not `// #[deprecated]` in a comment block.

### Step 3. Combine with usage data

For each finding, run:

```
who_uses(directory=..., target=<finding.qualified_name>)
who_uses_summary(directory=..., target=<finding.qualified_name>)
```

Pairs the attribute presence with consumption signal: "deprecated items still being called", "must_use functions whose return is being ignored at the call site" (the latter requires reading call sites manually), etc.

### Step 4. Targeted recipes

#### Recipe 18.1 — "Deprecation rollout audit"

```
items_with_attribute(crate_name=X, attribute_pattern="#[deprecated")
```

For each finding, `who_uses_summary(target=<qualified_name>)` → rank by remaining caller count. Items with non-zero callers are the migration backlog; items with zero callers are safe to delete the deprecated attribute (and the item itself).

#### Recipe 18.2 — "Serialization surface inventory"

```
items_with_attribute(crate_name=X, attribute_pattern="#[derive(Serialize")
```

Surfaces every type that participates in the wire format. Cross-reference with `module_tree(krate=X)` to confirm visibility (a `pub` Serialize struct is wire-format-stable; a `pub(crate)` Serialize struct may be incidental).

Note: `#[derive(Debug, Clone, Serialize)]` matches as one attribute string (the derive list isn't split). `attribute_pattern="#[derive(Serialize"` will match it; `attribute_pattern="#[derive(Clone)]"` will NOT match `#[derive(Debug, Clone, Serialize)]` because of the differing literal prefix.

#### Recipe 18.3 — "Must-use compliance"

```
items_with_attribute(crate_name=X, attribute_pattern="#[must_use]")
```

Returns every `#[must_use]` Item — the API contract list. Cross-reference with `module_tree` to find pub fns/types that should carry `#[must_use]` but don't (manual review required — there is no anti-attribute audit).

#### Recipe 18.4 — "Forward-compat audit"

```
items_with_attribute(crate_name=X, attribute_pattern="#[non_exhaustive]")
```

Surfaces enums and structs that are evolution-safe (callers must use `_` arms / non-positional construction). Combine with `enum_variants(target=<finding>)` (W24) to inventory variants per `#[non_exhaustive]` enum and predict downstream breakage when adding a new variant.

#### Recipe 18.5 — "Test-only fns"

```
items_with_attribute(crate_name=X, attribute_pattern="#[cfg(test)]")
```

Catches `#[cfg(test)] fn` declarations. Does NOT catch `#[cfg(test)] mod tests { fn ... }` — module-gated test fns inherit the gate from their parent module and don't carry the attribute themselves. For module-level cfg-gating, walk `module_tree` and inspect parent attributes manually.

### Decision frames

| Situation | Pattern shape |
|---|---|
| Match exact attribute | `attribute_pattern="#[must_use]"` (anchored prefix; the closing `]` makes the match strict) |
| Match attribute family | `attribute_pattern="#[deprecated"` (no closing bracket; matches `#[deprecated]`, `#[deprecated(...)]`) |
| Match doc-comment substring | `attribute_pattern="TODO"` against doc-comment bodies (anchored at the start of each `///` line body) |
| Match derive trait | `attribute_pattern="#[derive(Serialize"` (matches any derive list containing Serialize first; for non-first position the substring won't match — derives are not split) |

### Pattern reference

| Audit | Pattern |
|---|---|
| Deprecations | `"#[deprecated"` |
| Must-use | `"#[must_use]"` |
| Non-exhaustive | `"#[non_exhaustive]"` |
| Inline hints | `"#[inline"` |
| Test-only fns | `"#[cfg(test)]"` |
| Serializable types | `"#[derive(Serialize"` |
| Doc TODOs | `"TODO"` (matches `match_location: "doc"`) |

### Limitations

- Derive lists count as a single attribute string — `#[derive(Debug, Clone, Serialize)]` is one entry, not three. Substring match against the derive list works but isn't position-independent (the list is rendered as written).
- Nested attributes (`#[serde(skip)]` inside `#[derive(...)]`) are NOT split; they're rendered as one string when they appear in the source as one.
- Match is anchored prefix — substring-anywhere matching requires reading the full attribute list and filtering client-side.
- `#[cfg(test)]` on a `mod` is NOT inherited by child fns in the result list — those child fns won't carry the attribute on their own row.

---

## W19 — Signature-based fn discovery

Recorded `FunctionSignature` data per fn unlocks signature-shape filtering at workspace scale: "every async fn returning `Result<_, MyError>`", "every fn with ≥5 params", "every fn that takes `&Path`".

### Scope — single crate or per-fn signature inspection

### Step 1. Per-fn signature

```
function_signature(directory=..., target=<crate>::Y)
```

Returns the recorded signature: `is_async`, `self_param` (Owned/Ref/RefMut, or null for free fns / assoc fns without self), `params` (each with `name`, `type_string`, `by_ref`, `mutability`), `return_type`, `generics` (each with declaration-site trait bounds). Useful when reading the source would be more expensive than asking the graph.

Type strings come from RA's `HirDisplay` rendered against the function's owning crate; allocator/hasher type parameters (`, Global>`, `, RandomState>`, `, BuildHasherDefault<...>>`) and `LazyLock`/`OnceLock` init-fn pointer parameters are stripped.

### Step 2. Crate-wide filtered enumeration

```
functions_with_filter(directory=..., krate=X,
                      min_param_count=<n>,
                      has_param_type=<substring>,
                      returns_type_pattern=<substring>,
                      is_async=<bool>,
                      self_kind=<"none"|"owned"|"ref"|"ref_mut">,
                      limit=50, offset=0,
                      summary=false)
```

Knobs:
- `min_param_count` — fns with at least N non-self params.
- `has_param_type` — case-sensitive substring against any param's stringified type (e.g. `"&Path"`, `"tokio::sync::Mutex"`).
- `returns_type_pattern` — case-sensitive substring against return type (e.g. `"Result<"`, `"Result.*MyError"` — note: substring, not regex).
- `is_async` — `true` for async-only / `false` for sync-only / omit for both.
- `self_kind` — `"none"` (free fns + assoc fns without self), `"owned"` (`self`), `"ref"` (`&self`), `"ref_mut"` (`&mut self`).
- `limit` (default 50), `offset` (default 0).
- `summary=true` drops the `signature` payload from each match — useful for lightweight enumeration when the full signatures exceed the MCP token budget.

Sorted by qualified name. Trait-impl method bodies are NOT included (Layer 4 limitation — impl items aren't Item nodes).

### Step 3. Pagination

`total_match_count` returned per call. Compare to `offset + match_count` to detect "more pages exist". Bump `offset` by `limit` until `match_count < limit`.

### Step 4. Targeted recipes

#### Recipe 19.1 — "Migration helper"

```
functions_with_filter(krate=X, returns_type_pattern="Result<", is_async=true, has_param_type="OldError")
```

Surfaces every async fn returning a Result that mentions the legacy error type. Pair with `who_calls(target=<finding>)` (W14) to scope migration scope per fn.

#### Recipe 19.2 — "Builder pattern detection"

```
functions_with_filter(krate=X, self_kind="owned")
```

Consuming methods (`fn foo(self) -> Self`) are the builder-pattern signature. Combine with `returns_type_pattern="Self"` for a tight builder filter.

#### Recipe 19.3 — "Filesystem-touching surface"

```
functions_with_filter(krate=X, has_param_type="&Path")
```

Or `has_param_type="PathBuf"`. Returns every fn that takes a path. Pair with `who_uses_summary` per finding to rank by fan-in — the top filesystem-touching fns are the natural seam for an injected `FileSystem` trait if you want to factor I/O.

#### Recipe 19.4 — "Self-kind consistency"

For trait T's methods (from `module_tree`):

```
function_signature(target=<crate>::T::method)
```

per method. Compare `self_param` shape across the trait's method set — inconsistent self-kind on a trait (some `&self`, some `&mut self`, some owned `self`) is usually a smell.

For implementor crates, run `functions_with_filter(krate=<impl_crate>, self_kind="ref_mut")` and check whether impl methods match the trait's declared self-kind.

#### Recipe 19.5 — "High-arity fns"

```
functions_with_filter(krate=X, min_param_count=5)
```

Refactor candidates — fns with five or more params usually want a struct-of-args, builder pattern, or splitting. Cross-reference with `analyze_complexity` for the file containing the fn — high-arity + high cyclomatic = top refactor priority.

### Decision frames

| Goal | Mode |
|---|---|
| Workspace inventory ("list every async Result fn") | `summary=true` (drops signature payload) |
| Single-fn analysis | `function_signature(target=Y)` (no need for filter) |
| Migration prep | `functions_with_filter(returns_type_pattern=<old type>, is_async=...)` |
| Refactor candidate detection | `functions_with_filter(min_param_count=5)` |
| Self-kind audit | `function_signature` per method, compare manually |

### Pattern reference

| Filter combo | Result |
|---|---|
| `min_param_count=5` | Refactor candidates |
| `has_param_type="&Path"` | I/O surface |
| `returns_type_pattern="Result<"` + `is_async=true` | Async fallible API |
| `self_kind="owned"` | Consuming / builder methods |
| `self_kind="none"` | Free fns + static assoc fns |
| `has_param_type="tokio::sync::Mutex"` | Async-locked critical sections |

### Limitations

- `has_param_type` and `returns_type_pattern` are substring matches on `HirDisplay` output, not type-aware. `Result<MyError>` and `MyError` both substring-match `"MyError"` — disambiguation is the caller's job.
- Default type parameters (e.g. `, Global>` from `Vec<T, Global>`) are trimmed but other defaults may still appear depending on RA's render.
- `impl Trait` signatures may differ slightly from source (RA's `HirDisplay` renders the resolved trait obj, not the source `impl Trait` syntax).
- Trait-impl method bodies are NOT included — only free fns, inherent assoc fns, and trait declaration fns. To audit impl methods, walk `module_tree` filtered to impl Items.
- Where-clause bounds on generics are NOT included in `trait_bounds` — only declaration-site bounds (RA limitation).

---

## W20 — Unsafe-block audit

Every `unsafe { ... }` block in the workspace's local crates surfaces with its enclosing fn, line count, and a `has_safety_comment` heuristic flag. Live computation; nothing cached.

### Scope — workspace-wide

### Step 1. Pull every unsafe block

```
unsafe_audit(directory=...)
```

Returns:

```json
{
  "directory": "...",
  "finding_count": <n>,
  "findings": [
    { "file": "src/foo.rs", "span": [1024, 1100], "line_count": 4,
      "enclosing_function": "<64-char-hex>",
      "enclosing_function_name": "my_crate::do_unsafe_thing",
      "has_safety_comment": true }
  ]
}
```

Sorted by `(file, span)`. Per-invocation cost is dominated by the workspace load (~2-3s).

### Step 2. SAFETY-comment compliance

Filter `has_safety_comment=false`:

```
findings | where has_safety_comment=false
```

The flag is true when `SAFETY` appears as a substring in any of the 5 source lines preceding the `unsafe` keyword. False = undocumented unsafe — the audit's primary output. Empty-after-filter is the healthy signal.

### Step 3. Block-size distribution

Sort by `line_count` descending → top candidates for breakdown into smaller annotated blocks. Idiomatic Rust prefers small unsafe blocks with one-fact-per-block SAFETY comments; a 30-line unsafe block usually mixes too many invariants under one umbrella SAFETY note.

### Step 4. Blast-radius weighting

For each block:

```
recursive_callers_count(directory=..., target=<enclosing_function_name>, depth=8)
```

(Detail in W14.) The integer answers "how many fns transitively touch unsafe code via this fn?" A block whose enclosing fn has `transitive_callers=200` is on the hot path; a block whose enclosing fn has `transitive_callers=2` is a leaf.

### Step 5. Render context

For each finding worth investigating:

```
read_file_content(file_path=<finding.file>)
```

Slice [span[0] - 500, span[1] + 200] for the SAFETY comment and surrounding fn body. Review whether the comment matches the actual invariant being upheld.

### Step 6. Targeted recipes

#### Recipe 20.1 — "Undocumented-unsafe inventory"

Filter `has_safety_comment=false`; sort by `line_count` desc. Top candidates are the largest undocumented unsafe blocks — the highest-leverage places to add SAFETY comments first.

#### Recipe 20.2 — "Unsafe blast radius"

For each finding, `recursive_callers_count(target=<enclosing_function_name>, depth=8)`. Sort by `transitive_callers` desc → unsafe ranked by how many callers are downstream. Combine with `has_safety_comment=false` to identify high-blast-radius undocumented unsafe.

#### Recipe 20.3 — "Per-crate unsafe surface"

Group findings by the first path component of `file` (the crate dir). Crates with disproportionate unsafe density are the targets for FFI / perf-critical-section review.

### Decision frames

| Finding | Verdict |
|---|---|
| Small undocumented block (`line_count ≤ 2`) using `mem::transmute` between equivalent reprs | Tolerable; idiom |
| Small undocumented block doing pointer arithmetic | Add SAFETY comment |
| Large undocumented block (`line_count ≥ 10`) | Break into smaller blocks each with its own SAFETY |
| Block with documented SAFETY but high blast radius | Re-review the comment quality on PR |
| Block with no enclosing fn (e.g. const initializer) | `enclosing_function_name=null`; harder to attribute risk — review case-by-case |

### Pattern reference

| Audit | Invocation |
|---|---|
| Undocumented unsafe | `unsafe_audit` filtered to `has_safety_comment=false` |
| Top by size | `unsafe_audit` sorted by `line_count` desc |
| Top by blast radius | `unsafe_audit` × `recursive_callers_count(target=enclosing_fn)` |
| Per-crate density | `unsafe_audit` grouped by file's first path component |

### Limitations

- `has_safety_comment` is a substring heuristic — it checks for `SAFETY` in the 5 lines preceding the `unsafe` keyword. It does NOT validate comment quality, freshness, or whether the comment matches the actual invariant.
- `enclosing_function_name` is null for unsafe in const initializers, trait bounds, and closures-without-fn-parent. These cases need manual review.
- Live computation per invocation (no caching). Workspace load is ~2-3s; subsequent calls in the same session may be faster if RA's incremental cache is warm.
- Only counts `unsafe { ... }` blocks — does not surface `unsafe fn` declarations as findings (use `items_with_attribute` or `function_signature` for that).

---

## W21 — Global mutable state audit

Type-aware audit of every local `static` item whose HIR type matches `static mut` / `LazyLock<...>` / `OnceLock<...>` / `OnceCell<...>`.

### Scope — workspace-wide

### Step 1. Pull every match

```
mut_static_audit(directory=...)
```

Returns:

```json
{
  "directory": "...",
  "finding_count": 5,
  "findings": [
    { "item": "<64-char-hex>",
      "qualified_name": "my_crate::CONFIG",
      "matched_pattern": "LazyLock<...>",
      "type_string": "LazyLock<Mutex<Foo>>",
      "file": "src/config.rs",
      "span": [200, 260] }
  ]
}
```

Sorted by `(qualified_name, matched_pattern)`. A single static matching multiple patterns produces one finding per pattern. `type_string` is post-processed: init-fn pointers and allocator parameters are dropped.

### Step 2. Per-pattern audit

Filter findings by `matched_pattern`:
- `static mut` — the riskiest; requires `unsafe` to access. FFI / legacy compatibility hot spot.
- `LazyLock<...>` — process-lifetime init; common, often legitimate.
- `OnceLock<...>` — write-once cells.
- `OnceCell<...>` — same shape, different crate.

### Step 3. Per-finding fan-in

For each finding:

```
who_uses(directory=..., target=<finding.qualified_name>)
who_uses_summary(directory=..., target=<finding.qualified_name>)
```

Quantifies how many sites depend on the global. High fan-in = removing it requires touching many sites; the global is load-bearing.

### Step 4. Render context

```
read_file_content(file_path=<finding.file>)
```

at `span` widened by ~30 lines. Review init expression and surrounding documentation.

### Step 5. Targeted recipes

#### Recipe 21.1 — "Hidden singleton inventory"

List every `LazyLock` / `OnceLock` / `OnceCell` finding. The top candidates for "should this be DI'd instead of a global?" — singletons are easy to ship and hard to test. Rank by `who_uses_summary.total` desc to find the most consumed singletons (the most painful to remove, but also the highest-leverage if removed).

#### Recipe 21.2 — "static mut audit"

Filter to `matched_pattern="static mut"`. These are FFI / legacy compatibility cases. Each warrants a SAFETY review — `static mut` access requires `unsafe`, and the audit surfaces every site where the unsafe pre-condition must hold. Cross-reference with `unsafe_audit` (W20) — many `static mut` sites have a corresponding `unsafe { /* read STATIC_MUT */ }` block elsewhere.

#### Recipe 21.3 — "Cross-pattern singletons"

A static of type `LazyLock<OnceCell<T>>` would match both patterns and produce two findings for the same item. Group by `qualified_name` → cross-pattern singletons (uncommon; usually intentional layered init).

### Decision frames

| Pattern | Likely verdict |
|---|---|
| `LazyLock<HashMap<K, V>>` for a constant lookup table | Process-lifetime constant — fine |
| `LazyLock<Mutex<State>>` for shared mutable state | Hidden singleton — DI candidate |
| `OnceLock<Sender<T>>` for a global channel | Often DI candidate (carries side effects) |
| `static mut COUNT: usize = 0` | FFI / legacy — review SAFETY pre-conditions |
| `OnceCell<Config>` populated at startup | Probably fine; init order matters |

### Pattern reference

| Audit | Invocation |
|---|---|
| All globals | `mut_static_audit(directory=...)` |
| Risky `static mut` only | filter to `matched_pattern="static mut"` |
| Singletons | filter to `matched_pattern` ∈ {`LazyLock<...>`, `OnceLock<...>`, `OnceCell<...>`} |
| Cross-reference fan-in | per-finding `who_uses_summary(target=qualified_name)` |
| Cross-reference unsafe | `unsafe_audit` with `static mut` finding's qualified name in the unsafe block |

### Limitations

- The `lazy_static!` macro is NOT detected. Its expansion produces a generated wrapper type whose name doesn't contain `LazyLock`. Use `items_with_attribute(crate_name=X, attribute_pattern="lazy_static")` (matches `#[macro_use]` + `lazy_static!` invocations indirectly via items declared inside it) or `search(keyword="lazy_static!")` to cover that case.
- `parking_lot::Mutex<T>` constructor calls inside a fn body are NOT scanned — only `static` items are checked. For `Mutex` usage patterns inside fns, drop to `functions_with_filter(has_param_type="Mutex")`.
- Type-string match is post-processed (init-fn pointers dropped) — comparing `type_string` literally across findings is reliable; comparing against ad-hoc strings outside the tool may diverge.

---

## W22 — Re-export chain tracing

Trace `pub use` chains through facade modules. Pair with the `pub type` audit to catch aliases that are masquerading as re-exports.

### Scope — single Item with a long re-export chain, or a crate's facade audit

### Step 1. Trace a chain

```
re_export_chain(directory=..., target=<crate>::module::Y)
```

Walks every `pub use` re-export of the canonical target up to 8 hops with cycle detection, breadth-first. Returns `links` (one per visited binding) with `from_module`, `visible_name`, and `depth`. Useful for auditing the public surface — "this Item is exposed at facade depth 4? do we need that?".

### Step 2. Detect pub-type masquerading as re-export

```
pub_use_pub_type_audit(directory=..., crate_name=X)
```

Returns every `pub type` alias in the named crate whose owning module also carries a `pub use ... as <alias_name>` (or `pub use ::<alias_name>`) binding. Indicates the alias may be acting as a re-export disguised as a `pub type` declaration. The model does NOT record what the alias's RHS resolves to, so the heuristic can't confirm — verify with `find_definition(symbol_name=<alias>)` before acting.

### Step 3. Targeted recipes

#### Recipe 22.1 — "Decode a long facade chain"

When an Item is exported via 3+ hops, callers face a guess-the-canonical-path problem. `re_export_chain(target=<canonical>)` shows each step:

```
re_export_chain(target=domain::auth::AuthError)
  → links: [
      { from_module: "domain", visible_name: "AuthError", depth: 1 },
      { from_module: "shared", visible_name: "AuthError", depth: 2 },
      { from_module: "agent",  visible_name: "AuthError", depth: 3 }
    ]
```

This reveals which crate facades pin the visibility. Combine with `who_imports(target=domain::auth::AuthError)` to see which facade path consumers actually use — if everyone reaches for the canonical, drop the facades.

#### Recipe 22.2 — "Crate facade hygiene"

```
pub_use_pub_type_audit(crate_name=X)
```

Surfaces `pub type Y = path::Y;` that should be `pub use path::Y;`. The aliases keep the public name but introduce an extra type-level indirection that `pub use` would express more directly. Convert per finding:

```
// Before:
pub use foo::FooImpl;  // also re-exported
pub type Foo = foo::FooImpl;  // <-- the audit flags this

// After (one of):
pub use foo::FooImpl as Foo;  // single re-export
// or just keep the pub use FooImpl; and drop the alias.
```

### Decision frames

| Situation | Action |
|---|---|
| `pub type Y = Path;` where Y has no generic shape change | Should be `pub use Path as Y;` — drop the alias |
| `pub type Y<T> = Path<T, DefaultParam>;` (shape-changing) | Correct as `pub type` — keep |
| Re-export chain depth ≥ 4 | Audit each hop; consumers usually skip to the canonical |
| Re-export chain depth = 1 | Single facade — fine |

### Pattern reference

| Use case | Invocation |
|---|---|
| Trace one Item's facade exposure | `re_export_chain(target=Y)` |
| Audit a crate's pub type aliases | `pub_use_pub_type_audit(crate_name=X)` |
| Verify alias RHS | `find_definition(symbol_name=<alias>)` after the audit |

### Limitations

- `pub_use_pub_type_audit` is heuristic: compares alias name to `pub use` bindings declared in the same module; can't confirm the RHS resolves to the same target. False positives when an alias and a `pub use` happen to share a name but point to different types.
- `re_export_chain` walks up to 8 hops with cycle detection. Beyond 8, deeper chains aren't enumerated — increase if you have a workspace with extreme facade depth (rare).
- The chain only follows `pub use` re-export edges; `pub use *` glob re-exports are followed as well, but glob re-exports of glob re-exports (a chain of `pub use foo::*; pub use bar::*;`) may surface additional bindings depending on the resolver.

---

## W23 — Sortable per-crate dependency metric

Robert Martin's instability/abstractness metric per local crate, sortable by any of five keys. Surfaces architectural shape at higher resolution than reading the full `crate_edges` matrix.

### Scope — workspace-wide

### Step 1. Pull sorted metric

```
crate_dependency_metric(directory=..., sort_by=<key>, top_n=<n>)
```

Returns one row per local crate:

```json
{
  "crate_count": 12,
  "metrics": [
    { "crate_id": "<64-char-hex>", "crate_name": "my_crate",
      "efferent": 5, "afferent": 2,
      "instability": 0.71, "abstractness": 0.18, "item_count": 142 }
  ]
}
```

- `efferent` (Ce) — distinct outgoing producer crates (fan-out).
- `afferent` (Ca) — distinct incoming consumer crates (fan-in).
- `instability = Ce / (Ce + Ca)` — 0 = max stable (lots of consumers, no dependencies); 1 = max unstable.
- `abstractness = (traits + pub_type_aliases) / total_items` — high abstractness = trait-and-alias-heavy crate (facades, abstract layers).

`sort_by` accepts `instability`, `item_count`, `afferent`, `efferent`, `abstractness` (all descending). `top_n` slices the head after sorting.

### Step 2. Cross-reference

Pick the top crate from the metric; pull `crate_edges(directory=...)` filtered to that crate to see the per-edge breakdown:

```
crate_edges(directory=...) | filter consumer_crate=<top> OR producer_crate=<top>
```

The metric ranks; `crate_edges` shows the symbols carrying each edge.

### Step 3. Targeted recipes

#### Recipe 23.1 — "Most-depended-on crate"

```
crate_dependency_metric(sort_by="afferent", top_n=10)
```

Top by `afferent` = "what's the architectural core?" — crates with high fan-in are the universal-types layer. Matches the `crate_edges` decomposition in W2 / W7 but at higher resolution and with the abstractness ratio attached.

#### Recipe 23.2 — "Most-dependent crate"

```
crate_dependency_metric(sort_by="efferent", top_n=10)
```

Top by `efferent` = "what's the workhorse / orchestrator?" — crates with high fan-out integrate many services. Often the binary, the integration crate, or a god-crate candidate.

#### Recipe 23.3 — "Stable-but-concrete vs abstract main-sequence"

Robert Martin's main-sequence: stable crates should be abstract (high `abstractness`, low `instability`). Crates that are stable AND concrete (low instability, low abstractness) are the rigid concrete cores — refactor caution. Crates that are abstract AND unstable are facades over volatile internals — verify intent.

### Decision frames

| Situation | Tool |
|---|---|
| Per-edge analysis (which symbols flow on this edge?) | `crate_edges` |
| High-level ranking | `crate_dependency_metric` |
| Find architectural core | `sort_by="afferent"` |
| Find workhorses | `sort_by="efferent"` |
| Find facades / abstract layers | `sort_by="abstractness"` |
| Find god-crates | `sort_by="item_count"` |
| Find unstable cores | `sort_by="instability"` |

### Pattern reference

| Audit | Invocation |
|---|---|
| Top 10 by fan-in | `crate_dependency_metric(sort_by="afferent", top_n=10)` |
| Top 10 by fan-out | `crate_dependency_metric(sort_by="efferent", top_n=10)` |
| Top 10 by item count | `crate_dependency_metric(sort_by="item_count", top_n=10)` |
| Top 10 by abstractness | `crate_dependency_metric(sort_by="abstractness", top_n=10)` |
| Top 10 most unstable | `crate_dependency_metric(sort_by="instability", top_n=10)` |

### Limitations

- Counts edges, not symbols. For symbol-level breakdown of any (consumer, producer) pair, drop to `crate_edges` and filter.
- NaN-guarded: degenerate counts (zero items, zero edges) return 0.0 for both metrics.
- Unknown `sort_by` values produce an `invalid_params` error.
- `abstractness` counts `traits + pub_type_aliases` — it does NOT count `pub use` re-exports (which are bindings, not items). A facade that's all `pub use` will have low `abstractness` despite being structurally a facade.

---

## W24 — Enum-variant inspection

Variants of an enum are first-class — list them, then per-variant fan-in via `who_uses(target=Enum::Variant)`.

### Scope — single enum

### Step 1. Pull the variants

```
enum_variants(directory=..., target=<crate>::E)
```

Returns one row per variant in source order with `display_name`, `qualified_name`, `(file, span)`. Useful for auditing the variant set without parsing the source manually.

### Step 2. Per-variant fan-in

For each variant:

```
who_uses(directory=..., target=<crate>::E::Variant)
who_uses_summary(directory=..., target=<crate>::E::Variant)
```

Returns every pattern-match / construction site for the variant. Sort by total → "which states actually carry the load?".

### Step 3. Cross-reference with `semantic_overlaps`

```
semantic_overlaps(directory=..., item_kind="EnumVariant", threshold=0.95)
```

Variants whose source bytes hash identically (e.g. unit `Error` variant duplicated across 6 different enums) cluster together. The signal that the same logical state was modeled as separate variants on different enums — convergent enum design.

### Step 4. Targeted recipes

#### Recipe 24.1 — "Variant fan-in"

For an enum E, compute fan-in for every variant. Heaviest-used variants surface as the load-bearing states; rarely-used variants are candidates for collapse / split into a different type.

#### Recipe 24.2 — "Dead variant detection"

Variants with empty `who_uses` are dead. The constructor never executes; the pattern-match arm never matches. Either:
- The variant is reserved for future use (intentional; document with a comment).
- The variant is genuine dead state — remove (but verify the enum isn't `#[non_exhaustive]`, which preserves the variant for downstream pattern matching even if no caller in this workspace uses it).

#### Recipe 24.3 — "Convergent enum design"

`semantic_overlaps(item_kind="EnumVariant", threshold=0.95)` (Recipe 13.3) clusters variants whose source is identical. Each cluster is a candidate for harmonization — extract a shared base, introduce a trait, or collapse the convergent enums into one.

### Decision frames

| Finding | Action |
|---|---|
| Empty `who_uses` for a variant | Dead variant; remove (mind `#[non_exhaustive]`) |
| One variant carries 90% of fan-in | Other variants may be over-modeled — consider flattening |
| Variants are mostly unused unit variants | Collapse into a flag / single variant |
| Same variant duplicated across 3+ enums | Convergent design — harmonize |

### Pattern reference

| Audit | Invocation |
|---|---|
| List variants | `enum_variants(target=E)` |
| Per-variant fan-in | per-variant `who_uses_summary(target=E::Variant)` |
| Convergence | `semantic_overlaps(item_kind="EnumVariant", threshold=0.95)` |
| Cross-reference attributes | `item_attributes(target=E)` for `#[non_exhaustive]` etc. |

### Limitations

- Discriminants (the explicit `= 5` part of `Variant = 5`) are present only when the source declared them — implicit discriminants aren't computed.
- Struct/tuple variant fields are NOT enumerated separately — `Variant { a: T, b: U }` returns one row for the variant; the fields are not graph nodes. To inspect fields, drop to `read_file_content` at the variant's span.
- `who_uses(target=E::Variant)` resolves correctly for direct variant references, but pattern-matches that bind via `_` or `..` may not carry an explicit reference to the variant — the count is a lower bound.

---

## Output handling — when results are large

Some MCP outputs persist to a tool-results JSON file because they exceed the inline preview budget. Examples:
- `crate_edges` on a 17-crate workspace ≈ 67KB.
- `module_tree` at depth=3 on a 15-submodule crate ≈ 72KB.

### Detect a persisted output

The tool result includes a `<persisted-output>` block naming a path under `~/.claude/projects/.../tool-results/`. Parse that file rather than relying on the inline preview.

### Common reductions on `crate_edges`

The full edge matrix is verbose; the load-bearing summaries are usually:

1. **Per-producer fan-in** — who depends on this crate, with totals.
2. **Per-consumer fan-out** — what this crate depends on, with totals.
3. **Top-N edges** sorted by `total_refs_via_imports + total_refs_via_usages`.
4. **Symbol breakdowns within a single edge** — filter to one `(consumer, producer)` pair.

A small Python or jq script reads the persisted JSON, applies these reductions, and prints a table. Reuse the same script across workspaces — only the JSON path changes.

### `module_tree` depth as the first lever

Reach for `depth=2` before reaching for Bash post-processing. Full trees are rarely worth the bytes.

### Filter `crate_edges` client-side

The MCP returns the full matrix; per-crate analysis filters client-side by `consumer_crate` or `producer_crate`. Same for `overlaps`'s four buckets — the MCP returns all of them, you filter to the scope of interest.

---

## Index / cache management

### Build / refresh hypergraph

```
build_hypergraph(directory=..., force_rebuild?)
```

Schema bumps auto-invalidate. After a schema change (e.g. Layer 4 was v4→v5), `force_rebuild=false` still cold-rebuilds correctly because `SCHEMA_VERSION` is mixed into `graph_id`.

### Build / refresh vector index

```
index_codebase(directory=...)
```

Required for `search` (BM25) and `get_similar_code`. Independent of `build_hypergraph` — they share infrastructure but the indexes are separate.

### Clear corruption

```
clear_cache(directory?)
```

Use when an index is broken or stale beyond auto-detection.

### Verify infrastructure

```
health_check()
```

Confirms indexes exist, snapshot is current.

### Parallelism

All read tools (everything except `build_hypergraph`, `index_codebase`, `clear_cache`) are independent — call them in parallel when a workflow needs several. We routinely batch 5-10 calls per round (`build_hypergraph` against two workspaces, `who_uses_summary` on 10 collision targets, etc.) without issue.

---

## Quick recipe index

| If you want to... | Workflow |
|---|---|
| Find a symbol from a string | W1 |
| Get a workspace overview | W2 |
| Audit one crate | W3 |
| Audit one module | W4 |
| Forensics on one symbol | W5 |
| Audit a trait | W6 |
| Audit cross-crate imports/exports | W7 (`workflow-imports-exports.md`) |
| Plan a specific refactor | W8 |
| Audit naming hygiene / type overlaps | W9 (`workflow-type-overlaps.md`) |
| Distinguish test-only from production | W10 |
| Audit a type's method API | W11 |
| Audit a public API surface | W12 |
| Find duplicates by similarity | W13 |
| Trace fn-level call graphs (workspace-wide) | W14 |
| Prioritize by complexity × blast radius | W15 |
| Compare two snapshots / branches | W16 |
| Enforce architectural rules | W17 |
| Audit attributes (deprecated / must_use / non_exhaustive) | W18 |
| Find fns by signature shape | W19 |
| Audit `unsafe { ... }` blocks | W20 |
| Audit `static mut` / `LazyLock` / `OnceLock` / `OnceCell` | W21 |
| Trace `pub use` re-export chains | W22 |
| Rank crates by Robert Martin metric | W23 |
| Inspect enum variants and per-variant fan-in | W24 |

### W13 sub-recipes (semantic similarity)

| Recipe | Use case |
|---|---|
| 13.1 | Find duplicate logic worth extracting |
| 13.2 | Type-1 clone detection (literal duplicates, similarity 1.0) |
| 13.3 | Convergent enum design |
| 13.4 | Same-shape struct detection across crates |
| 13.5 | Refactor candidate ranking |
| 13.6 | Naming-convention enforcement |

### W14 sub-recipes (call graph)

| Recipe / step | Use case |
|---|---|
| Step 1 (`who_calls`) | Workspace-wide caller list |
| Step 2 (`calls_from`) | Workspace-wide callee list |
| Step 3 (`call_graph`) | Bounded reachability tree from a root fn |
| Step 4 (`callers_in_crate`) | Crate boundary check |
| Step 5 (`recursive_callers_count`) | Refactor blast-radius integer |
| Step 6 (`get_call_graph`) | Within-file structure (parser fallback) |

### W17-W24 sub-recipes

| Recipe | Use case |
|---|---|
| 17.1 | Layered architecture audit (DAG enforcement) |
| 17.2 | Async boundary check (domain crate must not import tokio/futures) |
| 17.3 | Domain crate framework hygiene |
| 18.1 | Deprecation rollout audit |
| 18.2 | Serialization surface inventory |
| 18.3 | Must-use compliance |
| 18.4 | Forward-compat audit (`#[non_exhaustive]`) |
| 18.5 | Test-only fns (`#[cfg(test)]`) |
| 19.1 | Migration helper (filter by old return type) |
| 19.2 | Builder pattern detection (`self_kind="owned"`) |
| 19.3 | Filesystem-touching surface (`has_param_type="&Path"`) |
| 19.4 | Self-kind consistency |
| 19.5 | High-arity fns (`min_param_count=5`) |
| 20.1 | Undocumented-unsafe inventory |
| 20.2 | Unsafe blast radius |
| 20.3 | Per-crate unsafe surface |
| 21.1 | Hidden singleton inventory |
| 21.2 | `static mut` audit |
| 21.3 | Cross-pattern singletons |
| 22.1 | Decode a long facade chain |
| 22.2 | Crate facade hygiene (`pub_use_pub_type_audit`) |
| 23.1 | Most-depended-on crate |
| 23.2 | Most-dependent crate |
| 23.3 | Stable-but-concrete vs abstract main-sequence |
| 24.1 | Variant fan-in |
| 24.2 | Dead variant detection |
| 24.3 | Convergent enum design |
