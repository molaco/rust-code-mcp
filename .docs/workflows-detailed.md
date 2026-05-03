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

### Worked example (`coding-agent-bad`)

17 crates, 1441 fan-in on `domain` (universal types crate), `agent` is heaviest fan-out consumer. `pub_crate_share` low — many bare `pub`. 89 dead pubs (47 in `plurimus`, a vendored UI lib — exclude). 5 cross-crate collisions, 1 module shadow, 6 within-crate duplicates. `common_fn_names` empty (good).

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

```
get_similar_code(query=<fn body>)
```

For each function, list semantic neighbors. Cluster. For each cluster, run `who_uses_summary` on each member to see if a shared helper would benefit them all. (Detail in W13.)

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

Find duplicate or near-duplicate logic that's named differently or split across crates.

### Scope — function bodies, type bodies, files

### Prerequisites

```
index_codebase(directory=...)
```

Required for `search` (BM25) and `get_similar_code`.

### Step 1. Find similar functions

```
get_similar_code(query=<fn body or description>)
```

Returns vector candidates ranked by semantic similarity.

### Step 2. Verify they're called

For each candidate:

```
who_uses_summary(directory=..., target=<qualified_name>)
```

Filter out dead candidates. Cluster live ones by domain area.

### Step 3. Confirm semantic equivalence

```
read_file_content(file_path=...)
```

at each candidate's span. Inspect manually — vector similarity isn't proof of semantic equivalence.

### Step 4. Targeted recipes

#### Recipe 13.1 — "Find similar functions across crates"

`get_similar_code(target=<fn>)` → cluster results by crate. If a single concept has implementations in 3+ crates, factor into a shared crate.

#### Recipe 13.2 — "Refactor candidate finding"

`get_similar_code(target)` + `who_uses_summary` per candidate → identify dedupe candidates worth the effort. Skip candidates with low fan-in (not worth deduping unused code).

#### Recipe 13.3 — "Naming-convention enforcement"

`module_tree(crate)` lists fn names. `get_similar_code(query=<body>)` finds semantically similar bodies with different names. Mismatched names = inconsistent vocabulary.

#### Recipe 13.4 — "Cross-crate duplicate detection (corroboration)"

`overlaps.cross_crate_type_collisions` finds same-name types. `get_similar_code(query=<body>)` confirms whether they're semantically the same. Different names + similar bodies are subtler dupes (often missed by `overlaps`).

### Decision frames

| Finding | Action |
|---|---|
| 3+ semantically similar fns across crates | Factor into shared utility crate |
| 2 similar fns in same crate | Inline / unify |
| Similar fns with different names | Rename one to match the convention |
| Vector candidates that are actually different | Confirm visually before deduping |

### Pattern reference

| Signal | Means |
|---|---|
| High vector similarity + identical fn signatures | Strong dedupe candidate |
| High vector similarity + different signatures | Conceptually similar but possibly intentionally separate |
| Vector candidates in `tests` modules | Test fixtures repeated; factor into common (W9 covers this) |

---

## W14 — Function-level call graphs

Within-file fn-to-fn edges. Parser-driven, not workspace-wide.

### Scope — single file

### Step 1. Pull the call graph

```
get_call_graph(file_path=<path>)
```

Returns function call edges within the file: `caller -> callee` pairs.

### Step 2. Identify shape

- **Leaf functions**: outputs without incoming edges. Often the public entry points OR utility leaves.
- **Entry-point functions**: outputs with no outgoing edges. Often dispatch handlers.
- **Hub functions**: high in-degree AND out-degree. Refactor candidates if also gnarly (W15).

### Step 3. Verify expected call paths

"Does `handle_request` call `validate_input`?" → check the parser-level edge directly.

### Step 4. Compose with cross-file analysis

Within-file: `get_call_graph(file_path=...)`.
Cross-file: `who_uses(target=<qualified_fn>)` aggregates module-level. Compose:

```
get_call_graph(file_path=<file_with_complex_fn>)         → see internal call tree
who_uses(directory=..., target=<crate>::<complex_fn>) → see external callers
```

### Decision frames

| Finding | Action |
|---|---|
| File has many fns with no edges (no callers, no callees) | File is a flat collection of unrelated helpers — consider splitting |
| One fn with high out-degree calling 10+ helpers | Refactor candidate; pull inline or extract module |
| Cycles in call graph | Recursion or mutual recursion; verify intent |

### Limitations

- **Not workspace-wide**: cross-file calls show up only via `who_uses` aggregation.
- **Method dispatch**: parser-level, may miss virtual dispatch through traits.

### Pattern reference

| Signal | Means |
|---|---|
| Single fn calls 20+ others in same file | Either dispatcher (legitimate) or god-fn (refactor) |
| Cycle detected | Mutual recursion; verify intent and termination |

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
| Trace within-file call structure | W14 |
| Prioritize by complexity × blast radius | W15 |
| Compare two snapshots / branches | W16 |
