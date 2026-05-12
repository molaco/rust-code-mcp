---
name: rmc-crate-audit
description: Audit one Rust crate.
argument-hint: "<crate-name> [workspace-path]"
allowed-tools: Read, Bash, mcp__rust-code-mcp__*
---

# Rust crate audit

Deep dive on one crate. Cousin of `rmc-workspace-overview` but scoped to a
single crate. For per-module analysis, hand off to `rmc-module-audit`. For
per-symbol forensics, hand off to `rmc-symbol-forensics`. For complexity-led
refactor prioritization, hand off to `rmc-complexity`.

## Scope — single crate

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

If the schema bumped or sources changed, this rebuilds. Otherwise reuse is
sub-second.

## Workflow

### Step 1. Crate snapshot (parallel)

```
module_tree(directory=..., krate=X, depth=2)
get_declared_reexports(directory=..., module=X)
dead_pub_in_crate(directory=..., krate=X)
get_imports(directory=..., module=X)
```

### Step 2. Characterize structure from `module_tree`

Default to `depth=2` to see "what submodules and root-level items exist".
Bump to `depth=3` to expand items inside each submodule. Full-depth produces
methods (Layer 4), but trees can be huge — a 15-submodule crate produces
~72KB at `depth=3`.

Look for:
- **Submodule count** — 1-2 = focused; 10+ = potentially overloaded
- **Root-level item count** — high count = facade or god module
- **`pub(in <crate>)` items** — internal API discipline signal (healthy)
- **Visibility distribution** at the root — `pub` vs `pub(crate)` vs private

### Step 3. Cross-tabulate the public surface

Build a table at the crate root using three sources:

| In `module_tree` (visibility=pub) | In `get_declared_reexports` | In `dead_pub_in_crate` | Verdict |
|---|---|---|---|
| ✓ | – | – | canonical pub, live |
| – | ✓ | – | re-export, live (facade) |
| – | ✓ | ✓ | **dead re-export** — drop the `pub use` |
| ✓ | – | ✓ | **dead canonical pub** — demote to `pub(crate)` |
| ✓ | ✓ | – | re-exported AND canonical (rare; usually drop one) |

### Step 4. Outgoing / incoming dependencies

If you have workspace `crate_edges` cached, filter:

- `consumer_crate=X` → outgoing dependencies (what X consumes)
- `producer_crate=X` → incoming dependencies (who consumes X)

Otherwise:

```
crate_edges(directory=...)
```

and filter client-side.

A crate with one consumer is single-purpose; multiple consumers means it's
a shared library. Single producer dependency means strong upstream coupling.

### Step 5. Complexity scan

For each `src/*.rs` file in the crate:

```
analyze_complexity(file_path=<path>)
```

Cross-reference top hits with `who_uses_summary` to prioritize — "complex
AND widely depended on" is the top refactor priority. Full recipe in
`rmc-complexity`.

### Step 6. Confirm canonical types are alive

For each non-dead pub item at the crate root:

```
who_uses_summary(directory=..., target=X::Type)
```

Category breakdown:

- **All-Test** → demote or wrap in `#[cfg(test)]`
- **All-Other** → critical-path, refactor with care
- **Mixed** → legitimate public API
- **Empty** → either covered by a re-export elsewhere OR genuinely dead
  (cross-check with `who_imports`)

### Step 7. Method-level analysis (Layer 4)

For key types, walk their methods from `module_tree` and check fan-in:

```
who_uses_summary(directory=..., target=X::Type::method)
```

- **Empty** → dead-method candidate
- **All-Test** → test-only helper
- **All-Other** → critical path

Pre-Layer-4 these queries errored. Post-Layer-4 they return real results,
including trait dispatch.

## Decision frames

| Finding | Action |
|---|---|
| Dead re-exports at crate root | Drop the `pub use`, demote source to `pub(crate)` |
| Dead canonical pubs | Demote to `pub(crate)` |
| Single-consumer crate with narrow API | Healthy — single integration point |
| Single-consumer crate with broad API | Suspicious — consumer probably doesn't need all of it |
| High submodule count + many facade re-exports | Likely god-crate; consider splitting |
| `pub(in <crate>)` items present | Good crate-internal discipline signal |
| Method with empty `who_uses_summary` | Verify (may be trait-dispatched); demote if unused |

## Pattern reference

| If you see... | Means |
|---|---|
| Crate has zero `pub use` declarations | No facade — everything at canonical paths (often intentional) |
| Crate has many `pub use` from one submodule | Submodule is the de-facto crate API; consider promoting to root |
| Many `pub(in <crate>)` items | Healthy internal-API discipline |
| All pubs are `pub`, no `pub(crate)` | Low encapsulation; check workspace_stats.pub_crate_share |
| One submodule contains all root-level pubs | Crate is single-purpose, well-bounded |
| Dead pub count > 5 in non-vendored crate | Genuine rot — schedule cleanup |
| Dead pub count > 30 | Likely vendored / library-style crate — verify before action |

## Output format

Severity-ranked findings table:

```
🔴 High    — broken or contradictory state (dead canonical pub at the crate
            root that's also referenced by a stale pub use)
🟡 Medium  — wasted surface (dead facade re-exports, over-broad pub API
            with all-Test consumers, god-crate signals)
🟢 Low     — naming clarity, mechanical refactors
⚪ Info    — confirms healthy structure (good pub(in <crate>) discipline)
```

Per finding: severity, location (qualified name + file:span), what's wrong,
recommended action.

## Worked example — `tui` crate in `coding-agent-bad`

- 15 submodules
- Single entry point `tui::run` (one caller in `coding-agent::interactive`)
- 7 dead pubs of which 3 are dead re-exports at the crate root
  (🟡 — drop the `pub use` lines)
- Sensible `pub(in tui)` discipline for crate-internal helpers (⚪)
- One within-crate type duplicate `TestEventSender` in
  `tui::unit::bridge_plugin` and `tui::unit::replay` (🟢 — test fixtures,
  factor to `tui::tests::common`)
- One namespace overload: `tui::unit` mixes test fixtures with
  `tui::unit::presentation::ToolName` production code (🟡 — split modules)

Total cleanup: drop dead re-exports, demote source types, factor
TestEventSender, split the `unit` module. Few hours of work.

## Limitations

- Method visibility is null on the Item Node — Layer 4 doesn't attach
  Declared bindings to methods. Read `Node.file + Node.span` for the source
  to determine method visibility.
- `dead_pub_in_crate` is computed from cross-crate consumers only — items
  used in unit tests or examples within the same crate will still be flagged
  as dead.
- Trait method dispatch through `dyn T` may miss some sites; the resolver
  is type-based. For trait-aware fan-in, use Layer 10 via `rmc-call-graph`.
- `analyze_complexity` is file-aggregated, not per-fn. For per-fn ranking
  use the workaround in `rmc-complexity`.
